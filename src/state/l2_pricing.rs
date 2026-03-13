use revm::primitives::{B256, U256};

use crate::{
    ArbitrumContextTr,
    constants::{
        INITIAL_L2_BACKLOG_TOLERANCE, INITIAL_L2_BASE_FEE_WEI, INITIAL_L2_MIN_BASE_FEE_WEI,
        INITIAL_L2_PER_BLOCK_GAS_LIMIT, INITIAL_L2_PER_TX_GAS_LIMIT, INITIAL_L2_PRICING_INERTIA,
        INITIAL_L2_SPEED_LIMIT,
    },
    math::{apply_gas_delta, approx_exp_basis_points, big_mul_by_bips, natural_to_bips},
    state::types::{
        ArbosStateError, StorageBackedTr, StorageBackedU64, StorageBackedU256, map_address,
    },
};

#[derive(Debug, Clone, PartialEq)]
pub struct L2PricingParams {
    pub speed_limit_per_second: u64,
    pub per_block_gas_limit: u64,
    pub base_fee_wei: U256,
    pub min_base_fee_wei: U256,
    pub gas_backlog: u64,
    pub pricing_inertia: u64,
    pub backlog_tolerance: u64,
    pub per_tx_gas_limit: u64,
}

impl Default for L2PricingParams {
    fn default() -> Self {
        Self {
            speed_limit_per_second: INITIAL_L2_SPEED_LIMIT,
            per_block_gas_limit: INITIAL_L2_PER_BLOCK_GAS_LIMIT,
            base_fee_wei: U256::from(INITIAL_L2_BASE_FEE_WEI),
            min_base_fee_wei: U256::from(INITIAL_L2_MIN_BASE_FEE_WEI),
            gas_backlog: 0,
            pricing_inertia: INITIAL_L2_PRICING_INERTIA,
            backlog_tolerance: INITIAL_L2_BACKLOG_TOLERANCE,
            per_tx_gas_limit: INITIAL_L2_PER_TX_GAS_LIMIT,
        }
    }
}

pub struct L2Pricing<'a, CTX>
where
    CTX: ArbitrumContextTr,
{
    context: &'a mut CTX,
    gas: Option<&'a mut revm::interpreter::Gas>,
    is_static: bool,
    slot: B256,
}

impl<'a, CTX: ArbitrumContextTr> L2Pricing<'a, CTX> {
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

    pub fn speed_limit_per_second(&mut self) -> StorageBackedU64<'_, CTX> {
        let slot = self.slot(0);
        StorageBackedU64::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }

    pub fn per_block_gas_limit(&mut self) -> StorageBackedU64<'_, CTX> {
        let slot = self.slot(1);
        StorageBackedU64::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }

    pub fn base_fee_wei(&mut self) -> StorageBackedU256<'_, CTX> {
        let slot = self.slot(2);
        StorageBackedU256::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }

    pub fn min_base_fee_wei(&mut self) -> StorageBackedU256<'_, CTX> {
        let slot = self.slot(3);
        StorageBackedU256::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }

    pub fn gas_backlog(&mut self) -> StorageBackedU64<'_, CTX> {
        let slot = self.slot(4);
        StorageBackedU64::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }

    pub fn pricing_inertia(&mut self) -> StorageBackedU64<'_, CTX> {
        let slot = self.slot(5);
        StorageBackedU64::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }

    pub fn backlog_tolerance(&mut self) -> StorageBackedU64<'_, CTX> {
        let slot = self.slot(6);
        StorageBackedU64::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }

    pub fn per_tx_gas_limit(&mut self) -> StorageBackedU64<'_, CTX> {
        let slot = self.slot(7);
        StorageBackedU64::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }

    pub fn initialize(&mut self, params: &L2PricingParams) -> Result<(), ArbosStateError> {
        self.speed_limit_per_second()
            .set(params.speed_limit_per_second)?;
        self.per_block_gas_limit().set(params.per_block_gas_limit)?;
        self.base_fee_wei().set(params.base_fee_wei)?;
        self.min_base_fee_wei().set(params.min_base_fee_wei)?;
        self.gas_backlog().set(params.gas_backlog)?;
        self.pricing_inertia().set(params.pricing_inertia)?;
        self.backlog_tolerance().set(params.backlog_tolerance)?;
        self.per_tx_gas_limit().set(params.per_tx_gas_limit)?;
        Ok(())
    }

    pub fn get(&mut self) -> Result<L2PricingParams, ArbosStateError> {
        Ok(L2PricingParams {
            speed_limit_per_second: self.speed_limit_per_second().get()?,
            per_block_gas_limit: self.per_block_gas_limit().get()?,
            base_fee_wei: self.base_fee_wei().get()?,
            min_base_fee_wei: self.min_base_fee_wei().get()?,
            gas_backlog: self.gas_backlog().get()?,
            pricing_inertia: self.pricing_inertia().get()?,
            backlog_tolerance: self.backlog_tolerance().get()?,
            per_tx_gas_limit: self.per_tx_gas_limit().get()?,
        })
    }

    /// Adjusts the gas backlog by the given signed delta.
    /// Positive values drain the backlog (time-based or consumed gas);
    /// negative values increase it (gas used by transactions).
    ///
    /// Matches nitro `model.go:39-57` (legacy single-constraint path).
    pub fn add_to_gas_pool(&mut self, gas: i64) -> Result<(), ArbosStateError> {
        let backlog = self.gas_backlog().get()?;
        self.gas_backlog().set(apply_gas_delta(backlog, gas))
    }

    /// Updates the L2 pricing model for a new block.
    ///
    /// 1. Drains the gas backlog by `time_passed * speed_limit`.
    /// 2. Recomputes the base fee using an exponential function of excess
    ///    backlog over the tolerance threshold.
    ///
    /// Matches nitro `model.go:119-133`.
    pub fn update_pricing_model(&mut self, time_passed: u64) -> Result<(), ArbosStateError> {
        let speed_limit = self.speed_limit_per_second().get()?;
        let drain = i64::try_from(time_passed.saturating_mul(speed_limit)).unwrap_or(i64::MAX);
        self.add_to_gas_pool(drain)?;

        let inertia = self.pricing_inertia().get()?;
        let tolerance = self.backlog_tolerance().get()?;
        let backlog = self.gas_backlog().get()?;
        let min_base_fee = self.min_base_fee_wei().get()?;

        let threshold = tolerance.saturating_mul(speed_limit);
        let base_fee = if backlog > threshold {
            let excess = i64::try_from(backlog - threshold).unwrap_or(i64::MAX);
            let denom = i64::try_from(inertia.saturating_mul(speed_limit))
                .unwrap_or(i64::MAX)
                .max(1);
            let exponent_bips = natural_to_bips(excess) / denom;
            big_mul_by_bips(min_base_fee, approx_exp_basis_points(exponent_bips, 4))
        } else {
            min_base_fee
        };
        self.base_fee_wei().set(base_fee)
    }
}
