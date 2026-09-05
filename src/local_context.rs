use std::{cell::RefCell, collections::VecDeque, rc::Rc};

use revm::{
    context::LocalContextTr,
    interpreter::CallScheme,
    primitives::{Address, B256, U256},
};

use crate::transaction::ArbitrumRetryTx;

pub trait ArbitrumLocalContextTr: LocalContextTr {
    fn stylus_pages_ever(&self) -> u16;
    fn stylus_pages_open(&self) -> u16;
    fn add_stylus_pages_open(&mut self, pages: u16);
    fn set_stylus_pages_open(&mut self, pages: u16);
    fn insert_recent_wasm(&mut self, code_hash: B256, retain: u16, block_number: u64) -> bool;
    fn enter_stylus(&mut self, address: Address) -> bool;
    fn exit_stylus(&mut self, address: Address);

    /// Get the cached L1 transaction cost (set during validation, used during reward)
    fn tx_l1_cost(&self) -> Option<U256>;
    /// Set the L1 transaction cost
    fn set_tx_l1_cost(&mut self, cost: Option<U256>);
    /// Get the cached poster gas (L1 cost converted to L2 gas units)
    fn poster_gas(&self) -> Option<u64>;
    /// Set the poster gas
    fn set_poster_gas(&mut self, gas: Option<u64>);
    fn poster_units(&self) -> Option<u64>;
    fn set_poster_units(&mut self, units: Option<u64>);
    fn held_gas(&self) -> u64;
    fn set_held_gas(&mut self, gas: u64);
    fn enter_precompile_call(&mut self, scheme: CallScheme);
    fn exit_precompile_call(&mut self);
    fn direct_call_scheme(&self) -> Option<CallScheme>;
    fn current_retryable(&self) -> Option<B256>;
    fn set_current_retryable(&mut self, ticket_id: Option<B256>);
    fn enter_frame(&mut self, caller: Address);
    fn exit_frame(&mut self);
    fn parent_frame_caller(&self) -> Option<Address>;
    fn schedule_retry_on_commit(&mut self, retry: ArbitrumRetryTx);
    fn take_scheduled_retries(&mut self) -> Vec<ArbitrumRetryTx>;
    fn filter_current_transaction(&mut self);
    fn take_filter_current_transaction(&mut self) -> bool;
}

/// Local context that is filled by execution.
#[derive(Clone, Debug)]
pub struct ArbitrumLocalContext {
    /// Interpreter shared memory buffer. A reused memory buffer for calls.
    pub shared_memory_buffer: Rc<RefCell<Vec<u8>>>,
    /// Optional precompile error message to bubble up.
    pub precompile_error_message: Option<String>,
    /// Stylus pages ever used in this transaction.
    pub stylus_pages_ever: u16,
    /// Stylus pages currently open.
    pub stylus_pages_open: u16,
    /// Recently invoked Stylus wasm code hashes (block-local LRU).
    pub recent_wasms: VecDeque<B256>,
    pub recent_wasms_block_number: Option<u64>,
    /// Addresses of currently executing Stylus frames, used for EVM API metadata.
    pub active_stylus_addresses: Vec<Address>,
    /// Cached L1 transaction cost (set during validation, cleared after execution)
    pub tx_l1_cost: Option<U256>,
    /// Cached poster gas (L1 cost converted to L2 gas units)
    pub poster_gas: Option<u64>,
    /// Brotli/data units charged to the transaction and awaiting a batch report.
    pub poster_units: Option<u64>,
    /// Compute gas held outside the first frame by ArbOS's transaction cap.
    pub held_gas: u64,
    /// Direct opcode scheme for nested precompile invocations. Unlike
    /// `is_static`, this does not inherit a static ancestor's read-only flag.
    pub precompile_call_schemes: Vec<CallScheme>,
    /// Ticket currently executing as a scheduled retry transaction.
    pub current_retryable: Option<B256>,
    /// Caller metadata for live EVM frames, maintained by `ArbitrumEvm`.
    pub frame_callers: Vec<Address>,
    /// Retry transactions staged by this transaction. The handler moves these
    /// into chain state only after successful journal commit.
    pub scheduled_retries_on_commit: Vec<ArbitrumRetryTx>,
    /// Backend-visible equivalent of Nitro's `StateDB.FilterTx` side effect.
    pub filter_current_transaction: bool,
}

