use revm::{
    context::JournalTr,
    primitives::{Address, B256, Bytes, U256, keccak256},
};

use crate::{
    ArbitrumContextTr,
    state::types::{
        ArbosStateError, StorageBackedAddress, StorageBackedAddressOrNil, StorageBackedBytes,
        StorageBackedQueue, StorageBackedTr, StorageBackedU64, StorageBackedU256, map_address,
        substorage,
    },
};

const ARBOS_STATE_RETRYABLE_TIMEOUT_QUEUE_KEY: &[u8] = &[0];
const ARBOS_STATE_RETRYABLE_CALLDATA_KEY: &[u8] = &[1];
const RETRYABLE_LIFETIME_SECONDS: u64 = 7 * 24 * 60 * 60;

fn retryable_escrow_address(ticket_id: B256) -> Address {
    let mut input = Vec::with_capacity("retryable escrow".len() + 32);
    input.extend_from_slice(b"retryable escrow");
    input.extend_from_slice(ticket_id.as_slice());
    Address::from_slice(&keccak256(input)[12..])
}

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

    #[allow(clippy::too_many_arguments)]
    pub fn create_retryable(
        &mut self,
        id: B256,
        timeout: u64,
        from: Address,
        to: Option<Address>,
        callvalue: U256,
        beneficiary: Address,
        calldata: &Bytes,
    ) -> Result<(), ArbosStateError> {
        let mut retryable = self.retryable(id);
        retryable.num_tries().set(0)?;
        retryable.from().set(from)?;
        retryable.to().set(to)?;
        retryable.callvalue().set(callvalue)?;
        retryable.beneficiary().set(beneficiary)?;
        retryable.calldata().set(calldata)?;
        retryable.timeout().set(timeout)?;
        retryable.timeout_windows_left().set(0)?;
        self.timeout_queue()
            .push(U256::from_be_slice(id.as_slice()))
    }

    /// Advances or removes at most one queued retryable, matching Nitro's
    /// bounded `startBlock` reaping operation.
    pub fn try_reap_one(&mut self, current_timestamp: u64) -> Result<(), ArbosStateError> {
        let Some(raw_id) = self.timeout_queue().peek()? else {
            return Ok(());
        };
        let id = B256::from(raw_id.to_be_bytes());
        let timeout = self.retryable(id).timeout().get()?;
        if timeout == 0 {
            self.timeout_queue().pop()?;
            return Ok(());
        }

        let windows_left = self.retryable(id).timeout_windows_left().get()?;
        if timeout >= current_timestamp {
            return Ok(());
        }
        self.timeout_queue().pop()?;

        if windows_left > 0 {
            let mut retryable = self.retryable(id);
            retryable
                .timeout()
                .set(timeout.saturating_add(RETRYABLE_LIFETIME_SECONDS))?;
            return retryable
                .timeout_windows_left()
                .set(windows_left.saturating_sub(1));
        }

        let beneficiary = self.retryable(id).beneficiary().get()?;
        let escrow = retryable_escrow_address(id);
        let balance = self
            .context
            .balance(escrow)
            .map(|load| load.data)
            .unwrap_or_default();
        if !balance.is_zero() {
            let transfer_error = self
                .context
                .journal_mut()
                .transfer(escrow, beneficiary, balance)
                .map_err(|_| {
                    ArbosStateError::Context("failed to refund retryable escrow".into())
                })?;
            if transfer_error.is_some() {
                return Err(ArbosStateError::Context(
                    "retryable escrow balance disappeared".into(),
                ));
            }
        }
        self.retryable(id).clear()
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

    pub fn to(&mut self) -> StorageBackedAddressOrNil<'_, CTX> {
        let slot = self.slot(2);
        StorageBackedAddressOrNil::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
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
        // DeleteRetryable clears the raw word rather than writing the nil
        // contract-creation sentinel used by a live retryable.
        self.to().set(Some(Address::ZERO))?;
        self.from().set(Address::ZERO)?;
        self.calldata().clear()?;
        self.beneficiary().set(Address::ZERO)?;
        self.timeout_windows_left().set(0)
    }
}
