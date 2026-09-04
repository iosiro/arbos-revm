use revm::{
    context::{Block, Cfg, JournalTr},
    interpreter::{Gas, gas::COLD_ACCOUNT_ACCESS_COST},
    primitives::{Address, B256, Bytes, U256, address},
    state::Bytecode,
};

use crate::{
    ArbitrumContextTr,
    config::ArbitrumConfigTr,
    constants::{
        ARBOS_50_MAX_STACK_DEPTH, ARBOS_60_MAX_WASM_SIZE, ARBOS_CHAIN_CONFIG_KEY,
        ARBOS_CHAIN_OWNERS_KEY, ARBOS_STATE_ADDRESS, ARBOS_STATE_ADDRESS_TABLE_KEY,
        ARBOS_STATE_BLOCKHASHES_KEY, ARBOS_STATE_FEATURES_KEY, ARBOS_STATE_L1_PRICING_KEY,
        ARBOS_STATE_L2_PRICING_KEY, ARBOS_STATE_NATIVE_TOKEN_OWNER_KEY, ARBOS_STATE_PROGRAMS_KEY,
        ARBOS_STATE_RETRYABLES_KEY, ARBOS_STATE_SEND_MERKLE_KEY,
        ARBOS_STATE_TRANSACTION_FILTERER_KEY, ARBOS_VERSION_COLLECT_TIPS,
        INITIAL_MAX_FRAGMENT_COUNT, MAX_ARBOS_VERSION_SUPPORTED,
    },
    state::{
        address_table::AddressTable,
        block_hashes::BlockHashes,
        filtered_transactions::FilteredTransactions,
        l1_pricing::L1Pricing,
        l2_pricing::L2Pricing,
        merkle_accumulator::MerkleAccumulator,
        program::{DataPricerParams, Programs, StylusParams},
        retryable::{Retryable, RetryableState},
        types::{
            ArbosStateError, StorageBackedAddress, StorageBackedAddressSet, StorageBackedBytes,
            StorageBackedQueue, StorageBackedTr, StorageBackedU64, StorageBackedU256, map_address,
            substorage,
        },
    },
};

const ARBOS_STATE_VERSION_OFFSET: u8 = 0;
const ARBOS_STATE_UPGRADE_VERSION_OFFSET: u8 = 1;
const ARBOS_STATE_UPGRADE_TIMESTAMP_OFFSET: u8 = 2;
const ARBOS_STATE_NETWORK_FEE_ACCOUNT_OFFSET: u8 = 3;
const ARBOS_STATE_CHAIN_ID_OFFSET: u8 = 4;
const ARBOS_STATE_GENESIS_BLOCK_NUM_OFFSET: u8 = 5;
const ARBOS_STATE_INFRA_FEE_ACCOUNT_OFFSET: u8 = 6;
const ARBOS_STATE_BROTLI_COMPRESSION_LEVEL_OFFSET: u8 = 7;
const ARBOS_STATE_NATIVE_TOKEN_ENABLED_FROM_TIME_OFFSET: u8 = 8;
const ARBOS_STATE_TRANSACTION_FILTERING_ENABLED_FROM_TIME_OFFSET: u8 = 9;
const ARBOS_STATE_FILTERED_FUNDS_RECIPIENT_OFFSET: u8 = 10;
const ARBOS_STATE_COLLECT_TIPS_OFFSET: u8 = 11;

