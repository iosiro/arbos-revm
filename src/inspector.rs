use revm::context::{ContextSetters, JournalOutput, JournalTr};
use revm::database_interface::DatabaseCommit;
use revm::handler::instructions::EthInstructions;

use revm::handler::{
    instructions::InstructionProvider, EvmTr, EvmTrError, FrameResult, PrecompileProvider,
};
use revm::inspector::{InspectorEvmTr, InspectorFrame, InspectorHandler, JournalExt};

use revm::interpreter::{
    interpreter::EthInterpreter, FrameInput, Interpreter, InterpreterAction, InterpreterResult,
    InterpreterTypes,
};
use revm::{
    context::result::FromStringError,
    handler::{ContextTrDbError, FrameInitOrResult},
};
use revm::{InspectCommitEvm, InspectEvm, Inspector};

use crate::frame::{ArbitrumFrame, ArbitrumFrameTr};
use crate::handler::ArbitrumHandler;
use crate::{ArbitrumCfgTr, ArbitrumContextTr, ArbitrumVM};

// Implementing InspectorHandler for ArbitrumHandler.
impl<EVM, ERROR, FRAME> InspectorHandler for ArbitrumHandler<EVM, ERROR, FRAME>
where
    EVM: InspectorEvmTr<
        Context: ArbitrumContextTr<
            Journal: JournalTr<FinalOutput = JournalOutput>,
            Chain = (),
            Cfg: ArbitrumCfgTr,
        >,
        Inspector: Inspector<<EVM as EvmTr>::Context, EthInterpreter>,
    >,
    ERROR: EvmTrError<EVM>,
    FRAME: ArbitrumFrameTr<
            Evm = EVM,
            Error = ERROR,
            FrameResult = FrameResult,
            FrameInit = FrameInput,
            IT = EthInterpreter,
        > + InspectorFrame<IT = EthInterpreter>,
    <EVM as InspectorEvmTr>::Inspector: Inspector<<EVM as EvmTr>::Context>,
{
    type IT = EthInterpreter;

    #[inline]
    fn inspect_frame_call(
        &mut self,
        frame: &mut Self::Frame,
        evm: &mut Self::Evm,
    ) -> Result<FrameInitOrResult<Self::Frame>, Self::Error> {
        if let Some(next_action) = crate::stylus::run::<EVM, Self, Self::Frame>(frame, evm, self) {
            frame.process_next_action(evm, next_action)
        } else {
            frame.run_inspect(evm)
        }
    }
}

impl<CTX, INSP, PRECOMPILE> InspectEvm
    for ArbitrumVM<CTX, INSP, EthInstructions<EthInterpreter, CTX>, PRECOMPILE>
where
    CTX: ArbitrumContextTr<Journal: JournalExt> + ContextSetters,
    INSP: Inspector<CTX, EthInterpreter>,
    PRECOMPILE: PrecompileProvider<CTX, Output = InterpreterResult>,
{
    type Inspector = INSP;

    fn set_inspector(&mut self, inspector: Self::Inspector) {
        self.0.inspector = inspector;
    }

    fn inspect_replay(&mut self) -> Self::Output {
        let mut h = ArbitrumHandler::<_, _, ArbitrumFrame<_, _, _>>::new();
        h.inspect_run(self)
    }
}

impl<CTX, INSP, PRECOMPILE> InspectCommitEvm
    for ArbitrumVM<CTX, INSP, EthInstructions<EthInterpreter, CTX>, PRECOMPILE>
where
    CTX: ArbitrumContextTr<Journal: JournalExt, Db: DatabaseCommit> + ContextSetters,
    INSP: Inspector<CTX, EthInterpreter>,
    PRECOMPILE: PrecompileProvider<CTX, Output = InterpreterResult>,
{
    fn inspect_replay_commit(&mut self) -> Self::CommitOutput {
        self.inspect_replay().map(|r| {
            self.ctx().db().commit(r.state);
            r.result
        })
    }
}

impl<CTX, INSP, I, P> InspectorEvmTr for ArbitrumVM<CTX, INSP, I, P>
where
    CTX: ArbitrumContextTr<Journal: JournalExt> + ContextSetters,
    I: InstructionProvider<
        Context = CTX,
        InterpreterTypes: InterpreterTypes<Output = InterpreterAction>,
    >,
    P: PrecompileProvider<CTX>,
    INSP: Inspector<CTX, I::InterpreterTypes>,
{
    type Inspector = INSP;

    fn inspector(&mut self) -> &mut Self::Inspector {
        &mut self.0.inspector
    }

    fn ctx_inspector(&mut self) -> (&mut Self::Context, &mut Self::Inspector) {
        (&mut self.0.ctx, &mut self.0.inspector)
    }

    fn run_inspect_interpreter(
        &mut self,
        interpreter: &mut Interpreter<
            <Self::Instructions as InstructionProvider>::InterpreterTypes,
        >,
    ) -> <<Self::Instructions as InstructionProvider>::InterpreterTypes as InterpreterTypes>::Output
    {
        self.0.run_inspect_interpreter(interpreter)
    }
}

/// Impl InspectorFrame for EthFrame.
impl<EVM, ERROR> InspectorFrame for ArbitrumFrame<EVM, ERROR, EthInterpreter>
where
    EVM: EvmTr<
            Context: ArbitrumContextTr,
            Precompiles: PrecompileProvider<EVM::Context, Output = InterpreterResult>,
            Instructions: InstructionProvider<
                Context = EVM::Context,
                InterpreterTypes = EthInterpreter,
            >,
        > + InspectorEvmTr,
    ERROR: From<ContextTrDbError<EVM::Context>> + FromStringError,
{
    type IT = EthInterpreter;

    fn run_inspect(&mut self, evm: &mut Self::Evm) -> Result<FrameInitOrResult<Self>, Self::Error> {
        let interpreter = self.inner.interpreter();
        let next_action = evm.run_inspect_interpreter(interpreter);
        self.inner.process_next_action(evm, next_action)
    }

    fn interpreter(&mut self) -> &mut Interpreter<Self::IT> {
        &mut self.inner.interpreter
    }

    fn frame_input(&self) -> &FrameInput {
        &self.inner.input
    }
}