impl Default for ArbitrumLocalContext {
    fn default() -> Self {
        Self {
            shared_memory_buffer: Rc::new(RefCell::new(Vec::with_capacity(1024 * 4))),
            precompile_error_message: None,
            stylus_pages_ever: 0,
            stylus_pages_open: 0,
            recent_wasms: VecDeque::new(),
            recent_wasms_block_number: None,
            active_stylus_addresses: Vec::new(),
            tx_l1_cost: None,
            poster_gas: None,
            poster_units: None,
            held_gas: 0,
            precompile_call_schemes: Vec::new(),
            current_retryable: None,
            frame_callers: Vec::new(),
            scheduled_retries_on_commit: Vec::new(),
            filter_current_transaction: false,
        }
    }
}

impl LocalContextTr for ArbitrumLocalContext {
    fn clear(&mut self) {
        // Sets len to 0 but it will not shrink to drop the capacity.
        unsafe { self.shared_memory_buffer.borrow_mut().set_len(0) };

        // Clear L1 fee related fields
        self.tx_l1_cost = None;
        self.poster_gas = None;
        self.poster_units = None;
        self.held_gas = 0;
        self.stylus_pages_open = 0;
        self.stylus_pages_ever = 0;
        self.active_stylus_addresses.clear();
        self.precompile_call_schemes.clear();
        self.current_retryable = None;
        self.frame_callers.clear();
        self.scheduled_retries_on_commit.clear();
        self.filter_current_transaction = false;
    }

    fn shared_memory_buffer(&self) -> &Rc<RefCell<Vec<u8>>> {
        &self.shared_memory_buffer
    }

    fn set_precompile_error_context(&mut self, output: String) {
        self.precompile_error_message = Some(output);
    }

    fn take_precompile_error_context(&mut self) -> Option<String> {
        self.precompile_error_message.take()
    }
}

impl ArbitrumLocalContextTr for ArbitrumLocalContext {
    fn stylus_pages_ever(&self) -> u16 {
        self.stylus_pages_ever
    }

    fn stylus_pages_open(&self) -> u16 {
        self.stylus_pages_open
    }

    fn add_stylus_pages_open(&mut self, pages: u16) {
        self.stylus_pages_open = self.stylus_pages_open.saturating_add(pages);
        if self.stylus_pages_open > self.stylus_pages_ever {
            self.stylus_pages_ever = self.stylus_pages_open;
        }
    }

    fn set_stylus_pages_open(&mut self, pages: u16) {
        self.stylus_pages_open = pages;
        if self.stylus_pages_open > self.stylus_pages_ever {
            self.stylus_pages_ever = self.stylus_pages_open;
        }
    }

    fn insert_recent_wasm(&mut self, code_hash: B256, retain: u16, block_number: u64) -> bool {
        if self.recent_wasms_block_number != Some(block_number) {
            self.recent_wasms.clear();
            self.recent_wasms_block_number = Some(block_number);
        }
        if let Some(pos) = self
            .recent_wasms
            .iter()
            .position(|existing| *existing == code_hash)
        {
            // Move existing entry to the back to track recency.
            if pos + 1 != self.recent_wasms.len()
                && let Some(found) = self.recent_wasms.remove(pos)
            {
                self.recent_wasms.push_back(found);
            }
            return true;
        }

        self.recent_wasms.push_back(code_hash);

        let retain = retain as usize;
        if retain > 0 && self.recent_wasms.len() > retain {
            self.recent_wasms.pop_front();
        }

        false
    }

