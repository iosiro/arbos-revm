use revm::{
    Context, Database, Journal,
    context::{Block, BlockEnv, ContextTr, JournalTr},
};

use crate::{
    chain::{ArbitrumChain, ArbitrumChainTr},
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
        Chain: ArbitrumChainTr,
        Local: ArbitrumLocalContextTr,
    >
{
}

impl<T> ArbitrumContextTr for T where
    T: ContextTr<
            Cfg: ArbitrumConfigTr,
            Tx: ArbitrumTxTr,
            Chain: ArbitrumChainTr,
            Local: ArbitrumLocalContextTr,
        >
{
}

/// Mutable extension used by the handler to keep the live execution config in
/// sync with the version persisted by a start-block upgrade.
pub trait ArbitrumContextMutTr: ArbitrumContextTr {
    fn set_live_arbos_version(&mut self, version: u64);
    fn drop_transaction_tip(&mut self, base_fee: u128);
}

impl<BLOCK, TX, CFG, DB, JOURNAL, CHAIN, LOCAL> ArbitrumContextMutTr
    for Context<BLOCK, TX, CFG, DB, JOURNAL, CHAIN, LOCAL>
where
    BLOCK: Block,
    TX: ArbitrumTxTr,
    CFG: ArbitrumConfigTr,
    DB: Database,
    JOURNAL: JournalTr<Database = DB>,
    CHAIN: ArbitrumChainTr,
    LOCAL: ArbitrumLocalContextTr,
{
    fn set_live_arbos_version(&mut self, version: u64) {
        self.cfg.set_arbos_version(version);
    }

    fn drop_transaction_tip(&mut self, base_fee: u128) {
        self.tx.drop_tip(base_fee);
    }
}
