use std::ops::{Deref, DerefMut};

use auto_impl::auto_impl;
use revm::{
    context::{Cfg, CfgEnv},
    context_interface::cfg::GasParams,
    primitives::hardfork::SpecId,
};

use crate::constants::INITIAL_ARBOS_VERSION;

#[auto_impl(&mut, Box)]
pub trait ArbitrumConfigTr: Cfg {
    fn arbos_version(&self) -> u64;
    /// Refresh the live execution version after a persisted ArbOS upgrade.
    fn set_arbos_version(&mut self, version: u64);
    fn debug_mode(&self) -> bool;
    fn disable_auto_cache(&self) -> bool;
    fn disable_auto_activate(&self) -> bool;
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ArbitrumConfig<SPEC = SpecId> {
    pub inner: CfgEnv<SPEC>,

    pub arbos_version: u64,
    pub debug_mode: bool,
    pub disable_auto_cache: bool,
    pub disable_auto_activate: bool,
}

impl<SPEC> ArbitrumConfig<SPEC> {
    pub fn new_with_spec(spec: SPEC) -> Self
    where
        SPEC: Into<SpecId> + Clone,
    {
        Self {
            inner: CfgEnv::new_with_spec(spec),
            arbos_version: INITIAL_ARBOS_VERSION,
            debug_mode: false,
            disable_auto_cache: false,
            disable_auto_activate: false,
        }
    }
}

impl<SPEC> Default for ArbitrumConfig<SPEC>
where
    SPEC: Into<SpecId> + Clone + Default,
{
    fn default() -> Self {
        Self {
            inner: CfgEnv::default(),
            arbos_version: INITIAL_ARBOS_VERSION,
            debug_mode: false,
            disable_auto_cache: false,
            disable_auto_activate: false,
        }
    }
}

impl<SPEC: Into<SpecId> + Clone> Cfg for ArbitrumConfig<SPEC> {
    type Spec = SPEC;

    fn chain_id(&self) -> u64 {
        self.inner.chain_id()
    }

    fn tx_chain_id_check(&self) -> bool {
        self.inner.tx_chain_id_check()
    }

    fn tx_gas_limit_cap(&self) -> u64 {
        // ArbOS applies its state-configured per-transaction/per-block gas
        // constraints in the handler. Ethereum's EIP-7825 cap must not reject
        // an Arbitrum transaction before those rules run.
        u64::MAX
    }

    fn spec(&self) -> Self::Spec {
        self.inner.spec().clone()
    }

    fn max_blobs_per_tx(&self) -> Option<u64> {
        self.inner.max_blobs_per_tx()
    }

    fn max_code_size(&self) -> usize {
        self.inner.max_code_size()
    }

    fn max_initcode_size(&self) -> usize {
        self.inner.max_initcode_size()
    }

    fn is_eip3607_disabled(&self) -> bool {
        self.inner.is_eip3607_disabled()
    }

    fn is_eip3541_disabled(&self) -> bool {
        // Stylus programs deliberately use the 0xEF-prefixed ArbOS bytecode envelope. Ethereum's
        // EIP-3541 rejection would prevent those programs from being deployed before ArbOS can
        // decode and activate them.
        true
    }

    fn is_balance_check_disabled(&self) -> bool {
        self.inner.is_balance_check_disabled()
    }

    fn is_block_gas_limit_disabled(&self) -> bool {
        // ArbOS applies its state-configured per-block gas constraint in the
        // handler, including cumulative gas from earlier transactions in the
        // same block. REVM's Ethereum block limit would reject transactions
        // before that versioned ArbOS logic can cap their execution allowance.
        true
    }

    fn is_nonce_check_disabled(&self) -> bool {
        self.inner.is_nonce_check_disabled()
    }

    fn is_base_fee_check_disabled(&self) -> bool {
        self.inner.is_base_fee_check_disabled()
    }

    fn is_priority_fee_check_disabled(&self) -> bool {
        self.inner.is_priority_fee_check_disabled()
    }

    fn is_fee_charge_disabled(&self) -> bool {
        self.inner.is_fee_charge_disabled()
    }

    fn is_eip7623_disabled(&self) -> bool {
        self.inner.is_eip7623_disabled()
    }

    fn is_eip7708_disabled(&self) -> bool {
        self.inner.is_eip7708_disabled()
    }

    fn is_eip8246_delayed_clear_disabled(&self) -> bool {
        self.inner.is_eip8246_delayed_clear_disabled()
    }

    fn gas_params(&self) -> &GasParams {
        self.inner.gas_params()
    }

    fn is_amsterdam_eip8037_enabled(&self) -> bool {
        self.inner.is_amsterdam_eip8037_enabled()
    }

    fn is_amsterdam_eip2780_enabled(&self) -> bool {
        self.inner.is_amsterdam_eip2780_enabled()
    }

    fn memory_limit(&self) -> u64 {
        self.inner.memory_limit()
    }
}

impl<SPEC> ArbitrumConfigTr for ArbitrumConfig<SPEC>
where
    SPEC: Into<SpecId> + Clone,
{
    fn arbos_version(&self) -> u64 {
        self.arbos_version
    }

    fn set_arbos_version(&mut self, version: u64) {
        self.arbos_version = version;
    }

    fn debug_mode(&self) -> bool {
        self.debug_mode
    }

    fn disable_auto_cache(&self) -> bool {
        self.disable_auto_cache
    }

    fn disable_auto_activate(&self) -> bool {
        self.disable_auto_activate
    }
}

impl<SPEC: Into<SpecId> + Clone> ArbitrumConfig<SPEC> {
    pub fn new(inner: CfgEnv<SPEC>) -> Self {
        Self {
            inner,
            arbos_version: INITIAL_ARBOS_VERSION,
            debug_mode: false,
            disable_auto_cache: false,
            disable_auto_activate: false,
        }
    }
}

impl<SPEC> From<CfgEnv<SPEC>> for ArbitrumConfig<SPEC>
where
    SPEC: Into<SpecId> + Clone,
{
    fn from(inner: CfgEnv<SPEC>) -> Self {
        Self::new(inner)
    }
}

impl<SPEC> From<ArbitrumConfig<SPEC>> for CfgEnv<SPEC>
where
    SPEC: Into<SpecId> + Clone,
{
    fn from(config: ArbitrumConfig<SPEC>) -> Self {
        config.inner
    }
}

impl<SPEC> Deref for ArbitrumConfig<SPEC>
where
    SPEC: Into<SpecId> + Clone,
{
    type Target = CfgEnv<SPEC>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<SPEC> DerefMut for ArbitrumConfig<SPEC>
where
    SPEC: Into<SpecId> + Clone,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}
