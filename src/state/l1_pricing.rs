use revm::{
    context::JournalTr,
    primitives::{Address, B256, I256, U256},
};

use crate::{
    ArbitrumContextTr,
    constants::{ARBOS_BATCH_POSTER_ADDRESS, ARBOS_L1_PRICER_FUNDS_ADDRESS},
    state::types::{
        ArbosStateError, StorageBackedAddress, StorageBackedAddressSet, StorageBackedI64,
        StorageBackedI256, StorageBackedTr, StorageBackedU64, StorageBackedU256, map_address,
        substorage,
    },
};

const ARBOS_L1_PRICING_BATCH_POSTER_TABLE_KEY: &[u8] = &[0];
const ARBOS_L1_PRICING_PAY_RECIPIENT_OFFSET: u64 = 0;
const ARBOS_L1_PRICING_EQUILIBRATION_UNITS_OFFSET: u64 = 1;
const ARBOS_L1_PRICING_INERTIA_OFFSET: u64 = 2;
const ARBOS_L1_PRICING_PER_UNIT_REWARD_OFFSET: u64 = 3;
const ARBOS_L1_PRICING_LAST_UPDATE_TIME_OFFSET: u64 = 4;
const ARBOS_L1_PRICING_FUNDS_DUE_FOR_REWARDS_OFFSET: u64 = 5;
const ARBOS_L1_PRICING_UNITS_SINCE_UPDATE_OFFSET: u64 = 6;
const ARBOS_L1_PRICING_PRICE_PER_UNIT_OFFSET: u64 = 7;
const ARBOS_L1_PRICING_LAST_SURPLUS_OFFSET: u64 = 8;
const ARBOS_L1_PRICING_PER_BATCH_GAS_COST_OFFSET: u64 = 9;
const ARBOS_L1_PRICING_AMORTIZED_COST_CAP_BIPS_OFFSET: u64 = 10;
const ARBOS_L1_PRICING_L1_FEES_AVAILABLE_OFFSET: u64 = 11;
const ARBOS_L1_PRICING_GAS_FLOOR_PER_TOKEN_OFFSET: u64 = 12;

