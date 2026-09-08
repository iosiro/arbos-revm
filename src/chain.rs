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
    /// RPC L2 height when the execution block environment exposes an L1 height.
    #[cfg_attr(feature = "serde", serde(default))]
    rpc_block_number: Option<u64>,
}

pub trait ArbitrumChainTr {
    fn rpc_block_number(&self) -> Option<u64>;
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
    fn rpc_block_number(&self) -> Option<u64> {
        self.rpc_block_number
    }

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
    pub fn set_rpc_block_number(&mut self, number: Option<u64>) {
        self.rpc_block_number = number;
    }
}
