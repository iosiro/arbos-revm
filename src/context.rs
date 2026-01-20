use revm::{
    Context, Journal,
    context::{BlockEnv, ContextTr},
};

use crate::{
    config::{ArbitrumConfig, ArbitrumConfigTr},
    local_context::{ArbitrumLocalContext, ArbitrumLocalContextTr},
    transaction::{ArbitrumTransaction, ArbitrumTxTr},
};

pub type ArbitrumChainInfo = ();

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
    ContextTr<Cfg: ArbitrumConfigTr, Tx: ArbitrumTxTr, Local: ArbitrumLocalContextTr>
{
}

impl<T> ArbitrumContextTr for T where
    T: ContextTr<Cfg: ArbitrumConfigTr, Tx: ArbitrumTxTr, Local: ArbitrumLocalContextTr>
{
}
