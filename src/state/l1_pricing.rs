use revm::primitives::{Address, B256, I256, U256};

use crate::{
    ArbitrumContextTr,
    constants::{
        ARBOS_BATCH_POSTER_ADDRESS, INITIAL_L1_EQUILIBRATION_UNITS, INITIAL_L1_INERTIA,
        INITIAL_L1_PER_UNIT_REWARD,
    },
    math::big_mul_by_bips,
    state::types::{
        ArbosStateError, StorageBackedAddress, StorageBackedAddressSet, StorageBackedI256,
        StorageBackedTr, StorageBackedU64, StorageBackedU256, map_address, substorage,
    },
};

#[derive(Debug, Clone, PartialEq)]
pub struct L1PricingParams {
    pub equilibration_units: U256,
    pub inertia: u64,
    pub per_unit_reward: u64,
    pub price_per_unit: U256,
    pub per_batch_gas_cost: u64,
    pub amortized_cost_cap_bips: u64,
}

impl Default for L1PricingParams {
    fn default() -> Self {
        Self {
            equilibration_units: U256::from(INITIAL_L1_EQUILIBRATION_UNITS),
            inertia: INITIAL_L1_INERTIA,
            per_unit_reward: INITIAL_L1_PER_UNIT_REWARD,
            price_per_unit: U256::ZERO,
            per_batch_gas_cost: 0,
            amortized_cost_cap_bips: 0,
        }
    }
}

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
    pub fn per_batch_gas_cost(&mut self) -> StorageBackedU64<'_, CTX> {
        let slot = self.slot(ARBOS_L1_PRICING_PER_BATCH_GAS_COST_OFFSET);
        StorageBackedU64::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
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

    pub fn initialize(&mut self, params: &L1PricingParams) -> Result<(), ArbosStateError> {
        self.equilibration_units().set(params.equilibration_units)?;
        self.inertia().set(params.inertia)?;
        self.per_unit_reward().set(params.per_unit_reward)?;
        self.price_per_unit().set(params.price_per_unit)?;
        self.per_batch_gas_cost().set(params.per_batch_gas_cost)?;
        self.amortized_cost_cap_bips()
            .set(params.amortized_cost_cap_bips)?;

        // Register the default batch poster (nitro l1pricing.go:92-94)
        self.batch_poster_table()
            .add_if_missing(ARBOS_BATCH_POSTER_ADDRESS, ARBOS_BATCH_POSTER_ADDRESS)?;

        Ok(())
    }

    /// Add calldata units to the units_since_update counter.
    /// Matches nitro `l1pricing.go:230-236` (AddToUnitsSinceUpdate).
    pub fn add_to_units_since_update(&mut self, units: u64) -> Result<(), ArbosStateError> {
        let old_units = self.units_since_update().get()?;
        self.units_since_update()
            .set(old_units.saturating_add(units))
    }

    pub fn get(&mut self) -> Result<L1PricingParams, ArbosStateError> {
        Ok(L1PricingParams {
            equilibration_units: self.equilibration_units().get()?,
            inertia: self.inertia().get()?,
            per_unit_reward: self.per_unit_reward().get()?,
            price_per_unit: self.price_per_unit().get()?,
            per_batch_gas_cost: self.per_batch_gas_cost().get()?,
            amortized_cost_cap_bips: self.amortized_cost_cap_bips().get()?,
        })
    }

    /// Processes a batch poster spending report, updating L1 pricing state.
    ///
    /// This implements the core L1 fee accounting: allocates calldata units,
    /// records poster debt, accrues rewards, settles payments from the fee
    /// pool, and adjusts the per-unit price using a signed-derivative
    /// convergence algorithm.
    ///
    /// Matches nitro `l1pricing.go:320-516`. Actual ETH transfers are omitted
    /// (state accounting only) since arbos-revm is a testing tool.
    pub fn update_for_batch_poster_spending(
        &mut self,
        update_time: u64,
        current_time: u64,
        batch_poster: Address,
        mut wei_spent: U256,
        l1_basefee: U256,
    ) -> Result<(), ArbosStateError> {
        // --- Step 1: Time allocation fraction ---
        let mut last_update_time = self.last_update_time().get()?;

        // First-update guard when lastUpdateTime==0
        // Matches nitro l1pricing.go:356-357
        if last_update_time == 0 && update_time > 0 {
            last_update_time = update_time - 1;
        }

        // Time bounds validation
        // Matches nitro l1pricing.go:359
        if update_time > current_time || update_time < last_update_time {
            return Err(ArbosStateError::InvalidTime);
        }

        let (alloc_num, alloc_denom) = {
            let denom = current_time.saturating_sub(last_update_time);
            let num = update_time.saturating_sub(last_update_time);
            if denom == 0 {
                (1u64, 1u64)
            } else {
                (num, denom)
            }
        };

        let units_since = self.units_since_update().get()?;
        let units_allocated = units_since
            .saturating_mul(alloc_num)
            .checked_div(alloc_denom)
            .unwrap_or(0);
        self.units_since_update()
            .set(units_since.saturating_sub(units_allocated))?;

        // --- Step 2: Amortized cost cap ---
        let cost_cap_bips = self.amortized_cost_cap_bips().get()?;
        if cost_cap_bips != 0 && units_allocated > 0 {
            let baseline = l1_basefee.saturating_mul(U256::from(units_allocated));
            let cap = big_mul_by_bips(baseline, cost_cap_bips as i64);
            if cap < wei_spent {
                wei_spent = cap;
            }
        }

        // --- Step 3: Record batch poster debt ---
        {
            let mut table = self.batch_poster_table();
            let due = table.get(batch_poster).funds_due().get()?;
            table
                .get(batch_poster)
                .funds_due()
                .set(due.saturating_add(wei_spent))?;
        }

        // --- Step 4: Accumulate rewards ---
        let per_unit_reward = self.per_unit_reward().get()?;
        let reward_increment =
            I256::try_from(U256::from(per_unit_reward).saturating_mul(U256::from(units_allocated)))
                .unwrap_or(I256::MAX);
        let funds_due_for_rewards = self.funds_due_for_rewards().get()?;
        self.funds_due_for_rewards()
            .set(funds_due_for_rewards.saturating_add(reward_increment))?;

        // --- Step 5: Settle rewards (accounting only, no transfer) ---
        // Only pay current period's reward, not all accumulated debt.
        // Matches nitro l1pricing.go:417-421
        let mut l1_fees_available = self.l1_fees_available().get()?;
        let funds_due_for_rewards = self.funds_due_for_rewards().get()?;
        {
            let payment_for_rewards =
                U256::from(per_unit_reward).saturating_mul(U256::from(units_allocated));
            let payment_for_rewards = payment_for_rewards.min(l1_fees_available);
            let payment_i256 = I256::try_from(payment_for_rewards).unwrap_or(I256::MAX);
            let new_funds_due = funds_due_for_rewards.saturating_sub(payment_i256);
            self.funds_due_for_rewards().set(new_funds_due)?;
            l1_fees_available = l1_fees_available.saturating_sub(payment_for_rewards);
            self.l1_fees_available().set(l1_fees_available)?;
        }

        // --- Step 6: Settle poster refund (accounting only, no transfer) ---
        {
            let poster_due = self
                .batch_poster_table()
                .get(batch_poster)
                .funds_due()
                .get()?;
            if !poster_due.is_zero() {
                let payment = poster_due.min(l1_fees_available);
                self.batch_poster_table()
                    .get(batch_poster)
                    .funds_due()
                    .set(poster_due.saturating_sub(payment))?;
                l1_fees_available = l1_fees_available.saturating_sub(payment);
                self.l1_fees_available().set(l1_fees_available)?;
            }
        }

        // --- Step 7: Update last update time ---
        self.last_update_time().set(update_time)?;

        // --- Step 8: Price adjustment (only if units were allocated) ---
        if units_allocated > 0 {
            // Compute surplus = l1_fees_available - (total_funds_due + funds_due_for_rewards)
            let total_funds_due_i256 = self.batch_poster_table().total_funds_due().get()?;
            let funds_due_for_rewards = self.funds_due_for_rewards().get()?;
            let l1_fees_i256 = I256::try_from(l1_fees_available).unwrap_or(I256::MAX);
            let obligations = total_funds_due_i256.saturating_add(funds_due_for_rewards);
            let surplus = l1_fees_i256.saturating_sub(obligations);

            let equilibration_units = self.equilibration_units().get()?;
            let inertia = self.inertia().get()?;
            let last_surplus = self.last_surplus().get()?;
            let current_price = I256::try_from(self.price_per_unit().get()?).unwrap_or(I256::MAX);

            if !equilibration_units.is_zero() {
                let eq_i256 = I256::try_from(equilibration_units).unwrap_or(I256::MAX);
                let units_i256 = I256::try_from(U256::from(units_allocated)).unwrap_or(I256::MAX);
                let inertia_units =
                    eq_i256 / I256::try_from(U256::from(inertia)).unwrap_or(I256::ONE);

                // desired_derivative = -surplus / equilibration_units
                let desired_derivative = surplus.checked_neg().unwrap_or(I256::ZERO) / eq_i256;

                // actual_derivative = (surplus - last_surplus) / units_allocated
                let actual_derivative = surplus.saturating_sub(last_surplus) / units_i256;

                // change_derivative_by = desired - actual
                let change_derivative_by = desired_derivative.saturating_sub(actual_derivative);

                // price_change = change_derivative_by * units_allocated / (units_allocated + inertia_units)
                let alloc_plus_inert = units_i256.saturating_add(inertia_units);
                let price_change = if alloc_plus_inert.is_zero() {
                    I256::ZERO
                } else {
                    change_derivative_by.saturating_mul(units_i256) / alloc_plus_inert
                };

                let new_price = current_price.saturating_add(price_change);
                let new_price_u256 = if new_price.is_negative() {
                    U256::ZERO
                } else {
                    U256::from(new_price.into_raw())
                };
                self.price_per_unit().set(new_price_u256)?;
            }

            self.last_surplus().set(surplus)?;
        }

        Ok(())
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

    /// Set funds due for a batch poster, atomically updating both the
    /// individual poster's `fundsDue` and the aggregate `totalFundsDue`.
    /// Matches nitro `batchPoster.go:119-134` (SetFundsDue).
    pub fn set_funds_due(
        &mut self,
        batch_poster: Address,
        new_val: U256,
    ) -> Result<(), ArbosStateError> {
        let old_val = self.get(batch_poster).funds_due().get()?;
        self.get(batch_poster).funds_due().set(new_val)?;

        let total = self.total_funds_due().get()?;
        let total_u256 = U256::from(total.into_raw());
        let new_total_u256 = total_u256.saturating_add(new_val).saturating_sub(old_val);
        let new_total = I256::try_from(new_total_u256).unwrap_or(I256::MAX);
        self.total_funds_due().set(new_total)?;

        Ok(())
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
}

impl<'a, CTX: ArbitrumContextTr> BatchPosterState<'a, CTX> {
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

    pub fn funds_due(&mut self) -> StorageBackedU256<'_, CTX> {
        let slot = map_address(&self.slot, &B256::from(U256::ZERO));
        StorageBackedU256::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }

    pub fn pay_recipient(&mut self) -> StorageBackedAddress<'_, CTX> {
        let slot = map_address(&self.slot, &B256::from(U256::ONE));
        StorageBackedAddress::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }
}
