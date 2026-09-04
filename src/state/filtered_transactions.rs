use revm::{
    context::JournalTr,
    interpreter::{
        Gas,
        gas::{ISTANBUL_SLOAD_GAS, SSTORE_RESET, SSTORE_SET},
    },
    primitives::{B256, U256},
};

use crate::{
    ArbitrumContextTr,
    constants::FILTERED_TRANSACTIONS_STATE_ADDRESS,
    state::types::{ArbosStateError, map_address},
};

/// Nitro-compatible transaction-filter storage. Unlike the rest of ArbOS state,
/// entries live at a dedicated account and use the transaction hash directly as
/// the storage key.
pub struct FilteredTransactions<'a, CTX: ArbitrumContextTr> {
    context: &'a mut CTX,
    gas: Option<&'a mut Gas>,
    is_static: bool,
}

impl<'a, CTX: ArbitrumContextTr> FilteredTransactions<'a, CTX> {
    pub fn new(context: &'a mut CTX, gas: Option<&'a mut Gas>, is_static: bool) -> Self {
        Self {
            context,
            gas,
            is_static,
        }
    }

    pub fn is_filtered(&mut self, tx_hash: B256) -> Result<bool, ArbosStateError> {
        self.load_account()?;
        if let Some(gas) = &mut self.gas
            && !gas.record_cost(ISTANBUL_SLOAD_GAS)
        {
            return Err(ArbosStateError::OutOfGas);
        }
        let slot = map_address(&B256::ZERO, &tx_hash);
        let value = self
            .context
            .journal_mut()
            .sload(FILTERED_TRANSACTIONS_STATE_ADDRESS, slot.into())
            .map(|value| value.data)
            .map_err(|err| ArbosStateError::Context(err.to_string()))?;
        Ok(value == U256::ONE)
    }

    pub fn add(&mut self, tx_hash: B256) -> Result<(), ArbosStateError> {
        self.set(tx_hash, U256::ONE)
    }

    pub fn delete(&mut self, tx_hash: B256) -> Result<(), ArbosStateError> {
        self.set(tx_hash, U256::ZERO)
    }

    fn set(&mut self, tx_hash: B256, value: U256) -> Result<(), ArbosStateError> {
        if self.is_static {
            return Err(ArbosStateError::StateChangeDuringStaticCall);
        }
        self.load_account()?;
        if let Some(gas) = &mut self.gas {
            let cost = if value.is_zero() {
                SSTORE_RESET
            } else {
                SSTORE_SET
            };
            if !gas.record_cost(cost) {
                return Err(ArbosStateError::OutOfGas);
            }
        }
        let slot = map_address(&B256::ZERO, &tx_hash);
        self.context
            .sstore(FILTERED_TRANSACTIONS_STATE_ADDRESS, slot.into(), value)
            .ok_or_else(|| ArbosStateError::Context("filtered transaction sstore failed".into()))?;
        self.context
            .journal_mut()
            .touch_account(FILTERED_TRANSACTIONS_STATE_ADDRESS);
        Ok(())
    }

    fn load_account(&mut self) -> Result<(), ArbosStateError> {
        self.context
            .journal_mut()
            .load_account_mut(FILTERED_TRANSACTIONS_STATE_ADDRESS)
            .map(|mut account| {
                if account.nonce() == 0 {
                    account.set_nonce(1);
                }
            })
            .map_err(|_| {
                ArbosStateError::Context("failed to load filtered transaction state".into())
            })
    }
}
