use std::collections::VecDeque;

use crate::{result::ArbitrumCommittedFailure, transaction::ArbitrumRetryTx};

/// Block/backend-owned Arbitrum state that survives transaction-local cleanup.
///
/// Scheduled retries are appended only after the transaction that created them
/// commits, so reverted calls cannot leak work into the backend queue.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ArbitrumChain {
    scheduled_retries: VecDeque<ArbitrumRetryTx>,
    committed_failure: Option<ArbitrumCommittedFailure>,
    block_number: Option<u64>,
    block_gas_used: u64,
    debug_mode: bool,
    disable_auto_cache: bool,
    disable_auto_activate: bool,
}

pub trait ArbitrumChainTr {
    fn schedule_retry(&mut self, retry: ArbitrumRetryTx);
    fn next_scheduled_retry(&mut self) -> Option<ArbitrumRetryTx>;
    fn scheduled_retries(&self) -> &VecDeque<ArbitrumRetryTx>;
    fn set_committed_failure(&mut self, failure: Option<ArbitrumCommittedFailure>);
    fn take_committed_failure(&mut self) -> Option<ArbitrumCommittedFailure>;
    fn begin_block(&mut self, number: u64);
    fn block_gas_used(&self) -> u64;
    fn record_block_gas(&mut self, gas: u64);
}

impl ArbitrumChainTr for ArbitrumChain {
    fn schedule_retry(&mut self, retry: ArbitrumRetryTx) {
        self.scheduled_retries.push_back(retry);
    }

    fn next_scheduled_retry(&mut self) -> Option<ArbitrumRetryTx> {
        self.scheduled_retries.pop_front()
    }

    fn scheduled_retries(&self) -> &VecDeque<ArbitrumRetryTx> {
        &self.scheduled_retries
    }

    fn set_committed_failure(&mut self, failure: Option<ArbitrumCommittedFailure>) {
        self.committed_failure = failure;
    }

    fn take_committed_failure(&mut self) -> Option<ArbitrumCommittedFailure> {
        self.committed_failure.take()
    }

    fn begin_block(&mut self, number: u64) {
        if self.block_number != Some(number) {
            self.block_number = Some(number);
            self.block_gas_used = 0;
        }
    }

    fn block_gas_used(&self) -> u64 {
        self.block_gas_used
    }

    fn record_block_gas(&mut self, gas: u64) {
        self.block_gas_used = self.block_gas_used.saturating_add(gas);
    }
}

impl ArbitrumChain {
    pub fn configure_execution(
        &mut self,
        debug_mode: bool,
        disable_auto_cache: bool,
        disable_auto_activate: bool,
    ) {
        self.debug_mode = debug_mode;
        self.disable_auto_cache = disable_auto_cache;
        self.disable_auto_activate = disable_auto_activate;
    }

    pub const fn debug_mode(&self) -> bool {
        self.debug_mode
    }

    pub const fn disable_auto_cache(&self) -> bool {
        self.disable_auto_cache
    }

    pub const fn disable_auto_activate(&self) -> bool {
        self.disable_auto_activate
    }
}