/// Nitro's `PrecompileMinArbOSVersions`. Solidity checks that high-level call
/// targets have code, so Nitro installs a one-byte INVALID placeholder when
/// each ArbOS precompile becomes active.
const PRECOMPILE_ACTIVATION_VERSIONS: &[(Address, u64)] = &[
    (address!("0000000000000000000000000000000000000064"), 0),
    (address!("0000000000000000000000000000000000000065"), 0),
    (address!("0000000000000000000000000000000000000066"), 0),
    (address!("0000000000000000000000000000000000000067"), 0),
    (address!("0000000000000000000000000000000000000068"), 0),
    (address!("0000000000000000000000000000000000000069"), 0),
    (address!("000000000000000000000000000000000000006b"), 0),
    (address!("000000000000000000000000000000000000006c"), 0),
    (address!("000000000000000000000000000000000000006d"), 0),
    (address!("000000000000000000000000000000000000006e"), 0),
    (address!("000000000000000000000000000000000000006f"), 0),
    (address!("0000000000000000000000000000000000000070"), 0),
    (address!("0000000000000000000000000000000000000071"), 30),
    (address!("0000000000000000000000000000000000000072"), 30),
    (address!("0000000000000000000000000000000000000073"), 41),
    (address!("0000000000000000000000000000000000000074"), 60),
    (address!("00000000000000000000000000000000000000ff"), 0),
    (address!("00000000000000000000000000000000000a4b05"), 0),
];

fn state_slot(offset: u8) -> B256 {
    map_address(&B256::ZERO, &B256::from(U256::from(offset as u64)))
}

fn state_subkey(key: &[u8]) -> B256 {
    substorage(&B256::ZERO, key)
}

pub trait ArbStateGetter<CTX: ArbitrumContextTr> {
    fn arbos_version(&mut self) -> StorageBackedU64<'_, CTX>;
    fn programs(&mut self) -> Programs<'_, CTX>;
    fn chain_owners<'b>(&'b mut self) -> StorageBackedAddressSet<'b, CTX>;
    fn native_token_owners<'b>(&'b mut self) -> StorageBackedAddressSet<'b, CTX>;
    fn transaction_filterers<'b>(&'b mut self) -> StorageBackedAddressSet<'b, CTX>;
    fn filtered_transactions<'b>(&'b mut self) -> FilteredTransactions<'b, CTX>;
    fn is_chain_owner(&mut self, address: Address) -> Result<bool, ArbosStateError>;
    fn is_native_token_owner(&mut self, address: Address) -> Result<bool, ArbosStateError>;
    fn code_hash(&mut self, address: Address) -> Result<B256, ArbosStateError>;
    fn upgrade_timestamp(&mut self) -> StorageBackedU64<'_, CTX>;
    fn upgrade_version(&mut self) -> StorageBackedU64<'_, CTX>;
    fn network_fee_account(&mut self) -> StorageBackedAddress<'_, CTX>;
    fn infra_fee_account(&mut self) -> StorageBackedAddress<'_, CTX>;
    fn chain_id(&mut self) -> StorageBackedU256<'_, CTX>;
    fn genesis_block_num(&mut self) -> StorageBackedU64<'_, CTX>;
    fn brotli_compression_level(&mut self) -> StorageBackedU64<'_, CTX>;
    fn native_token_enabled_time(&mut self) -> StorageBackedU64<'_, CTX>;
    fn transaction_filtering_enabled_time(&mut self) -> StorageBackedU64<'_, CTX>;
    fn filtered_funds_recipient(&mut self) -> StorageBackedAddress<'_, CTX>;
    fn filtered_funds_recipient_or_default(&mut self) -> Result<Address, ArbosStateError>;
    fn collect_tips(&mut self) -> Result<bool, ArbosStateError>;
    fn set_collect_tips(&mut self, collect: bool) -> Result<(), ArbosStateError>;
    fn address_table(&mut self) -> AddressTable<'_, CTX>;
    fn l1_pricing(&mut self) -> L1Pricing<'_, CTX>;
    fn l2_pricing(&mut self) -> L2Pricing<'_, CTX>;
    fn retryable_state(&mut self) -> RetryableState<'_, CTX>;
    fn retryable<'b>(&'b mut self, id: B256) -> Retryable<'b, CTX>;
    fn timeout_queue(&mut self) -> StorageBackedQueue<'_, CTX>;
    fn features(&mut self) -> StorageBackedU256<'_, CTX>;
    fn chain_config(&mut self) -> StorageBackedBytes<'_, CTX>;
    fn blockhashes(&mut self) -> BlockHashes<'_, CTX>;
    fn send_merkle(&mut self) -> MerkleAccumulator<'_, CTX>;
}