/// Matches Go `big.Int.Div`: Euclidean division, which floors when the
/// divisor is positive instead of truncating negative quotients toward zero.
fn floor_div_i256(dividend: I256, divisor: I256) -> I256 {
    let quotient = dividend / divisor;
    let remainder = dividend % divisor;
    if remainder != I256::ZERO && dividend.is_negative() != divisor.is_negative() {
        quotient.saturating_sub(I256::ONE)
    } else {
        quotient
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod arithmetic_tests {
    use super::*;

    #[test]
    fn signed_division_matches_go_big_int_div() {
        assert_eq!(
            floor_div_i256(I256::try_from(-7).unwrap(), I256::try_from(2).unwrap()),
            I256::try_from(-4).unwrap()
        );
        assert_eq!(
            floor_div_i256(I256::try_from(-1_101).unwrap(), I256::try_from(50).unwrap()),
            I256::try_from(-23).unwrap()
        );
        assert_eq!(
            floor_div_i256(I256::try_from(7).unwrap(), I256::try_from(2).unwrap()),
            I256::try_from(3).unwrap()
        );
        assert_eq!(
            floor_div_i256(I256::try_from(-8).unwrap(), I256::try_from(2).unwrap()),
            I256::try_from(-4).unwrap()
        );
    }

    #[test]
    fn signed_pricer_intermediates_saturate_at_i256_bounds() {
        assert_eq!(I256::MIN.saturating_neg(), I256::MAX);
        assert_eq!(I256::MIN.saturating_sub(I256::ONE), I256::MIN);
        assert_eq!(I256::MAX.saturating_add(I256::ONE), I256::MAX);
    }
}

pub struct L1Pricing<'a, CTX>
where
    CTX: ArbitrumContextTr,
{
    context: &'a mut CTX,
    gas: Option<&'a mut revm::interpreter::Gas>,
    is_static: bool,
    slot: B256,
}

impl<'a, CTX: ArbitrumContextTr> L1Pricing<'a, CTX> {
    pub fn new(
        context: &'a mut CTX,
        gas: Option<&'a mut revm::interpreter::Gas>,
        is_static: bool,
        slot: B256,
    ) -> Self {
        Self {
            context,
            gas,
            is_static,
            slot,
        }
    }

    #[inline]
    fn slot(&self, offset: u64) -> B256 {
        map_address(&self.slot, &B256::from(U256::from(offset)))
    }

    pub fn batch_poster_table(&mut self) -> BatchPosterTable<'_, CTX> {
        let slot = substorage(&self.slot, ARBOS_L1_PRICING_BATCH_POSTER_TABLE_KEY);
        BatchPosterTable::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }
    pub fn reward_recipient(&mut self) -> StorageBackedAddress<'_, CTX> {
        let slot = self.slot(ARBOS_L1_PRICING_PAY_RECIPIENT_OFFSET);
        StorageBackedAddress::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }
    pub fn equilibration_units(&mut self) -> StorageBackedU256<'_, CTX> {
        let slot = self.slot(ARBOS_L1_PRICING_EQUILIBRATION_UNITS_OFFSET);
        StorageBackedU256::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }
    pub fn inertia(&mut self) -> StorageBackedU64<'_, CTX> {
        let slot = self.slot(ARBOS_L1_PRICING_INERTIA_OFFSET);
        StorageBackedU64::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }
    pub fn per_unit_reward(&mut self) -> StorageBackedU64<'_, CTX> {
        let slot = self.slot(ARBOS_L1_PRICING_PER_UNIT_REWARD_OFFSET);
        StorageBackedU64::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }
    pub fn last_update_time(&mut self) -> StorageBackedU64<'_, CTX> {
        let slot = self.slot(ARBOS_L1_PRICING_LAST_UPDATE_TIME_OFFSET);
        StorageBackedU64::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }
    pub fn funds_due_for_rewards(&mut self) -> StorageBackedI256<'_, CTX> {
        let slot = self.slot(ARBOS_L1_PRICING_FUNDS_DUE_FOR_REWARDS_OFFSET);
        StorageBackedI256::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }
    pub fn units_since_update(&mut self) -> StorageBackedU64<'_, CTX> {
        let slot = self.slot(ARBOS_L1_PRICING_UNITS_SINCE_UPDATE_OFFSET);
        StorageBackedU64::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }
    pub fn price_per_unit(&mut self) -> StorageBackedU256<'_, CTX> {
        let slot = self.slot(ARBOS_L1_PRICING_PRICE_PER_UNIT_OFFSET);
        StorageBackedU256::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }
    pub fn last_surplus(&mut self) -> StorageBackedI256<'_, CTX> {
        let slot = self.slot(ARBOS_L1_PRICING_LAST_SURPLUS_OFFSET);
        StorageBackedI256::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }
    pub fn per_batch_gas_cost(&mut self) -> StorageBackedI64<'_, CTX> {
        let slot = self.slot(ARBOS_L1_PRICING_PER_BATCH_GAS_COST_OFFSET);
        StorageBackedI64::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }
    pub fn amortized_cost_cap_bips(&mut self) -> StorageBackedU64<'_, CTX> {
        let slot = self.slot(ARBOS_L1_PRICING_AMORTIZED_COST_CAP_BIPS_OFFSET);
        StorageBackedU64::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }
    pub fn l1_fees_available(&mut self) -> StorageBackedU256<'_, CTX> {
        let slot = self.slot(ARBOS_L1_PRICING_L1_FEES_AVAILABLE_OFFSET);
        StorageBackedU256::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }
    pub fn gas_floor_per_token(&mut self) -> StorageBackedU64<'_, CTX> {
        let slot = self.slot(ARBOS_L1_PRICING_GAS_FLOOR_PER_TOKEN_OFFSET);
        StorageBackedU64::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }

    pub fn initialize(
        &mut self,
        arbos_version: u64,
        reward_recipient: Address,
        initial_l1_base_fee: U256,
    ) -> Result<(), ArbosStateError> {
        self.batch_poster_table()
            .total_funds_due()
            .set(I256::ZERO)?;
        self.batch_poster_table()
            .add_if_missing(ARBOS_BATCH_POSTER_ADDRESS, ARBOS_BATCH_POSTER_ADDRESS)?;
        self.reward_recipient().set(reward_recipient)?;
        self.equilibration_units().set(if arbos_version >= 6 {
            U256::from(160_000_000_u64)
        } else {
            U256::from(96_000_000_u64)
        })?;
        self.inertia().set(10)?;
        self.per_unit_reward().set(10)?;
        self.last_update_time().set(0)?;
        self.funds_due_for_rewards().set(I256::ZERO)?;
        self.units_since_update().set(0)?;
        self.price_per_unit().set(initial_l1_base_fee)?;
        self.last_surplus().set(I256::ZERO)?;
        self.per_batch_gas_cost().set(if arbos_version >= 11 {
            210_000
        } else if arbos_version >= 6 {
            100_000
        } else {
            0
        })?;
        self.amortized_cost_cap_bips()
            .set(if (3..11).contains(&arbos_version) {
                u64::MAX
            } else {
                0
            })?;
        self.l1_fees_available().set(U256::ZERO)?;
        self.gas_floor_per_token().set(0)?;
        Ok(())
    }

    /// Updates Nitro's post-ArbOS-10 single-dimensional batch-poster ledger.
    pub fn update_for_batch_poster_spending(
        &mut self,
        arbos_version: u64,
        update_time: u64,
        current_time: u64,
        batch_poster: Address,
        mut wei_spent: U256,
        l1_base_fee: U256,
    ) -> Result<(), ArbosStateError> {
        if arbos_version < 10 {
            return Err(ArbosStateError::Context(
                "pre-ArbOS-10 batch-poster accounting is not supported".into(),
            ));
        }
        self.batch_poster_table()
            .add_if_missing(batch_poster, batch_poster)?;

        let mut last_update_time = self.last_update_time().get()?;
        if last_update_time == 0 && update_time > 0 {
            last_update_time = update_time - 1;
        }
        if update_time > current_time || update_time < last_update_time {
            return Err(ArbosStateError::Context(
                "invalid batch report timestamp".into(),
            ));
        }
        let mut numerator = update_time - last_update_time;
        let mut denominator = current_time - last_update_time;
        if denominator == 0 {
            numerator = 1;
            denominator = 1;
        }

        let units_since_update = self.units_since_update().get()?;
        let units_allocated = units_since_update.saturating_mul(numerator) / denominator;
        self.units_since_update()
            .set(units_since_update - units_allocated)?;

        let cap_bips = self.amortized_cost_cap_bips().get()?;
        if arbos_version >= 3 && cap_bips != 0 {
            let cap = l1_base_fee
                .saturating_mul(U256::from(units_allocated))
                .saturating_mul(U256::from(cap_bips))
                / U256::from(10_000);
            wei_spent = wei_spent.min(cap);
        }

        let poster_due = self
            .batch_poster_table()
            .get(batch_poster)
            .funds_due()
            .get()?;
        self.batch_poster_table()
            .get(batch_poster)
            .set_funds_due(poster_due.saturating_add(wei_spent))?;

        let reward =
            U256::from(self.per_unit_reward().get()?).saturating_mul(U256::from(units_allocated));
        let reward_signed = I256::try_from(reward).unwrap_or(I256::MAX);
        let rewards_due = self
            .funds_due_for_rewards()
            .get()?
            .saturating_add(reward_signed);
        self.funds_due_for_rewards().set(rewards_due)?;

        let mut available = self.l1_fees_available().get()?;
        let reward_payment = available.min(reward);
        if !reward_payment.is_zero() {
            let recipient = self.reward_recipient().get()?;
            self.transfer_from_l1_fees(recipient, reward_payment)?;
            available -= reward_payment;
            self.funds_due_for_rewards().set(
                rewards_due.saturating_sub(I256::try_from(reward_payment).unwrap_or(I256::MAX)),
            )?;
        }

        let poster_due = self
            .batch_poster_table()
            .get(batch_poster)
            .funds_due()
            .get()?;
        let poster_payment = available.min(poster_due);
        if !poster_payment.is_zero() {
            let recipient = self
                .batch_poster_table()
                .get(batch_poster)
                .pay_recipient()
                .get()?;
            self.transfer_from_l1_fees(recipient, poster_payment)?;
            available -= poster_payment;
            self.batch_poster_table()
                .get(batch_poster)
                .set_funds_due(poster_due - poster_payment)?;
        }
        self.last_update_time().set(update_time)?;

        if units_allocated > 0 {
            let total_due = self.batch_poster_table().total_funds_due().get()?;
            let rewards_due = self.funds_due_for_rewards().get()?;
            let surplus = I256::try_from(available)
                .unwrap_or(I256::MAX)
                .saturating_sub(total_due)
                .saturating_sub(rewards_due);
            let equilibration = I256::try_from(self.equilibration_units().get()?)
                .map_err(|_| ArbosStateError::Context("equilibration units too large".into()))?;
            let inertia = self.inertia().get()?;
            if equilibration == I256::ZERO || inertia == 0 {
                return Err(ArbosStateError::Context(
                    "invalid zero L1-pricing inertia or equilibration units".into(),
                ));
            }
            let units = I256::try_from(units_allocated).unwrap();
            let inertia_units = floor_div_i256(equilibration, I256::try_from(inertia).unwrap());
            let old_surplus = self.last_surplus().get()?;
            let desired_derivative = floor_div_i256(surplus.saturating_neg(), equilibration);
            let actual_derivative = floor_div_i256(surplus.saturating_sub(old_surplus), units);
            let price_change = floor_div_i256(
                desired_derivative
                    .saturating_sub(actual_derivative)
                    .saturating_mul(units),
                inertia_units.saturating_add(units),
            );
            self.last_surplus().set(surplus)?;
            let price = I256::try_from(self.price_per_unit().get()?).unwrap_or(I256::MAX);
            let new_price = price.saturating_add(price_change);
            self.price_per_unit().set(if new_price.is_negative() {
                U256::ZERO
            } else {
                U256::try_from(new_price).unwrap_or(U256::MAX)
            })?;
        }
        Ok(())
    }

    fn transfer_from_l1_fees(
        &mut self,
        recipient: Address,
        amount: U256,
    ) -> Result<(), ArbosStateError> {
        let transfer_error = self
            .context
            .journal_mut()
            .transfer(ARBOS_L1_PRICER_FUNDS_ADDRESS, recipient, amount)
            .map_err(|err| ArbosStateError::Context(err.to_string()))?;
        if let Some(err) = transfer_error {
            return Err(ArbosStateError::Context(format!(
                "L1-pricer transfer failed: {err:?}"
            )));
        }
        let available = self.l1_fees_available().get()?;
        self.l1_fees_available()
            .set(available.saturating_sub(amount))
    }
}

