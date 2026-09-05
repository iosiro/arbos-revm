use std::ops::{Deref, DerefMut};

use crate::{
    ArbitrumContextTr,
    config::ArbitrumConfigTr,
    constants::{
        ARBOS_VERSION_STYLUS_CONTRACT_LIMIT, STYLUS_DISCRIMINANT, STYLUS_FRAGMENT_DISCRIMINANT,
        STYLUS_ROOT_DISCRIMINANT,
    },
    context::ArbitrumContextMutTr,
    handler::ArbitrumHandler,
    transaction::ArbitrumTransactionError,
};
use revm::{
    Database, DatabaseCommit, ExecuteCommitEvm, ExecuteEvm, Inspector,
    context::{
        Cfg, ContextError, ContextSetters, ContextTr, Evm, FrameStack, JournalTr,
        result::{EVMError, ExecutionResult, HaltReason, ResultAndState},
    },
    handler::{
        EthFrame, EvmTr, FrameData, FrameInitOrResult, FrameResult, FrameTr, Handler, ItemOrResult,
        PrecompileProvider,
        instructions::{EthInstructions, InstructionProvider},
    },
    interpreter::{
        InstructionResult, InterpreterAction, InterpreterResult, interpreter::EthInterpreter,
        interpreter_action::FrameInit,
    },
    primitives::hardfork::{LONDON, SpecId},
    state::EvmState,
};

pub(crate) fn validate_arbos_create_output(
    action: &mut InterpreterAction,
    is_create: bool,
    arbos_version: u64,
    spec: SpecId,
    eip3541_disabled: bool,
) {
    if !is_create || eip3541_disabled || !spec.is_enabled_in(LONDON) {
        return;
    }
    let InterpreterAction::Return(result) = action else {
        return;
    };
    if !result.result.is_ok() || result.output.first() != Some(&0xef) {
        return;
    }

    let code = result.output.as_ref();
    let classic = arbos_version >= 30
        && code.len() > STYLUS_DISCRIMINANT.len()
        && code.starts_with(STYLUS_DISCRIMINANT);
    let component = arbos_version >= ARBOS_VERSION_STYLUS_CONTRACT_LIMIT
        && code.len() > STYLUS_ROOT_DISCRIMINANT.len()
        && (code.starts_with(STYLUS_ROOT_DISCRIMINANT)
            || code.starts_with(STYLUS_FRAGMENT_DISCRIMINANT));
    if !classic && !component {
        result.result = InstructionResult::CreateContractStartingWithEF;
    }
}

pub struct ArbitrumEvm<CTX, INSP, P, I = EthInstructions<EthInterpreter, CTX>, F = EthFrame>(
    pub Evm<CTX, INSP, I, P, F>,
);

impl<CTX, I, INSP, P, F> ArbitrumEvm<CTX, INSP, P, I, F> {
    /// Create a new EVM instance with a given context, inspector, instruction set, and precompile
    /// provider.
    pub fn new_with_inspector(ctx: CTX, inspector: INSP, instruction: I, precompiles: P) -> Self {
        Self(Evm {
            ctx,
            inspector,
            instruction,
            precompiles,
            frame_stack: FrameStack::new(),
        })
    }
}