pub trait ArbState<'a, CTX: ArbitrumContextTr> {
    type ArbStateGetterType: ArbStateGetter<CTX>;
    fn arb_state(
        &'a mut self,
        gas: Option<&'a mut Gas>,
        is_static: bool,
    ) -> Self::ArbStateGetterType;
}

impl<'a, CTX: ArbitrumContextTr + 'a> ArbState<'a, CTX> for CTX {
    type ArbStateGetterType = ArbStateWrapper<'a, CTX>;
    fn arb_state(
        &'a mut self,
        gas: Option<&'a mut Gas>,
        is_static: bool,
    ) -> Self::ArbStateGetterType {
        ArbStateWrapper::new(self, gas, is_static)
    }
}

pub struct ArbStateWrapper<'a, CTX: ArbitrumContextTr> {
    context: &'a mut CTX,
    gas: Option<&'a mut Gas>,
    is_static: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArbosStateParams {
    pub arbos_version: u64,
    pub initial_chain_owner: Address,
    pub upgrade_version: u64,
    pub upgrade_timestamp: u64,
    pub network_fee_account: Address,
    pub infra_fee_account: Address,
    pub chain_id: U256,
    pub genesis_block_num: u64,
    pub brotli_compression_level: u64,
    pub native_token_enabled_time: u64,
    pub transaction_filtering_enabled_time: u64,
    pub filtered_funds_recipient: Address,
    pub collect_tips: bool,
    pub l1_reward_recipient: Address,
    pub initial_l1_base_fee: U256,
    pub stylus_params: StylusParams,
    pub data_pricer_params: DataPricerParams,
}

impl Default for ArbosStateParams {
    fn default() -> Self {
        Self::for_arbos_version(crate::constants::INITIAL_ARBOS_VERSION)
    }
}

impl ArbosStateParams {
    pub fn for_arbos_version(arbos_version: u64) -> Self {
        Self {
            arbos_version,
            initial_chain_owner: Address::ZERO,
            upgrade_version: 0,
            upgrade_timestamp: 0,
            network_fee_account: Address::ZERO,
            infra_fee_account: Address::ZERO,
            chain_id: U256::ZERO,
            genesis_block_num: 0,
            brotli_compression_level: u64::from(arbos_version >= 20),
            native_token_enabled_time: 0,
            transaction_filtering_enabled_time: 0,
            filtered_funds_recipient: Address::ZERO,
            collect_tips: false,
            l1_reward_recipient: Address::ZERO,
            initial_l1_base_fee: U256::from(50_000_000_000u64),
            stylus_params: StylusParams::for_arbos_version(arbos_version),
            data_pricer_params: DataPricerParams::default(),
        }
    }
}

impl<'a, CTX: ArbitrumContextTr> ArbStateWrapper<'a, CTX> {
    pub fn new(context: &'a mut CTX, mut gas: Option<&'a mut Gas>, is_static: bool) -> Self {
        if let Err(err) = context
            .journal_mut()
            .load_account_mut(ARBOS_STATE_ADDRESS)
            .map(|mut account| {
                if account.nonce() == 0 {
                    account.set_nonce(1);
                }
            })
        {
            // Consume all gas so downstream callers see a consistent failure state rather than a
            // panic.
            if let Some(gas) = gas.as_deref_mut() {
                gas.spend_all();
            }
            let _ = err;
        }
        Self {
            context,
            gas,
            is_static,
        }
    }
}