const ARBOS_BATCH_POSTER_ADDRS_KEY: &[u8] = &[0];
const ARBOS_BATCH_POSTER_INFO_KEY: &[u8] = &[1];

pub struct BatchPosterTable<'a, CTX>
where
    CTX: ArbitrumContextTr,
{
    context: &'a mut CTX,
    gas: Option<&'a mut revm::interpreter::Gas>,
    is_static: bool,
    slot: B256,
}

impl<'a, CTX: ArbitrumContextTr> BatchPosterTable<'a, CTX> {
    pub fn new(
        context: &'a mut CTX,
        gas: Option<&'a mut revm::interpreter::Gas>,
        is_static: bool,
        slot: B256,
    ) -> Self {
        Self {
            context,
            gas,
            is_static,
            slot,
        }
    }

    fn posters_address_set(&mut self) -> StorageBackedAddressSet<'_, CTX> {
        let slot = substorage(&self.slot, ARBOS_BATCH_POSTER_ADDRS_KEY);
        StorageBackedAddressSet::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }

    pub fn all(&mut self) -> Result<Vec<Address>, ArbosStateError> {
        self.posters_address_set().all()
    }

    pub fn get(&mut self, batch_poster: Address) -> BatchPosterState<'_, CTX> {
        let poster_info = substorage(&self.slot, ARBOS_BATCH_POSTER_INFO_KEY);
        let bp_storage = substorage(&poster_info, batch_poster.as_slice());
        BatchPosterState::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            bp_storage,
            self.slot,
        )
    }

    pub fn contains(&mut self, batch_poster: Address) -> Result<bool, ArbosStateError> {
        Ok(self.all()?.contains(&batch_poster))
    }

    pub fn add_if_missing(
        &mut self,
        batch_poster: Address,
        pay_recipient: Address,
    ) -> Result<bool, ArbosStateError> {
        if self.contains(batch_poster)? {
            return Ok(false);
        }

        self.add(batch_poster, pay_recipient)?;
        Ok(true)
    }

    pub fn add(
        &mut self,
        batch_poster: Address,
        pay_recipient: Address,
    ) -> Result<(), ArbosStateError> {
        self.posters_address_set().add(batch_poster)?;
        self.get(batch_poster).pay_recipient().set(pay_recipient)
    }

    pub fn total_funds_due(&mut self) -> StorageBackedI256<'_, CTX> {
        let slot = map_address(&self.slot, &B256::from(U256::ZERO));
        StorageBackedI256::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }

    pub fn fee_collector(&mut self, batch_poster: Address) -> Result<Address, ArbosStateError> {
        self.get(batch_poster).pay_recipient().get()
    }

    pub fn set_fee_collector(
        &mut self,
        batch_poster: Address,
        new_fee_collector: Address,
    ) -> Result<(), ArbosStateError> {
        self.get(batch_poster)
            .pay_recipient()
            .set(new_fee_collector)
    }
}