    fn enter_stylus(&mut self, address: Address) -> bool {
        let reentrant = self.active_stylus_addresses.contains(&address);
        self.active_stylus_addresses.push(address);
        reentrant
    }

    fn exit_stylus(&mut self, address: Address) {
        debug_assert_eq!(self.active_stylus_addresses.last(), Some(&address));
        self.active_stylus_addresses.pop();
    }

    fn tx_l1_cost(&self) -> Option<U256> {
        self.tx_l1_cost
    }

    fn set_tx_l1_cost(&mut self, cost: Option<U256>) {
        self.tx_l1_cost = cost;
    }

    fn poster_gas(&self) -> Option<u64> {
        self.poster_gas
    }

    fn set_poster_gas(&mut self, gas: Option<u64>) {
        self.poster_gas = gas;
    }

    fn poster_units(&self) -> Option<u64> {
        self.poster_units
    }

    fn set_poster_units(&mut self, units: Option<u64>) {
        self.poster_units = units;
    }

    fn held_gas(&self) -> u64 {
        self.held_gas
    }

    fn set_held_gas(&mut self, gas: u64) {
        self.held_gas = gas;
    }

    fn enter_precompile_call(&mut self, scheme: CallScheme) {
        self.precompile_call_schemes.push(scheme);
    }

    fn exit_precompile_call(&mut self) {
        self.precompile_call_schemes.pop();
    }

    fn direct_call_scheme(&self) -> Option<CallScheme> {
        self.precompile_call_schemes.last().copied()
    }

    fn current_retryable(&self) -> Option<B256> {
        self.current_retryable
    }

    fn set_current_retryable(&mut self, ticket_id: Option<B256>) {
        self.current_retryable = ticket_id;
    }

    fn enter_frame(&mut self, caller: Address) {
        self.frame_callers.push(caller);
    }

    fn exit_frame(&mut self) {
        self.frame_callers.pop();
    }

    fn parent_frame_caller(&self) -> Option<Address> {
        self.frame_callers
            .len()
            .checked_sub(2)
            .and_then(|index| self.frame_callers.get(index))
            .copied()
    }

    fn schedule_retry_on_commit(&mut self, retry: ArbitrumRetryTx) {
        self.scheduled_retries_on_commit.push(retry);
    }

    fn take_scheduled_retries(&mut self) -> Vec<ArbitrumRetryTx> {
        std::mem::take(&mut self.scheduled_retries_on_commit)
    }

    fn filter_current_transaction(&mut self) {
        self.filter_current_transaction = true;
    }

    fn take_filter_current_transaction(&mut self) -> bool {
        std::mem::take(&mut self.filter_current_transaction)
    }
}

#[cfg(test)]
mod reentrancy_tests {
    use super::*;

    #[test]
    fn stylus_reentrancy_is_address_specific_and_stack_scoped() {
        let mut local = ArbitrumLocalContext::default();
        let first = Address::repeat_byte(1);
        let second = Address::repeat_byte(2);
        assert!(!local.enter_stylus(first));
        assert!(!local.enter_stylus(second));
        local.exit_stylus(second);
        assert!(local.enter_stylus(first));
        local.exit_stylus(first);
        local.exit_stylus(first);
        assert!(!local.enter_stylus(first));
    }
}

impl ArbitrumLocalContext {
    /// Creates a new local context, initcodes are hashes and added to the mapping.
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transaction_clear_resets_pages_but_recent_cache_is_block_scoped() {
        let mut local = ArbitrumLocalContext::default();
        let hash = B256::repeat_byte(0x11);
        local.add_stylus_pages_open(12);
        assert!(!local.insert_recent_wasm(hash, 4, 7));
        assert!(local.insert_recent_wasm(hash, 4, 7));

        local.clear();
        assert_eq!(local.stylus_pages_open(), 0);
        assert_eq!(local.stylus_pages_ever(), 0);
        assert!(local.insert_recent_wasm(hash, 4, 7));
        assert!(!local.insert_recent_wasm(hash, 4, 8));
    }
}