impl<'a, CTX> ArbStateWrapper<'a, CTX>
where
    CTX: ArbitrumContextTr,
{
    fn install_precompile_code_through(
        &mut self,
        arbos_version: u64,
    ) -> Result<(), ArbosStateError> {
        for &(address, activation_version) in PRECOMPILE_ACTIVATION_VERSIONS {
            if activation_version > arbos_version {
                continue;
            }
            self.context
                .journal_mut()
                .load_account(address)
                .map_err(|_| ArbosStateError::Context("load precompile account".into()))?;
            self.context
                .journal_mut()
                .set_code(address, Bytecode::new_raw(Bytes::from_static(&[0xfe])));
        }
        Ok(())
    }

    fn install_precompile_code_at(&mut self, arbos_version: u64) -> Result<(), ArbosStateError> {
        for &(address, activation_version) in PRECOMPILE_ACTIVATION_VERSIONS {
            if activation_version != arbos_version {
                continue;
            }
            self.context
                .journal_mut()
                .load_account(address)
                .map_err(|_| ArbosStateError::Context("load precompile account".into()))?;
            self.context
                .journal_mut()
                .set_code(address, Bytecode::new_raw(Bytes::from_static(&[0xfe])));
        }
        Ok(())
    }

    pub fn initialize(&mut self, params: &ArbosStateParams) -> Result<(), ArbosStateError> {
        if params.arbos_version > MAX_ARBOS_VERSION_SUPPORTED {
            return Err(ArbosStateError::UnsupportedArbosVersion(
                params.arbos_version,
            ));
        }
        self.install_precompile_code_through(params.arbos_version)?;
        self.arbos_version().set(params.arbos_version)?;
        self.upgrade_version().set(params.upgrade_version)?;
        self.upgrade_timestamp().set(params.upgrade_timestamp)?;
        let network_fee_account =
            if params.arbos_version >= 2 && params.network_fee_account.is_zero() {
                params.initial_chain_owner
            } else {
                params.network_fee_account
            };
        self.network_fee_account().set(network_fee_account)?;
        self.infra_fee_account().set(params.infra_fee_account)?;
        self.chain_id().set(params.chain_id)?;
        self.genesis_block_num().set(params.genesis_block_num)?;
        self.brotli_compression_level()
            .set(params.brotli_compression_level)?;
        self.native_token_enabled_time()
            .set(params.native_token_enabled_time)?;
        self.transaction_filtering_enabled_time()
            .set(params.transaction_filtering_enabled_time)?;
        self.filtered_funds_recipient()
            .set(params.filtered_funds_recipient)?;
        self.set_collect_tips(params.collect_tips)?;
        if !params.initial_chain_owner.is_zero() {
            self.chain_owners().add(params.initial_chain_owner)?;
        }
        let reward_recipient = if params.l1_reward_recipient.is_zero() {
            if params.arbos_version >= 2 {
                params.initial_chain_owner
            } else {
                crate::constants::ARBOS_BATCH_POSTER_ADDRESS
            }
        } else {
            params.l1_reward_recipient
        };
        self.l1_pricing().initialize(
            params.arbos_version,
            reward_recipient,
            params.initial_l1_base_fee,
        )?;
        self.l2_pricing().initialize(params.arbos_version)?;
        self.timeout_queue().initialize()?;

        self.programs()
            .initialize(&params.stylus_params, &params.data_pricer_params)?;

        Ok(())
    }

    /// Applies a scheduled ArbOS upgrade when its flag day has been reached.
    /// Returns whether state was upgraded. The caller should use the persisted
    /// version when constructing the execution context for subsequent transactions.
    pub fn upgrade_arbos_version_if_necessary(
        &mut self,
        current_timestamp: u64,
    ) -> Result<bool, ArbosStateError> {
        let current = self.arbos_version().get()?;
        let upgrade_to = self.upgrade_version().get()?;
        let flag_day = self.upgrade_timestamp().get()?;
        if current >= upgrade_to || current_timestamp < flag_day {
            return Ok(false);
        }
        self.upgrade_arbos_version(upgrade_to)?;
        Ok(true)
    }

    /// Applies Nitro's state-changing upgrade steps that do not depend on
    /// multidimensional gas accounting.
    pub fn upgrade_arbos_version(&mut self, upgrade_to: u64) -> Result<(), ArbosStateError> {
        let mut current = self.arbos_version().get()?;
        if upgrade_to > MAX_ARBOS_VERSION_SUPPORTED {
            return Err(ArbosStateError::UnsupportedArbosVersion(upgrade_to));
        }
        if upgrade_to < current {
            return Err(ArbosStateError::ArbosVersionDowngrade {
                current,
                requested: upgrade_to,
            });
        }

        while current < upgrade_to {
            let next = current + 1;
            match next {
                30 => {
                    self.programs().initialize(
                        &StylusParams::for_arbos_version(30),
                        &DataPricerParams::default(),
                    )?;
                }
                31 => {
                    let mut params = self.programs().stylus_params().get()?;
                    if params.version != 1 {
                        return Err(ArbosStateError::UnexpectedStylusVersion {
                            current: params.version,
                            requested: 2,
                        });
                    }
                    params.version = 2;
                    params.min_init_gas = crate::constants::STYLUS_V2_MIN_INIT_GAS;
                    self.programs().stylus_params().set(&params)?;
                }
                40 => {
                    let mut params = self.programs().stylus_params().get()?;
                    if params.version != 2 {
                        return Err(ArbosStateError::UnexpectedStylusVersion {
                            current: params.version,
                            requested: 2,
                        });
                    }
                    params.max_wasm_size = crate::constants::INITIAL_MAX_WASM_SIZE;
                    self.programs().stylus_params().set(&params)?;
                }
                50 => {
                    let mut params = self.programs().stylus_params().get()?;
                    params.max_stack_depth = params.max_stack_depth.min(ARBOS_50_MAX_STACK_DEPTH);
                    self.programs().stylus_params().set(&params)?;
                    self.l2_pricing().per_tx_gas_limit().set(32_000_000)?;
                }
                59 => {
                    let mut params = self.programs().stylus_params().get()?;
                    if params.version != 2 {
                        return Err(ArbosStateError::UnexpectedStylusVersion {
                            current: params.version,
                            requested: 3,
                        });
                    }
                    params.version = 3;
                    self.programs().stylus_params().set(&params)?;
                }
                60 => {
                    let mut params = self.programs().stylus_params().get()?;
                    params.max_wasm_size = ARBOS_60_MAX_WASM_SIZE;
                    params.max_fragment_count = INITIAL_MAX_FRAGMENT_COUNT;
                    self.programs().stylus_params().set(&params)?;
                }
                // ArbOS 61 only changes multidimensional-gas refunds. Other
                // versions in this range either have no state transition or
                // are reserved for Orbit chains.
                _ => {}
            }
            self.install_precompile_code_at(next)?;
            self.arbos_version().set(next)?;
            current = next;
        }
        Ok(())
    }

    pub fn get(&mut self) -> Result<ArbosStateParams, ArbosStateError> {
        // Read values from storage
        let mut params = ArbosStateParams {
            arbos_version: self.arbos_version().get()?,
            initial_chain_owner: self
                .chain_owners()
                .all()?
                .first()
                .copied()
                .unwrap_or_default(),
            upgrade_version: self.upgrade_version().get()?,
            upgrade_timestamp: self.upgrade_timestamp().get()?,
            network_fee_account: self.network_fee_account().get()?,
            infra_fee_account: self.infra_fee_account().get()?,
            chain_id: self.chain_id().get()?,
            genesis_block_num: self.genesis_block_num().get()?,
            brotli_compression_level: self.brotli_compression_level().get()?,
            native_token_enabled_time: self.native_token_enabled_time().get()?,
            transaction_filtering_enabled_time: self.transaction_filtering_enabled_time().get()?,
            filtered_funds_recipient: self.filtered_funds_recipient().get()?,
            collect_tips: self.collect_tips()?,
            l1_reward_recipient: self.l1_pricing().reward_recipient().get()?,
            initial_l1_base_fee: self.l1_pricing().price_per_unit().get()?,
            stylus_params: self.programs().stylus_params().get()?,
            data_pricer_params: self.programs().data_pricer().get()?,
        };

        // If values are default/zero, populate from context without writing to storage.
        // This enables lazy initialization - state is only written when explicitly set.
        if params.upgrade_timestamp == 0 {
            params.upgrade_timestamp = self.context.block().timestamp().saturating_to::<u64>();
        }
        if params.chain_id == U256::ZERO {
            params.chain_id = U256::from(self.context.cfg().chain_id());
        }
        // genesis_block_num: 0 is a valid value (genesis block), so don't override
        // brotli_compression_level: default of 1 is set in ArbosStateParams::default()

        Ok(params)
    }
}

