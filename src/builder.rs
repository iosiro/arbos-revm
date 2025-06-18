use crate::{evm::ArbitrumVM, precompiles::ArbitrumPrecompiles, ArbitrumCfgTr, ArbitrumSpecId};
use revm::{
    context::{Evm, JournalOutput, Transaction},
    context_interface::{Block, JournalTr},
    handler::instructions::EthInstructions,
    inspector::{JournalExt, NoOpInspector},
    interpreter::interpreter::EthInterpreter,
    Context, Database,
};

/// Trait that allows for optimism OpEvm to be built.
pub trait ArbitrumBuilder: Sized {
    /// Type of the context.
    type Context;

    /// Build the arbitrum.
    fn build_arbitrum(self) -> ArbitrumVM<Self::Context, NoOpInspector>;

    /// Build the arbitrum with an inspector.
    fn build_arbitrum_with_inspector<INSP>(
        self,
        inspector: INSP,
    ) -> ArbitrumVM<Self::Context, INSP, EthInstructions<EthInterpreter, Self::Context>>;
}

impl<BLOCK, TX, CFG, DB, JOURNAL> ArbitrumBuilder for Context<BLOCK, TX, CFG, DB, JOURNAL>
where
    BLOCK: Block,
    TX: Transaction,
    CFG: ArbitrumCfgTr<Spec = ArbitrumSpecId>,
    DB: Database,
    JOURNAL: JournalTr<Database = DB, FinalOutput = JournalOutput> + JournalExt,
{
    type Context = Self;

    fn build_arbitrum(
        self,
    ) -> ArbitrumVM<Self::Context, NoOpInspector, EthInstructions<EthInterpreter, Self::Context>>
    {
        ArbitrumVM {
            0: Evm {
                ctx: self,
                inspector: NoOpInspector::default(),
                instruction: EthInstructions::default(),
                precompiles: ArbitrumPrecompiles::default(),
            },
        }
    }

    fn build_arbitrum_with_inspector<INSP>(
        self,
        inspector: INSP,
    ) -> ArbitrumVM<Self::Context, INSP, EthInstructions<EthInterpreter, Self::Context>> {
        ArbitrumVM {
            0: Evm {
                ctx: self,
                inspector,
                instruction: EthInstructions::default(),
                precompiles: ArbitrumPrecompiles::default(),
            },
        }
    }
}
