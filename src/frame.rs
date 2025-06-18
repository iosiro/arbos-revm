use revm::{
    context::{result::FromStringError, ContextTr},
    handler::{
        instructions::InstructionProvider, ContextTrDbError, EthFrame, EvmTr, Frame,
        FrameInitOrResult, FrameResult, PrecompileProvider,
    },
    interpreter::{
        interpreter::EthInterpreter, FrameInput, InterpreterAction, InterpreterResult,
        InterpreterTypes,
    },
};

/// Traits that extends the Frame with additional functionality that is needed to run the Arbitrum VM
///
/// It is implemented for [`EthFrame`] as default Ethereum frame implementation.
pub trait ArbitrumFrameTr: Frame {
    type IT: InterpreterTypes;

    /// Returns a reference to the frame input. Frame input is needed for call/create/eofcreate [`crate::Inspector`] methods
    fn process_next_action(
        &mut self,
        evm: &mut Self::Evm,
        next_action: InterpreterAction,
    ) -> Result<FrameInitOrResult<Self>, Self::Error>;
}

pub struct ArbitrumFrame<EVM, ERROR, IW: InterpreterTypes> {
    pub inner: EthFrame<EVM, ERROR, IW>,
}

impl<EVM, ERROR, IW> ArbitrumFrame<EVM, ERROR, IW>
where
    IW: InterpreterTypes,
{
    pub fn new(inner: EthFrame<EVM, ERROR, IW>) -> Self {
        Self { inner }
    }
}

impl<EVM, ERROR> ArbitrumFrameTr for ArbitrumFrame<EVM, ERROR, EthInterpreter>
where
    EVM: EvmTr<
        Context: ContextTr,
        Precompiles: PrecompileProvider<EVM::Context, Output = InterpreterResult>,
        Instructions: InstructionProvider<
            Context = EVM::Context,
            InterpreterTypes = EthInterpreter,
        >,
    >,
    ERROR: From<ContextTrDbError<EVM::Context>> + FromStringError,
{
    type IT = EthInterpreter;

    fn process_next_action(
        &mut self,
        evm: &mut EVM,
        next_action: InterpreterAction,
    ) -> Result<FrameInitOrResult<Self>, ERROR> {
        self.inner.process_next_action(evm, next_action)
    }
}

impl<EVM, ERROR> Frame for ArbitrumFrame<EVM, ERROR, EthInterpreter>
where
    EVM: EvmTr<
        Precompiles: PrecompileProvider<EVM::Context, Output = InterpreterResult>,
        Instructions: InstructionProvider<
            Context = EVM::Context,
            InterpreterTypes = EthInterpreter,
        >,
    >,
    ERROR: From<ContextTrDbError<EVM::Context>> + FromStringError,
{
    type Evm = EVM;
    type FrameInit = FrameInput;
    type FrameResult = FrameResult;
    type Error = ERROR;

    fn return_result(
        &mut self,
        evm: &mut Self::Evm,
        result: Self::FrameResult,
    ) -> Result<(), Self::Error> {
        self.inner.return_result(evm, result)
    }

    fn init_first(
        evm: &mut Self::Evm,
        frame_input: Self::FrameInit,
    ) -> Result<revm::handler::FrameOrResult<Self>, Self::Error> {
        EthFrame::init_first(evm, frame_input).map(|frame_or_result| match frame_or_result {
            revm::handler::FrameOrResult::Item(inner_frame) => {
                revm::handler::FrameOrResult::Item(Self { inner: inner_frame })
            }
            revm::handler::FrameOrResult::Result(result) => {
                revm::handler::FrameOrResult::Result(result)
            }
        })
    }

    fn init(
        &mut self,
        evm: &mut Self::Evm,
        frame_input: Self::FrameInit,
    ) -> Result<revm::handler::FrameOrResult<Self>, Self::Error> {
        self.inner
            .init(evm, frame_input)
            .map(|frame_or_result| match frame_or_result {
                revm::handler::FrameOrResult::Item(inner_frame) => {
                    revm::handler::FrameOrResult::Item(Self { inner: inner_frame })
                }
                revm::handler::FrameOrResult::Result(result) => {
                    revm::handler::FrameOrResult::Result(result)
                }
            })
    }

    fn run(
        &mut self,
        evm: &mut Self::Evm,
    ) -> Result<revm::handler::FrameInitOrResult<Self>, Self::Error> {
        self.inner.run(evm)
    }
}
