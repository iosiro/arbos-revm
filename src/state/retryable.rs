use revm::primitives::{Address, B256, Bytes, U256};

use crate::{
    ArbitrumContextTr,
    constants::RETRYABLE_LIFETIME_SECONDS,
    state::types::{
        ArbosStateError, StorageBackedAddress, StorageBackedBytes, StorageBackedQueue,
        StorageBackedTr, StorageBackedU64, StorageBackedU256, map_address, substorage,
    },
};

const ARBOS_STATE_RETRYABLE_TIMEOUT_QUEUE_KEY: &[u8] = &[0];
const ARBOS_STATE_RETRYABLE_CALLDATA_KEY: &[u8] = &[1];

pub struct RetryableState<'a, CTX>
where
    CTX: ArbitrumContextTr,
{
    context: &'a mut CTX,
    gas: Option<&'a mut revm::interpreter::Gas>,
    pub is_static: bool,
    slot: B256,
}

impl<'a, CTX: ArbitrumContextTr> RetryableState<'a, CTX> {
    pub fn new(
        context: &'a mut CTX,
        gas: Option<&'a mut revm::interpreter::Gas>,
        is_static: bool,
        subkey: B256,
    ) -> Self {
        Self {
            context,
            gas,
            is_static,
            slot: subkey,
        }
    }

    pub fn timeout_queue(&mut self) -> StorageBackedQueue<'_, CTX> {
        let slot = substorage(&self.slot, ARBOS_STATE_RETRYABLE_TIMEOUT_QUEUE_KEY);
        StorageBackedQueue::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }

    pub fn retryable(&mut self, id: B256) -> Retryable<'_, CTX> {
        let slot = substorage(&self.slot, id.as_slice());
        Retryable::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }

    /// Attempts to reap one expired retryable ticket from the timeout queue.
    ///
    /// If the queue head is already deleted (timeout == 0), it is silently
    /// discarded. If the ticket has expired and has no remaining timeout
    /// windows, it is cleared. Otherwise it is re-enqueued with an extended
    /// timeout.
    ///
    /// Matches nitro `retryable.go:293-333`. Escrow ETH transfers are
    /// omitted — this is a testing tool.
    pub fn try_to_reap_one_retryable(
        &mut self,
        current_timestamp: u64,
    ) -> Result<(), ArbosStateError> {
        // Peek at queue head
        let id = match self.timeout_queue().peek()? {
            None => return Ok(()),
            Some(id) => B256::from(id),
        };

        let timeout = {
            let mut retryable = self.retryable(id);
            retryable.timeout().get()?
        };

        if timeout == 0 {
            // Already deleted, discard from queue
            self.timeout_queue().pop()?;
            return Ok(());
        }
        if current_timestamp < timeout {
            return Ok(()); // Not expired yet
        }

        // Pop from queue — we will either delete or re-enqueue
        self.timeout_queue().pop()?;

        let windows_left = {
            let mut retryable = self.retryable(id);
            retryable.timeout_windows_left().get()?
        };

        if windows_left == 0 {
            // Expired, delete the retryable
            self.retryable(id).clear()?;
        } else {
            // Extend timeout and re-enqueue
            {
                let mut retryable = self.retryable(id);
                retryable
                    .timeout()
                    .set(timeout + RETRYABLE_LIFETIME_SECONDS)?;
            }
            {
                let mut retryable = self.retryable(id);
                retryable.timeout_windows_left().set(windows_left - 1)?;
            }
            self.timeout_queue()
                .push(U256::from_be_slice(id.as_slice()))?;
        }
        Ok(())
    }
}

pub struct Retryable<'a, CTX>
where
    CTX: ArbitrumContextTr,
{
    context: &'a mut CTX,
    gas: Option<&'a mut revm::interpreter::Gas>,
    is_static: bool,
    slot: B256,
}

impl<'a, CTX: ArbitrumContextTr> Retryable<'a, CTX> {
    pub fn new(
        context: &'a mut CTX,
        gas: Option<&'a mut revm::interpreter::Gas>,
        is_static: bool,
        subkey: B256,
    ) -> Self {
        Self {
            context,
            gas,
            is_static,
            slot: subkey,
        }
    }

    #[inline]
    fn slot(&self, offset: u64) -> B256 {
        map_address(&self.slot, &B256::from(U256::from(offset)))
    }

    pub fn num_tries(&mut self) -> StorageBackedU64<'_, CTX> {
        let slot = self.slot(0);
        StorageBackedU64::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }

    pub fn from(&mut self) -> StorageBackedAddress<'_, CTX> {
        let slot = self.slot(1);
        StorageBackedAddress::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }

    pub fn to(&mut self) -> StorageBackedAddress<'_, CTX> {
        let slot = self.slot(2);
        StorageBackedAddress::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }

    pub fn callvalue(&mut self) -> StorageBackedU256<'_, CTX> {
        let slot = self.slot(3);
        StorageBackedU256::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }

    pub fn beneficiary(&mut self) -> StorageBackedAddress<'_, CTX> {
        let slot = self.slot(4);
        StorageBackedAddress::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }

    pub fn calldata(&mut self) -> StorageBackedBytes<'_, CTX> {
        let slot = substorage(&self.slot, ARBOS_STATE_RETRYABLE_CALLDATA_KEY);
        StorageBackedBytes::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }

    pub fn timeout(&mut self) -> StorageBackedU64<'_, CTX> {
        let slot = self.slot(5);
        StorageBackedU64::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }

    pub fn timeout_windows_left(&mut self) -> StorageBackedU64<'_, CTX> {
        let slot = self.slot(6);
        StorageBackedU64::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }

    pub fn clear(&mut self) -> Result<(), ArbosStateError> {
        self.num_tries().set(0)?;
        self.timeout().set(0)?;
        self.callvalue().set(U256::ZERO)?;
        self.to().set(Address::ZERO)?;
        self.from().set(Address::ZERO)?;
        self.calldata().set(&Bytes::new())?;
        self.beneficiary().set(Address::ZERO)?;
        self.timeout_windows_left().set(0)
    }
}
