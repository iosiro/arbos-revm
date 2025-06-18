use crate::{frame::ArbitrumFrameTr, ArbitrumCfgTr, ArbitrumContextTr, ArbitrumHaltReason, ArbitrumLocalContextTr};
use revm::{
    context::{JournalOutput, JournalTr},
    handler::{handler::EvmTrError, EvmTr, Frame, FrameInitOrResult, FrameResult, Handler},
    inspector::{InspectorEvmTr, InspectorFrame},
    interpreter::{interpreter::EthInterpreter, FrameInput},
    Inspector,
};

pub struct ArbitrumHandler<EVM, ERROR, FRAME> {
    pub _phantom: core::marker::PhantomData<(EVM, ERROR, FRAME)>,
}

impl<EVM, ERROR, FRAME> ArbitrumHandler<EVM, ERROR, FRAME> {
    pub fn new() -> Self {
        Self {
            _phantom: core::marker::PhantomData,
        }
    }
}

impl<EVM, ERROR, FRAME> Default for ArbitrumHandler<EVM, ERROR, FRAME> {
    fn default() -> Self {
        Self::new()
    }
}

impl<EVM, ERROR, FRAME> Handler for ArbitrumHandler<EVM, ERROR, FRAME>
where
    EVM: InspectorEvmTr<
        Context: ArbitrumContextTr<
            Journal: JournalTr<FinalOutput = JournalOutput>,
            Chain = (),
            Cfg: ArbitrumCfgTr,
            Local: ArbitrumLocalContextTr,
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
    type Evm = EVM;
    type Error = ERROR;
    type Frame = FRAME;
    type HaltReason = ArbitrumHaltReason;

    #[inline]
    fn frame_call(
        &mut self,
        frame: &mut Self::Frame,
        evm: &mut Self::Evm,
    ) -> Result<FrameInitOrResult<Self::Frame>, Self::Error> {
        if let Some(next_action) = crate::stylus::run(frame, evm, self) {
            frame.process_next_action(evm, next_action)
        } else {
            Frame::run(frame, evm)
        }
    }
}
// impl<EVM, ERROR, FRAME> Handler for ArbitrumHandler<EVM, ERROR, FRAME>
// where
//     EVM: InspectorEvmTr<Context: ArbitrumContextTr, Inspector: Inspector<<EVM as EvmTr>::Context, EthInterpreter>>,
//     ERROR: EvmTrError<EVM> + FromStringError,
//     // TODO `FrameResult` should be a generic trait.
//     // TODO `FrameInit` should be a generic.
//     FRAME: ArbitrumFrameTr<
//         Evm = EVM,
//         Error = ERROR,
//         FrameResult = FrameResult,
//         FrameInit = FrameInput,
//     > + InspectorFrame<IT = EthInterpreter>,
//     EVM::Precompiles: PrecompileProvider<EVM::Context>,
// {
//     type Evm = EVM;
//     type Error = ERROR;
//     type Frame = FRAME;
//     type HaltReason = ArbitrumHaltReason;

//     #[inline]
//     fn frame_call(
//         &mut self,
//         frame: &mut Self::Frame,
//         evm: &mut Self::Evm,
//     ) -> Result<FrameInitOrResult<Self::Frame>, Self::Error> {
//         if let Some(next_action) = crate::stylus::run(frame.interpreter(), evm, self) {
//            frame.process_next_action(evm, next_action)
//         } else {
//             Frame::run(frame, evm)
//         }
//     }
// }