pub struct BatchPosterState<'a, CTX>
where
    CTX: ArbitrumContextTr,
{
    context: &'a mut CTX,
    gas: Option<&'a mut revm::interpreter::Gas>,
    is_static: bool,
    slot: B256,
    table_slot: B256,
}

impl<'a, CTX: ArbitrumContextTr> BatchPosterState<'a, CTX> {
    pub fn new(
        context: &'a mut CTX,
        gas: Option<&'a mut revm::interpreter::Gas>,
        is_static: bool,
        slot: B256,
        table_slot: B256,
    ) -> Self {
        Self {
            context,
            gas,
            is_static,
            slot,
            table_slot,
        }
    }

    pub fn funds_due(&mut self) -> StorageBackedU256<'_, CTX> {
        let slot = map_address(&self.slot, &B256::from(U256::ZERO));
        StorageBackedU256::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }

    pub fn pay_recipient(&mut self) -> StorageBackedAddress<'_, CTX> {
        let slot = map_address(&self.slot, &B256::from(U256::ONE));
        StorageBackedAddress::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }

    pub fn set_funds_due(&mut self, value: U256) -> Result<(), ArbosStateError> {
        let previous = self.funds_due().get()?;
        let total_slot = map_address(&self.table_slot, &B256::from(U256::ZERO));
        let mut total = StorageBackedI256::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            total_slot,
        );
        let previous_total = total.get()?;
        let value_signed = I256::try_from(value).unwrap_or(I256::MAX);
        let previous_signed = I256::try_from(previous).unwrap_or(I256::MAX);
        total.set(previous_total.saturating_add(value_signed.saturating_sub(previous_signed)))?;
        self.funds_due().set(value)
    }
}