impl<CTX, INSP, P, I, F> Deref for ArbitrumEvm<CTX, INSP, P, I, F>
where
    CTX: ArbitrumContextTr + ContextSetters,
    INSP: Inspector<CTX, I::InterpreterTypes>,
    I: InstructionProvider<Context = CTX, InterpreterTypes = EthInterpreter>,
    P: PrecompileProvider<CTX, Output = InterpreterResult>,
{
    type Target = Evm<CTX, INSP, I, P, F>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<CTX, INSP, P, I, F> DerefMut for ArbitrumEvm<CTX, INSP, P, I, F>
where
    CTX: ArbitrumContextTr + ContextSetters,
    INSP: Inspector<CTX, I::InterpreterTypes>,
    I: InstructionProvider<Context = CTX, InterpreterTypes = EthInterpreter>,
    P: PrecompileProvider<CTX, Output = InterpreterResult>,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<CTX, INSP, P, I> EvmTr for ArbitrumEvm<CTX, INSP, P, I, EthFrame<EthInterpreter>>
where
    CTX: ArbitrumContextTr,
    I: InstructionProvider<Context = CTX, InterpreterTypes = EthInterpreter>,
    P: PrecompileProvider<CTX, Output = InterpreterResult>,
{
    type Context = CTX;
    type Instructions = I;
    type Precompiles = P;
    type Frame = EthFrame<EthInterpreter>;

    fn ctx(&mut self) -> &mut Self::Context {
        &mut self.0.ctx
    }

    fn ctx_ref(&self) -> &Self::Context {
        &self.0.ctx
    }

    fn ctx_instructions(&mut self) -> (&mut Self::Context, &mut Self::Instructions) {
        (&mut self.0.ctx, &mut self.0.instruction)
    }

    fn ctx_precompiles(&mut self) -> (&mut Self::Context, &mut Self::Precompiles) {
        (&mut self.0.ctx, &mut self.0.precompiles)
    }

    fn frame_stack(&mut self) -> &mut FrameStack<Self::Frame> {
        &mut self.0.frame_stack
    }

    fn frame_init(
        &mut self,
        frame_input: <Self::Frame as FrameTr>::FrameInit,
    ) -> Result<
        ItemOrResult<&mut Self::Frame, <Self::Frame as FrameTr>::FrameResult>,
        ContextError<<<Self::Context as ContextTr>::Db as Database>::Error>,
    > {
        self.0.frame_init(frame_input)
    }

    fn frame_run(
        &mut self,
    ) -> Result<
        FrameInitOrResult<Self::Frame>,
        ContextError<<<Self::Context as ContextTr>::Db as Database>::Error>,
    > {
        let code = self.frame_stack().get().interpreter.bytecode.bytes();
        let is_stylus = code.starts_with(STYLUS_DISCRIMINANT)
            || (self.ctx().cfg().arbos_version() >= ARBOS_VERSION_STYLUS_CONTRACT_LIMIT
                && code.starts_with(STYLUS_ROOT_DISCRIMINANT));
        if is_stylus && let Some(action) = self.frame_run_stylus() {
            let frame = self.0.frame_stack.get();
            let context = &mut self.0.ctx;
            return frame.process_next_action(context, action).inspect(|i| {
                if i.is_result() {
                    frame.set_finished(true);
                }
            });
        }

        let frame = self.0.frame_stack.get();
        let context = &mut self.0.ctx;
        let instructions = &mut self.0.instruction;
        let mut action = frame
            .interpreter
            .run_plain(instructions.instruction_table(), context);
        validate_arbos_create_output(
            &mut action,
            matches!(frame.data, FrameData::Create(_)),
            context.cfg().arbos_version(),
            context.cfg().spec().into(),
            context.cfg().is_eip3541_explicitly_disabled(),
        );
        frame.process_next_action(context, action).inspect(|item| {
            if item.is_result() {
                frame.set_finished(true);
            }
        })
    }

    fn frame_return_result(
        &mut self,
        result: <Self::Frame as FrameTr>::FrameResult,
    ) -> Result<
        Option<<Self::Frame as FrameTr>::FrameResult>,
        ContextError<<<Self::Context as ContextTr>::Db as Database>::Error>,
    > {
        self.0.frame_return_result(result)
    }

    fn all(
        &self,
    ) -> (
        &Self::Context,
        &Self::Instructions,
        &Self::Precompiles,
        &FrameStack<Self::Frame>,
    ) {
        (
            &self.0.ctx,
            &self.0.instruction,
            &self.0.precompiles,
            &self.0.frame_stack,
        )
    }

    fn all_mut(
        &mut self,
    ) -> (
        &mut Self::Context,
        &mut Self::Instructions,
        &mut Self::Precompiles,
        &mut FrameStack<Self::Frame>,
    ) {
        (
            &mut self.0.ctx,
            &mut self.0.instruction,
            &mut self.0.precompiles,
            &mut self.0.frame_stack,
        )
    }
}

impl<CTX, INSP, INST, PRECOMPILES> ExecuteEvm
    for ArbitrumEvm<CTX, INSP, PRECOMPILES, INST, EthFrame<EthInterpreter>>
where
    CTX: ArbitrumContextMutTr<Journal: JournalTr<State = EvmState>> + ContextSetters,
    INST: InstructionProvider<Context = CTX, InterpreterTypes = EthInterpreter>,
    PRECOMPILES: PrecompileProvider<CTX, Output = InterpreterResult>,
{
    type ExecutionResult = ExecutionResult<HaltReason>;
    type State = EvmState;
    type Error = EVMError<<CTX::Db as Database>::Error, ArbitrumTransactionError>;
    type Tx = <CTX as ContextTr>::Tx;
    type Block = <CTX as ContextTr>::Block;

    #[inline]
    fn transact_one(&mut self, tx: Self::Tx) -> Result<Self::ExecutionResult, Self::Error> {
        self.0.ctx.set_tx(tx);
        ArbitrumHandler::default().run(self)
    }

    #[inline]
    fn finalize(&mut self) -> Self::State {
        self.0.journal_mut().finalize()
    }

    #[inline]
    fn set_block(&mut self, block: Self::Block) {
        self.0.ctx.set_block(block);
    }

    #[inline]
    fn replay(&mut self) -> Result<ResultAndState<HaltReason>, Self::Error> {
        ArbitrumHandler::default().run(self).map(|result| {
            let state = self.finalize();
            ResultAndState::new(result, state)
        })
    }
}

impl<CTX, INSP, INST, PRECOMPILES> ExecuteCommitEvm
    for ArbitrumEvm<CTX, INSP, PRECOMPILES, INST, EthFrame<EthInterpreter>>
where
    CTX: ArbitrumContextMutTr<Journal: JournalTr<State = EvmState>, Db: DatabaseCommit>
        + ContextSetters,
    INST: InstructionProvider<Context = CTX, InterpreterTypes = EthInterpreter>,
    PRECOMPILES: PrecompileProvider<CTX, Output = InterpreterResult>,
{
    #[inline]
    fn commit(&mut self, state: Self::State) {
        self.0.db_mut().commit(state);
    }
}

impl<CTX, INSP, P, I> ArbitrumEvm<CTX, INSP, P, I>
where
    CTX: ArbitrumContextTr,
    I: InstructionProvider<Context = CTX, InterpreterTypes = EthInterpreter>,
    P: PrecompileProvider<CTX, Output = InterpreterResult>,
{
    /// Executes the main frame processing loop.
    ///
    /// This loop manages the frame stack, processing each frame until execution completes.
    /// For each iteration:
    /// 1. Calls the current frame
    /// 2. Handles the returned frame input or result
    /// 3. Creates new frames or propagates results as needed
    #[inline]
    pub(crate) fn run_exec_loop(
        &mut self,
        first_frame_input: FrameInit,
    ) -> Result<FrameResult, ContextError<<<CTX as ContextTr>::Db as Database>::Error>> {
        let res = self.frame_init(first_frame_input)?;

        if let ItemOrResult::Result(frame_result) = res {
            return Ok(frame_result);
        }

        loop {
            let call_or_result = self.frame_run()?;

            let result = match call_or_result {
                ItemOrResult::Item(init) => {
                    match self.frame_init(init)? {
                        ItemOrResult::Item(_) => {
                            continue;
                        }
                        // Do not pop the frame since no new frame was created
                        ItemOrResult::Result(result) => result,
                    }
                }
                ItemOrResult::Result(result) => result,
            };

            if let Some(result) = self.frame_return_result(result)? {
                return Ok(result);
            }
        }
    }
}
