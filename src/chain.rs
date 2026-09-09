use std::collections::VecDeque;

use revm::primitives::B256;

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
    /// Verified RPC parent hash, distinct from the L1 hashes exposed by BLOCKHASH backends.
    #[cfg_attr(feature = "serde", serde(default))]
    rpc_parent_block_hash: Option<B256>,
    /// Consensus-priced recent program accesses, shared by transactions in a block.
    #[cfg_attr(feature = "serde", serde(default))]
    recent_wasms: VecDeque<B256>,
    /// Nitro fixes the capacity on the first program access in each block.
    #[cfg_attr(feature = "serde", serde(default))]
    recent_wasms_capacity: Option<u16>,
}

pub trait ArbitrumChainTr {
    fn rpc_block_number(&self) -> Option<u64>;
    fn rpc_parent_block_hash(&self) -> Option<B256>;
    fn schedule_retry(&mut self, retry: ArbitrumRetryTx);
    fn next_scheduled_retry(&mut self) -> Option<ArbitrumRetryTx>;
    fn scheduled_retries(&self) -> &VecDeque<ArbitrumRetryTx>;
    fn set_committed_failure(&mut self, failure: Option<ArbitrumCommittedFailure>);
    fn take_committed_failure(&mut self) -> Option<ArbitrumCommittedFailure>;
    fn begin_block(&mut self, number: u64);
    fn block_gas_used(&self) -> u64;
    fn record_block_gas(&mut self, gas: u64);
    /// Records a program access, returning whether it was already in the block's LRU.
    /// Accesses survive transaction cleanup and frame reverts, but not block changes.
    fn insert_recent_wasm(&mut self, code_hash: B256, retain: u16, block_number: u64) -> bool;
}

impl ArbitrumChainTr for ArbitrumChain {
    fn rpc_block_number(&self) -> Option<u64> {
        self.rpc_block_number
    }

    fn rpc_parent_block_hash(&self) -> Option<B256> {
        self.rpc_parent_block_hash
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
            self.recent_wasms.clear();
            self.recent_wasms_capacity = None;
        }
    }

    fn block_gas_used(&self) -> u64 {
        self.block_gas_used
    }

    fn record_block_gas(&mut self, gas: u64) {
        self.block_gas_used = self.block_gas_used.saturating_add(gas);
    }

    fn insert_recent_wasm(&mut self, code_hash: B256, retain: u16, block_number: u64) -> bool {
        self.begin_block(block_number);
        // Nitro's BasicLRU clamps zero capacity to one and never resizes an existing cache.
        let capacity = *self.recent_wasms_capacity.get_or_insert(retain.max(1));
        if let Some(pos) = self.recent_wasms.iter().position(|hash| *hash == code_hash) {
            self.recent_wasms.remove(pos);
            self.recent_wasms.push_back(code_hash);
            return true;
        }
        if self.recent_wasms.len() == usize::from(capacity) {
            self.recent_wasms.pop_front();
        }
        self.recent_wasms.push_back(code_hash);
        false
    }
}

impl ArbitrumChain {
    pub fn set_rpc_block_number(&mut self, number: Option<u64>) {
        if self.rpc_block_number != number {
            self.rpc_parent_block_hash = None;
        }
        self.rpc_block_number = number;
    }

    pub fn set_rpc_block(&mut self, number: u64, parent_hash: B256) {
        self.set_rpc_block_number(Some(number));
        self.rpc_parent_block_hash = Some(parent_hash);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpc_parent_hash_follows_the_exact_block_context() {
        let mut chain = ArbitrumChain::default();
        let parent = B256::repeat_byte(0x42);
        chain.set_rpc_block(7, parent);
        chain.set_rpc_block_number(Some(7));
        assert_eq!(chain.rpc_parent_block_hash(), Some(parent));
        let snapshot = chain.clone();
        chain.set_rpc_block_number(Some(8));
        assert_eq!(chain.rpc_parent_block_hash(), None);
        assert_eq!(snapshot.rpc_parent_block_hash(), Some(parent));
        chain.set_rpc_block(7, B256::repeat_byte(0x99));
        assert_eq!(chain.rpc_parent_block_hash(), Some(B256::repeat_byte(0x99)));
        chain.set_rpc_block_number(None);
        assert_eq!(chain.rpc_parent_block_hash(), None);
    }

    #[test]
    fn recent_wasm_zero_capacity_matches_nitro() {
        let mut chain = ArbitrumChain::default();
        let a = B256::repeat_byte(1);
        let b = B256::repeat_byte(2);
        assert!(!chain.insert_recent_wasm(a, 0, 7));
        assert!(chain.insert_recent_wasm(a, 0, 7));
        assert!(!chain.insert_recent_wasm(b, 0, 7));
        assert!(!chain.insert_recent_wasm(a, 0, 7));
    }

    #[test]
    fn recent_wasm_capacity_and_recency_are_block_scoped() {
        let mut chain = ArbitrumChain::default();
        let a = B256::repeat_byte(1);
        let b = B256::repeat_byte(2);
        let c = B256::repeat_byte(3);
        assert!(!chain.insert_recent_wasm(a, 2, 7));
        assert!(!chain.insert_recent_wasm(b, 1, 7));
        assert!(chain.insert_recent_wasm(a, 1, 7));
        assert!(!chain.insert_recent_wasm(c, 1, 7));
        assert!(chain.insert_recent_wasm(a, 1, 7));
        assert!(!chain.insert_recent_wasm(b, 1, 7));

        let mut nested = chain.clone();
        assert!(nested.insert_recent_wasm(b, 1, 7));
        assert!(!nested.insert_recent_wasm(c, 1, 7));
        assert!(
            chain.insert_recent_wasm(a, 1, 7),
            "clones must be independent"
        );

        assert!(!chain.insert_recent_wasm(a, 1, 8));
        assert!(!chain.insert_recent_wasm(b, 2, 8));
        assert!(!chain.insert_recent_wasm(a, 2, 8));
    }
}
