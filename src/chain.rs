//! Block-scoped Arbitrum execution state.

use revm::primitives::B256;
use std::collections::{BTreeMap, VecDeque};

/// State shared by transactions in a block, copied with fork and snapshot state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ArbitrumChain {
    /// RPC/L2 height, independent of EVM's L1 NUMBER value on forks.
    pub rpc_block_number: Option<u64>,
    /// Whether ArbOS state is initialized locally or expected from the selected fork.
    pub arbos_initialized: bool,
    /// Optional embedding overrides for L1 BLOCKHASH, never written to L2 history.
    pub block_hash_overrides: BTreeMap<u64, B256>,
    /// Enables deterministic empty-database hashes for locally rolled development blocks.
    pub synthetic_block_hashes: bool,
    recent_wasms: VecDeque<B256>,
    recent_wasms_block: Option<u64>,
    recent_wasms_capacity: usize,
}

impl ArbitrumChain {
    /// Records a program access and returns whether it was already cached this block.
    pub fn insert_recent_wasm(&mut self, hash: B256, retain: u16, block: u64) -> bool {
        if self.recent_wasms_block != Some(block) {
            self.recent_wasms.clear();
            self.recent_wasms_block = Some(block);
            self.recent_wasms_capacity = usize::from(retain).max(1);
        }
        if let Some(index) = self.recent_wasms.iter().position(|entry| *entry == hash) {
            self.recent_wasms.remove(index);
            self.recent_wasms.push_back(hash);
            return true;
        }
        self.recent_wasms.push_back(hash);
        if self.recent_wasms.len() > self.recent_wasms_capacity {
            self.recent_wasms.pop_front();
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn recent_wasm_capacity_and_clones_are_block_scoped() {
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
        let mut copy = chain.clone();
        assert!(copy.insert_recent_wasm(b, 1, 7));
        assert!(!copy.insert_recent_wasm(c, 1, 7));
        assert!(chain.insert_recent_wasm(a, 1, 7));
        assert!(!chain.insert_recent_wasm(a, 1, 8));
        assert!(!chain.insert_recent_wasm(b, 2, 8));
        assert!(!chain.insert_recent_wasm(a, 2, 8));
    }
}
