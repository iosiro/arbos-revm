use revm::primitives::{B256, U256};

use crate::{
    ArbitrumContextTr,
    state::types::{
        ArbosStateError, StorageBackedTr, StorageBackedU64, StorageBackedU256, map_address,
        substorage,
    },
};

const ONE_IN_BIPS: u64 = 10_000;
pub const GAS_CONSTRAINTS_MAX_NUM: u64 = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GasConstraint {
    pub target: u64,
    pub adjustment_window: u64,
    pub backlog: u64,
}

fn approx_exp_basis_points(value: u64) -> u64 {
    const ACCURACY: u64 = 4;
    let mut result = ONE_IN_BIPS.saturating_add(value / ACCURACY);
    for i in (1..ACCURACY).rev() {
        result = ONE_IN_BIPS.saturating_add(
            result
                .saturating_mul(value)
                .checked_div(i * ONE_IN_BIPS)
                .unwrap_or(u64::MAX),
        );
    }
    result
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

    /// Initializes Nitro's legacy single-dimensional L2 pricing state. Nitro
    /// first writes the ArbOS-0 defaults and, for a genesis version at least 6,
    /// applies the first-time v6 throughput limits.
    pub fn initialize(&mut self, arbos_version: u64) -> Result<(), ArbosStateError> {
        let (speed_limit, block_limit) = if arbos_version >= 6 {
            (7_000_000, 32_000_000)
        } else {
            (1_000_000, 20_000_000)
        };
        self.speed_limit_per_second().set(speed_limit)?;
        self.per_block_gas_limit().set(block_limit)?;
        self.base_fee_wei().set(U256::from(100_000_000u64))?;
        self.min_base_fee_wei().set(U256::from(100_000_000u64))?;
        self.gas_backlog().set(0)?;
        self.pricing_inertia().set(102)?;
        self.backlog_tolerance().set(10)?;
        if arbos_version >= 50 {
            self.per_tx_gas_limit().set(32_000_000)?;
        }
        Ok(())
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

    #[inline]
    fn gas_constraints_root(&self) -> B256 {
        substorage(&self.slot, &[0])
    }

    #[inline]
    fn gas_constraints_length_slot(&self) -> B256 {
        map_address(&self.gas_constraints_root(), &B256::ZERO)
    }

    #[inline]
    fn gas_constraint_root(&self, index: u64) -> B256 {
        substorage(&self.gas_constraints_root(), &index.to_be_bytes())
    }

    #[inline]
    fn gas_constraint_slot(&self, index: u64, offset: u64) -> B256 {
        map_address(
            &self.gas_constraint_root(index),
            &B256::from(U256::from(offset)),
        )
    }

    pub fn gas_constraints_len(&mut self) -> Result<u64, ArbosStateError> {
        let slot = self.gas_constraints_length_slot();
        StorageBackedU64::new(self.context, self.gas.as_deref_mut(), self.is_static, slot).get()
    }

    pub fn gas_constraint(&mut self, index: u64) -> Result<GasConstraint, ArbosStateError> {
        let target_slot = self.gas_constraint_slot(index, 0);
        let window_slot = self.gas_constraint_slot(index, 1);
        let backlog_slot = self.gas_constraint_slot(index, 2);
        Ok(GasConstraint {
            target: StorageBackedU64::new(
                self.context,
                self.gas.as_deref_mut(),
                self.is_static,
                target_slot,
            )
            .get()?,
            adjustment_window: StorageBackedU64::new(
                self.context,
                self.gas.as_deref_mut(),
                self.is_static,
                window_slot,
            )
            .get()?,
            backlog: StorageBackedU64::new(
                self.context,
                self.gas.as_deref_mut(),
                self.is_static,
                backlog_slot,
            )
            .get()?,
        })
    }

    fn gas_constraint_backlog(&mut self, index: u64) -> Result<u64, ArbosStateError> {
        let slot = self.gas_constraint_slot(index, 2);
        StorageBackedU64::new(self.context, self.gas.as_deref_mut(), self.is_static, slot).get()
    }

    fn set_gas_constraint_backlog(
        &mut self,
        index: u64,
        backlog: u64,
    ) -> Result<(), ArbosStateError> {
        let slot = self.gas_constraint_slot(index, 2);
        StorageBackedU64::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
            .set(backlog)
    }

    fn set_gas_constraint(
        &mut self,
        index: u64,
        constraint: GasConstraint,
    ) -> Result<(), ArbosStateError> {
        for (offset, value) in [
            (0, constraint.target),
            (1, constraint.adjustment_window),
            (2, constraint.backlog),
        ] {
            let slot = self.gas_constraint_slot(index, offset);
            StorageBackedU64::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
                .set(value)?;
        }
        Ok(())
    }

    pub fn replace_gas_constraints(
        &mut self,
        constraints: &[GasConstraint],
    ) -> Result<(), ArbosStateError> {
        let old_len = self.gas_constraints_len()?;
        for index in 0..old_len {
            self.set_gas_constraint(
                index,
                GasConstraint {
                    target: 0,
                    adjustment_window: 0,
                    backlog: 0,
                },
            )?;
        }
        let length_slot = self.gas_constraints_length_slot();
        StorageBackedU64::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            length_slot,
        )
        .set(constraints.len() as u64)?;
        for (index, constraint) in constraints.iter().copied().enumerate() {
            self.set_gas_constraint(index as u64, constraint)?;
        }
        Ok(())
    }

    pub fn grow_backlog(
        &mut self,
        gas_used: u64,
        arbos_version: u64,
    ) -> Result<(), ArbosStateError> {
        let len = self.gas_constraints_len()?;
        if arbos_version >= 50 && len > 0 {
            for index in 0..len {
                let backlog = self.gas_constraint_backlog(index)?;
                self.set_gas_constraint_backlog(index, backlog.saturating_add(gas_used))?;
            }
            Ok(())
        } else {
            let backlog = self.gas_backlog().get()?.saturating_add(gas_used);
            self.gas_backlog().set(backlog)
        }
    }

    /// Removes gas donated to a scheduled retry from the active
    /// single-dimensional backlog. The retry attempt adds its actual usage
    /// back when it executes.
    pub fn shrink_backlog(&mut self, gas: u64, arbos_version: u64) -> Result<(), ArbosStateError> {
        let len = self.gas_constraints_len()?;
        if arbos_version >= 50 && len > 0 {
            for index in 0..len {
                let backlog = self.gas_constraint_backlog(index)?;
                self.set_gas_constraint_backlog(index, backlog.saturating_sub(gas))?;
            }
            Ok(())
        } else {
            let backlog = self.gas_backlog().get()?.saturating_sub(gas);
            self.gas_backlog().set(backlog)
        }
    }

    /// Nitro's prepaid storage cost for `ShrinkBacklog`. ArbOS 60 uses a
    /// fixed single-slot cost even though its multidimensional accounting is
    /// intentionally not represented by this crate.
    pub fn backlog_update_cost(&mut self, arbos_version: u64) -> Result<u64, ArbosStateError> {
        const READ: u64 = 800;
        const WRITE: u64 = 20_000;
        if arbos_version >= 60 {
            return Ok(READ + WRITE);
        }

        let mut cost = if arbos_version >= 50 { READ } else { 0 };
        if arbos_version >= 51 {
            let len = self.gas_constraints_len()?;
            if len > 0 {
                return Ok(cost
                    .saturating_add(READ)
                    .saturating_add(len.saturating_mul(READ + WRITE)));
            }
        }
        cost = cost.saturating_add(READ + WRITE);
        Ok(cost)
    }

    pub fn update_pricing_model(
        &mut self,
        time_passed: u64,
        arbos_version: u64,
    ) -> Result<(), ArbosStateError> {
        let len = self.gas_constraints_len()?;
        if arbos_version < 50 || len == 0 {
            return self.update_legacy_pricing_model(time_passed);
        }

        let mut total_exponent = 0u64;
        for index in 0..len {
            let mut constraint = self.gas_constraint(index)?;
            constraint.backlog = constraint
                .backlog
                .saturating_sub(time_passed.saturating_mul(constraint.target));
            if constraint.backlog > 0 {
                let divisor = constraint
                    .adjustment_window
                    .saturating_mul(constraint.target)
                    .min(i64::MAX as u64);
                if divisor == 0 {
                    return Err(ArbosStateError::Context(
                        "constraint target and adjustment window must be nonzero".into(),
                    ));
                }
                let exponent = constraint
                    .backlog
                    .min(i64::MAX as u64)
                    .saturating_mul(ONE_IN_BIPS)
                    / divisor;
                total_exponent = total_exponent.saturating_add(exponent).min(i64::MAX as u64);
            }
            self.set_gas_constraint_backlog(index, constraint.backlog)?;
        }

        let minimum = self.min_base_fee_wei().get()?;
        let base_fee = if total_exponent == 0 {
            minimum
        } else {
            minimum.saturating_mul(U256::from(approx_exp_basis_points(total_exponent)))
                / U256::from(ONE_IN_BIPS)
        };
        self.base_fee_wei().set(base_fee)
    }

    /// Applies Nitro's legacy single-dimensional pricing update used before
    /// the multidimensional pricer: shrink backlog by elapsed capacity and
    /// recompute the base fee with its quartic basis-point approximation.
    pub fn update_legacy_pricing_model(&mut self, time_passed: u64) -> Result<(), ArbosStateError> {
        let speed_limit = self.speed_limit_per_second().get()?;
        let backlog = self
            .gas_backlog()
            .get()?
            .saturating_sub(time_passed.saturating_mul(speed_limit));
        self.gas_backlog().set(backlog)?;

        let minimum = self.min_base_fee_wei().get()?;
        let tolerance = self.backlog_tolerance().get()?;
        let threshold = tolerance.saturating_mul(speed_limit);
        let base_fee = if backlog > threshold {
            let inertia = self.pricing_inertia().get()?;
            let divisor = inertia.saturating_mul(speed_limit).min(i64::MAX as u64);
            if divisor == 0 {
                return Err(ArbosStateError::Context(
                    "pricing inertia and speed limit must be nonzero".into(),
                ));
            }
            let excess = backlog.saturating_sub(threshold).min(i64::MAX as u64);
            let exponent_bips = excess.saturating_mul(ONE_IN_BIPS).min(i64::MAX as u64) / divisor;
            minimum.saturating_mul(U256::from(approx_exp_basis_points(exponent_bips)))
                / U256::from(ONE_IN_BIPS)
        } else {
            minimum
        };
        self.base_fee_wei().set(base_fee)
    }
}
