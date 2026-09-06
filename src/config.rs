use std::ops::{Deref, DerefMut};

use auto_impl::auto_impl;
use revm::{
    context::{Cfg, CfgEnv},
    primitives::hardfork::SpecId,
};

use crate::constants::INITIAL_ARBOS_VERSION;

/// Returns the Ethereum execution specification activated by an ArbOS version.
///
/// These thresholds mirror Nitro's Arbitrum-specific fork selection in
/// `params.ChainConfig`: Shanghai at ArbOS 11, Cancun at 20, Prague at 40, and
/// Osaka at 50. Earlier supported versions use the Merge specification.
pub const fn spec_id_for_arbos_version(arbos_version: u64) -> SpecId {
    match arbos_version {
        50.. => SpecId::OSAKA,
        40..=49 => SpecId::PRAGUE,
        20..=39 => SpecId::CANCUN,
        11..=19 => SpecId::SHANGHAI,
        _ => SpecId::MERGE,
    }
}

#[auto_impl(&mut, Box)]
pub trait ArbitrumConfigTr: Cfg {
    fn arbos_version(&self) -> u64;
    /// Refresh the live execution version after a persisted ArbOS upgrade.
    fn set_arbos_version(&mut self, version: u64);
    fn debug_mode(&self) -> bool;
    fn disable_auto_cache(&self) -> bool;
    fn disable_auto_activate(&self) -> bool;
    fn disable_stylus_deployment(&self) -> bool {
        false
    }
    /// Whether the embedding explicitly disabled EIP-3541 validation.
    fn is_eip3541_explicitly_disabled(&self) -> bool {
        false
    }
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
    pub disable_stylus_deployment: bool,
}

impl<SPEC> ArbitrumConfig<SPEC> {
    pub fn new_with_spec(spec: SPEC) -> Self
    where
        SPEC: Into<SpecId> + Copy,
    {
        Self {
            inner: CfgEnv::new_with_spec(spec),
            arbos_version: INITIAL_ARBOS_VERSION,
            debug_mode: false,
            disable_auto_cache: false,
            disable_auto_activate: false,
            disable_stylus_deployment: false,
        }
    }
}

impl<SPEC> Default for ArbitrumConfig<SPEC>
where
    SPEC: Into<SpecId> + Copy + Default,
{
    fn default() -> Self {
        Self {
            inner: CfgEnv::default(),
            arbos_version: INITIAL_ARBOS_VERSION,
            debug_mode: false,
            disable_auto_cache: false,
            disable_auto_activate: false,
            disable_stylus_deployment: false,
        }
    }
}

impl<SPEC: Into<SpecId> + Copy> Cfg for ArbitrumConfig<SPEC> {
    type Spec = SPEC;

    fn chain_id(&self) -> u64 {
        self.inner.chain_id()
    }

    fn tx_chain_id_check(&self) -> bool {
        self.inner.tx_chain_id_check()
    }

    fn tx_gas_limit_cap(&self) -> u64 {
        self.inner.tx_gas_limit_cap()
    }

    fn spec(&self) -> Self::Spec {
        self.inner.spec()
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
        // REVM's generic check rejects every 0xEF-prefixed deployment. ArbOS
        // applies the narrower check in its frame runner so valid Stylus
        // components can be deployed.
        true
    }

    fn is_balance_check_disabled(&self) -> bool {
        self.inner.is_balance_check_disabled()
    }

    fn is_block_gas_limit_disabled(&self) -> bool {
        self.inner.is_block_gas_limit_disabled()
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

    fn memory_limit(&self) -> u64 {
        self.inner.memory_limit()
    }
}

impl<SPEC> ArbitrumConfigTr for ArbitrumConfig<SPEC>
where
    SPEC: Into<SpecId> + Copy + Copy,
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

    fn disable_stylus_deployment(&self) -> bool {
        self.disable_stylus_deployment
    }

    fn is_eip3541_explicitly_disabled(&self) -> bool {
        self.inner.is_eip3541_disabled()
    }
}

impl<SPEC: Into<SpecId> + Copy> ArbitrumConfig<SPEC> {
    pub fn new(inner: CfgEnv<SPEC>) -> Self {
        Self {
            inner,
            arbos_version: INITIAL_ARBOS_VERSION,
            debug_mode: false,
            disable_auto_cache: false,
            disable_auto_activate: false,
            disable_stylus_deployment: false,
        }
    }
}

impl<SPEC> Deref for ArbitrumConfig<SPEC>
where
    SPEC: Into<SpecId> + Copy,
{
    type Target = CfgEnv<SPEC>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<SPEC> DerefMut for ArbitrumConfig<SPEC>
where
    SPEC: Into<SpecId> + Copy,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::spec_id_for_arbos_version;
    use revm::primitives::hardfork::SpecId;

    #[test]
    fn arbos_versions_select_nitro_execution_specs() {
        let cases = [
            (10, SpecId::MERGE),
            (11, SpecId::SHANGHAI),
            (19, SpecId::SHANGHAI),
            (20, SpecId::CANCUN),
            (39, SpecId::CANCUN),
            (40, SpecId::PRAGUE),
            (49, SpecId::PRAGUE),
            (50, SpecId::OSAKA),
            (61, SpecId::OSAKA),
        ];

        for (arbos_version, expected) in cases {
            assert_eq!(spec_id_for_arbos_version(arbos_version), expected);
        }
    }
}
