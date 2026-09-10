use revm::{
    Context, Database, Journal,
    context::{Block, BlockEnv, ContextTr, JournalTr},
};

use crate::{
    chain::ArbitrumChain,
    config::{ArbitrumConfig, ArbitrumConfigTr},
    local_context::{ArbitrumLocalContext, ArbitrumLocalContextTr},
    transaction::{ArbitrumTransaction, ArbitrumTxTr},
};

pub type ArbitrumChainInfo = ArbitrumChain;

/// Type alias for the default context type of the ArbitrumEvm.
pub type ArbitrumContext<DB> = Context<
    BlockEnv,
    ArbitrumTransaction,
    ArbitrumConfig,
    DB,
    Journal<DB>,
    ArbitrumChainInfo,
    ArbitrumLocalContext,
>;

/// Type alias for Arbitrum context
pub trait ArbitrumContextTr:
    ContextTr<
        Cfg: ArbitrumConfigTr,
        Tx: ArbitrumTxTr,
        Local: ArbitrumLocalContextTr,
        Chain = ArbitrumChain,
    >
{
    /// Returns the L2 height without changing EVM NUMBER.
    fn arb_block_number(&self) -> revm::primitives::U256 {
        self.chain()
            .rpc_block_number
            .map(revm::primitives::U256::from)
            .unwrap_or_else(|| self.block_number())
    }
}

impl<T> ArbitrumContextTr for T where
    T: ContextTr<
            Cfg: ArbitrumConfigTr,
            Tx: ArbitrumTxTr,
            Local: ArbitrumLocalContextTr,
            Chain = ArbitrumChain,
        >
{
}

/// Mutable extension used by the handler to keep the live execution config in
/// sync with the version persisted by a start-block upgrade.
pub trait ArbitrumContextMutTr: ArbitrumContextTr {
    fn set_live_arbos_version(&mut self, version: u64);
    fn drop_transaction_tip(&mut self, base_fee: u128);
}

impl<BLOCK, TX, CFG, DB, JOURNAL, LOCAL> ArbitrumContextMutTr
    for Context<BLOCK, TX, CFG, DB, JOURNAL, ArbitrumChain, LOCAL>
where
    BLOCK: Block,
    TX: ArbitrumTxTr,
    CFG: ArbitrumConfigTr,
    DB: Database,
    JOURNAL: JournalTr<Database = DB>,
    LOCAL: ArbitrumLocalContextTr,
{
    fn set_live_arbos_version(&mut self, version: u64) {
        self.cfg.set_arbos_version(version);
        self.journaled_state.set_spec_id(self.cfg.spec().into());
        self.chain.arbos_initialized = version != 0;
    }

    fn drop_transaction_tip(&mut self, base_fee: u128) {
        self.tx.drop_tip(base_fee);
    }
}