impl<'a, CTX> ArbStateGetter<CTX> for ArbStateWrapper<'a, CTX>
where
    CTX: ArbitrumContextTr,
{
    fn arbos_version(&mut self) -> StorageBackedU64<'_, CTX> {
        StorageBackedU64::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            state_slot(ARBOS_STATE_VERSION_OFFSET),
        )
    }
    fn programs(&mut self) -> Programs<'_, CTX> {
        Programs::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            state_subkey(ARBOS_STATE_PROGRAMS_KEY),
        )
    }

    fn brotli_compression_level(&mut self) -> StorageBackedU64<'_, CTX> {
        StorageBackedU64::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            state_slot(ARBOS_STATE_BROTLI_COMPRESSION_LEVEL_OFFSET),
        )
    }
    fn native_token_enabled_time(&mut self) -> StorageBackedU64<'_, CTX> {
        StorageBackedU64::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            state_slot(ARBOS_STATE_NATIVE_TOKEN_ENABLED_FROM_TIME_OFFSET),
        )
    }
    fn transaction_filtering_enabled_time(&mut self) -> StorageBackedU64<'_, CTX> {
        StorageBackedU64::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            state_slot(ARBOS_STATE_TRANSACTION_FILTERING_ENABLED_FROM_TIME_OFFSET),
        )
    }
    fn filtered_funds_recipient(&mut self) -> StorageBackedAddress<'_, CTX> {
        StorageBackedAddress::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            state_slot(ARBOS_STATE_FILTERED_FUNDS_RECIPIENT_OFFSET),
        )
    }
    fn filtered_funds_recipient_or_default(&mut self) -> Result<Address, ArbosStateError> {
        let recipient = self.filtered_funds_recipient().get()?;
        if recipient.is_zero() {
            self.network_fee_account().get()
        } else {
            Ok(recipient)
        }
    }
    fn collect_tips(&mut self) -> Result<bool, ArbosStateError> {
        if self.context.cfg().arbos_version() < ARBOS_VERSION_COLLECT_TIPS {
            return Ok(false);
        }
        let value = StorageBackedU64::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            state_slot(ARBOS_STATE_COLLECT_TIPS_OFFSET),
        )
        .get()?;
        Ok(value != 0)
    }
    fn set_collect_tips(&mut self, collect: bool) -> Result<(), ArbosStateError> {
        StorageBackedU64::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            state_slot(ARBOS_STATE_COLLECT_TIPS_OFFSET),
        )
        .set(u64::from(collect))
    }
    fn infra_fee_account(&mut self) -> StorageBackedAddress<'_, CTX> {
        StorageBackedAddress::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            state_slot(ARBOS_STATE_INFRA_FEE_ACCOUNT_OFFSET),
        )
    }
    fn genesis_block_num(&mut self) -> StorageBackedU64<'_, CTX> {
        StorageBackedU64::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            state_slot(ARBOS_STATE_GENESIS_BLOCK_NUM_OFFSET),
        )
    }
    fn upgrade_version(&mut self) -> StorageBackedU64<'_, CTX> {
        StorageBackedU64::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            state_slot(ARBOS_STATE_UPGRADE_VERSION_OFFSET),
        )
    }
    fn upgrade_timestamp(&mut self) -> StorageBackedU64<'_, CTX> {
        StorageBackedU64::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            state_slot(ARBOS_STATE_UPGRADE_TIMESTAMP_OFFSET),
        )
    }
    fn chain_id(&mut self) -> StorageBackedU256<'_, CTX> {
        StorageBackedU256::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            state_slot(ARBOS_STATE_CHAIN_ID_OFFSET),
        )
    }

    fn blockhashes(&mut self) -> BlockHashes<'_, CTX> {
        BlockHashes::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            state_subkey(ARBOS_STATE_BLOCKHASHES_KEY),
        )
    }

    fn send_merkle(&mut self) -> MerkleAccumulator<'_, CTX> {
        MerkleAccumulator::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            state_subkey(ARBOS_STATE_SEND_MERKLE_KEY),
        )
    }

    fn chain_owners<'b>(&'b mut self) -> StorageBackedAddressSet<'b, CTX> {
        StorageBackedAddressSet::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            state_subkey(ARBOS_CHAIN_OWNERS_KEY),
        )
    }

    fn chain_config(&mut self) -> StorageBackedBytes<'_, CTX> {
        StorageBackedBytes::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            state_subkey(ARBOS_CHAIN_CONFIG_KEY),
        )
    }

    fn features(&mut self) -> StorageBackedU256<'_, CTX> {
        let slot = map_address(&state_subkey(ARBOS_STATE_FEATURES_KEY), &B256::ZERO);
        StorageBackedU256::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }

    fn native_token_owners<'b>(&'b mut self) -> StorageBackedAddressSet<'b, CTX> {
        StorageBackedAddressSet::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            state_subkey(ARBOS_STATE_NATIVE_TOKEN_OWNER_KEY),
        )
    }
    fn transaction_filterers<'b>(&'b mut self) -> StorageBackedAddressSet<'b, CTX> {
        StorageBackedAddressSet::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            state_subkey(ARBOS_STATE_TRANSACTION_FILTERER_KEY),
        )
    }
    fn filtered_transactions<'b>(&'b mut self) -> FilteredTransactions<'b, CTX> {
        FilteredTransactions::new(self.context, self.gas.as_deref_mut(), self.is_static)
    }

    fn is_chain_owner(&mut self, address: Address) -> Result<bool, ArbosStateError> {
        self.chain_owners().contains(address)
    }

    fn is_native_token_owner(&mut self, address: Address) -> Result<bool, ArbosStateError> {
        self.native_token_owners().contains(address)
    }

    fn address_table(&mut self) -> AddressTable<'_, CTX> {
        AddressTable::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            state_subkey(ARBOS_STATE_ADDRESS_TABLE_KEY),
        )
    }

    fn l1_pricing(&mut self) -> L1Pricing<'_, CTX> {
        L1Pricing::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            state_subkey(ARBOS_STATE_L1_PRICING_KEY),
        )
    }

    fn l2_pricing(&mut self) -> L2Pricing<'_, CTX> {
        L2Pricing::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            state_subkey(ARBOS_STATE_L2_PRICING_KEY),
        )
    }

    fn retryable_state(&mut self) -> RetryableState<'_, CTX> {
        RetryableState::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            state_subkey(ARBOS_STATE_RETRYABLES_KEY),
        )
    }

    fn retryable<'b>(&'b mut self, id: B256) -> Retryable<'b, CTX> {
        let root = state_subkey(ARBOS_STATE_RETRYABLES_KEY);
        Retryable::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            substorage(&root, id.as_slice()),
        )
    }

    fn timeout_queue(&mut self) -> StorageBackedQueue<'_, CTX> {
        let root = state_subkey(ARBOS_STATE_RETRYABLES_KEY);
        let slot = substorage(&root, &[0]);
        StorageBackedQueue::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
    }

    fn code_hash(&mut self, address: Address) -> Result<B256, ArbosStateError> {
        let code_hash = self
            .context
            .load_account_code_hash(address)
            .map(|s| s.data)
            .unwrap_or_default();
        if let Some(gas) = self.gas.as_deref_mut()
            && !gas.record_cost(COLD_ACCOUNT_ACCESS_COST)
        {
            return Err(ArbosStateError::OutOfGas);
        }
        Ok(code_hash)
    }

    fn network_fee_account(&mut self) -> StorageBackedAddress<'_, CTX> {
        StorageBackedAddress::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            state_slot(ARBOS_STATE_NETWORK_FEE_ACCOUNT_OFFSET),
        )
    }
}
