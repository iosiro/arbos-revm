use std::ops::{Deref, DerefMut};

use crate::{
    ArbitrumContextTr,
    chain::ArbitrumChainTr,
    constants::STYLUS_DISCRIMINANT,
    context::ArbitrumContextMutTr,
    handler::ArbitrumHandler,
    local_context::ArbitrumLocalContextTr,
    result::ArbitrumExecutionOutcome,
    state::{ArbState, ArbStateGetter},
    transaction::{ArbitrumRetryTx, ArbitrumTransaction, ArbitrumTransactionError},
};
use revm::{
    Database, DatabaseCommit, ExecuteCommitEvm, ExecuteEvm, Inspector,
    context::{
        ContextError, ContextSetters, ContextTr, Evm, FrameStack, JournalTr,
        result::{EVMError, ExecutionResult, HaltReason, ResultAndState},
    },
    handler::{
        EthFrame, EvmTr, FrameInitOrResult, FrameResult, FrameTr, Handler, ItemOrResult,
        PrecompileProvider,
        instructions::{EthInstructions, InstructionProvider},
    },
    interpreter::{
        FrameInput, InterpreterResult, interpreter::EthInterpreter, interpreter_action::FrameInit,
    },
    primitives::{U256, keccak256},
    state::EvmState,
};

pub struct ArbitrumEvm<CTX, INSP, P, I = EthInstructions<EthInterpreter, CTX>, F = EthFrame>(
    pub Evm<CTX, INSP, I, P, F>,
);

type ArbitrumEvmError<CTX> =
    EVMError<<<CTX as ContextTr>::Db as Database>::Error, ArbitrumTransactionError>;
type ScheduledRetryResult<CTX> = Result<Option<ExecutionResult<HaltReason>>, ArbitrumEvmError<CTX>>;

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
        let caller = match &frame_input.frame_input {
            FrameInput::Call(inputs) => Some(inputs.caller),
            FrameInput::Create(inputs) => Some(inputs.caller()),
            FrameInput::Empty => None,
        };
        if let Some(caller) = caller {
            self.0.ctx.local_mut().enter_frame(caller);
        }
        self.0.frame_init(frame_input)
    }

    fn frame_run(
        &mut self,
    ) -> Result<
        FrameInitOrResult<Self::Frame>,
        ContextError<<<Self::Context as ContextTr>::Db as Database>::Error>,
    > {
        if self
            .frame_stack()
            .get()
            .interpreter
            .bytecode
            .bytes()
            .starts_with(STYLUS_DISCRIMINANT)
            && let Some(action) = self.frame_run_stylus()
        {
            let frame = self.0.frame_stack.get();
            let context = &mut self.0.ctx;
            return frame.process_next_action(context, action).inspect(|i| {
                if i.is_result() {
                    frame.set_finished(true);
                }
            });
        }

        self.0.frame_run()
    }

    fn frame_return_result(
        &mut self,
        result: <Self::Frame as FrameTr>::FrameResult,
    ) -> Result<
        Option<<Self::Frame as FrameTr>::FrameResult>,
        ContextError<<<Self::Context as ContextTr>::Db as Database>::Error>,
    > {
        self.0.ctx.local_mut().exit_frame();
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
        self.0.ctx.chain_mut().set_committed_failure(None);
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

impl<CTX, INSP, INST, PRECOMPILES>
    ArbitrumEvm<CTX, INSP, PRECOMPILES, INST, EthFrame<EthInterpreter>>
where
    CTX: ArbitrumContextMutTr<Tx = ArbitrumTransaction, Journal: JournalTr<State = EvmState>>
        + ContextSetters,
    INST: InstructionProvider<Context = CTX, InterpreterTypes = EthInterpreter>,
    PRECOMPILES: PrecompileProvider<CTX, Output = InterpreterResult>,
{
    /// Executes one transaction while preserving ArbOS's typed distinction
    /// between committed receipt failures and execution errors.
    pub fn transact_one_arbitrum(
        &mut self,
        tx: ArbitrumTransaction,
    ) -> Result<
        ArbitrumExecutionOutcome,
        EVMError<<CTX::Db as Database>::Error, ArbitrumTransactionError>,
    > {
        self.0.ctx.chain_mut().set_committed_failure(None);
        self.0.ctx.set_tx(tx);
        let result = ArbitrumHandler::<
            Self,
            EVMError<<CTX::Db as Database>::Error, ArbitrumTransactionError>,
            EthFrame<EthInterpreter>,
        >::default()
        .run(self)?;
        let committed_failure = self.0.ctx.chain_mut().take_committed_failure();
        Ok(ArbitrumExecutionOutcome {
            result,
            committed_failure,
        })
    }

    /// Pops and executes the next retry scheduled by a committed transaction.
    /// Returns `Ok(None)` when the backend queue is empty.
    pub fn transact_next_scheduled_retry(&mut self) -> ScheduledRetryResult<CTX> {
        let Some(retry) = self.0.ctx.chain_mut().next_scheduled_retry() else {
            return Ok(None);
        };
        self.prepare_retry(&retry)?;
        self.0.ctx.set_tx(retry.clone().into_transaction());
        self.0
            .ctx
            .local_mut()
            .set_current_retryable(Some(retry.ticket_id));

        let result = ArbitrumHandler::<
            Self,
            EVMError<<CTX::Db as Database>::Error, ArbitrumTransactionError>,
            EthFrame<EthInterpreter>,
        >::default()
        .run(self)?;
        self.finish_retry(&retry, result.is_success())?;
        Ok(Some(result))
    }

    fn prepare_retry(
        &mut self,
        retry: &ArbitrumRetryTx,
    ) -> Result<(), EVMError<<CTX::Db as Database>::Error, ArbitrumTransactionError>> {
        let escrow_hash =
            keccak256([b"retryable escrow".as_slice(), retry.ticket_id.as_slice()].concat());
        let escrow = revm::primitives::Address::from_slice(&escrow_hash[12..]);
        if self
            .0
            .ctx
            .journal_mut()
            .transfer(escrow, retry.from, retry.value)
            .map_err(EVMError::Database)?
            .is_some()
        {
            return Err(EVMError::Transaction(
                ArbitrumTransactionError::RetryPreparationFailed,
            ));
        }
        let prepaid = retry
            .gas_fee_cap
            .saturating_mul(U256::from(retry.gas_limit));
        self.0
            .ctx
            .journal_mut()
            .balance_incr(retry.from, prepaid)
            .map_err(EVMError::Database)?;
        Ok(())
    }

    fn finish_retry(
        &mut self,
        retry: &ArbitrumRetryTx,
        success: bool,
    ) -> Result<(), EVMError<<CTX::Db as Database>::Error, ArbitrumTransactionError>> {
        if success {
            self.0
                .ctx
                .arb_state(None, false)
                .retryable(retry.ticket_id)
                .clear()
                .map_err(|_| {
                    EVMError::Transaction(ArbitrumTransactionError::RetryPreparationFailed)
                })?;
        } else {
            let escrow_hash =
                keccak256([b"retryable escrow".as_slice(), retry.ticket_id.as_slice()].concat());
            let escrow = revm::primitives::Address::from_slice(&escrow_hash[12..]);
            if self
                .0
                .ctx
                .journal_mut()
                .transfer(retry.from, escrow, retry.value)
                .map_err(EVMError::Database)?
                .is_some()
            {
                return Err(EVMError::Transaction(
                    ArbitrumTransactionError::RetryPreparationFailed,
                ));
            }
        }
        self.0.ctx.journal_mut().commit_tx();
        Ok(())
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
