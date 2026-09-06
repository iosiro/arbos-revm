use std::{
    fmt::{self, Debug},
    sync::Arc,
};

use revm::{
    context::{Cfg, ContextTr, LocalContextTr},
    handler::PrecompileProvider,
    interpreter::{
        CallInput, CallInputs, Gas, InstructionResult, InterpreterResult, gas::ISTANBUL_SLOAD_GAS,
    },
    precompile::{PrecompileError, PrecompileId, PrecompileSpecId, Precompiles},
    primitives::{
        Address, Bytes, HashMap, HashSet, SHORT_ADDRESS_CAP, U256, hardfork::SpecId, short_address,
    },
};

use crate::state::{ArbState, ArbStateGetter};

mod arb_address_table;
mod arb_aggregator;
mod arb_bls;
mod arb_debug;
mod arb_filtered_transactions_manager;
mod arb_function_table;
mod arb_gas_info;
mod arb_info;
mod arb_native_token_manager;
mod arb_owner;
mod arb_owner_public;
mod arb_retryable_tx;
mod arb_statistics;
mod arb_sys;
pub mod arb_wasm;
mod arb_wasm_cache;
mod arbos_test;

use crate::{
    ArbitrumContextTr,
    config::ArbitrumConfigTr,
    macros::interpreter_revert,
    precompiles::{arb_wasm::arb_wasm_precompile, arb_wasm_cache::arb_wasm_cache_precompile},
    try_record_cost,
};

macro_rules! selector_or_revert {
    ($gas:expr, $input:expr) => {{
        if $input.len() < 4 {
            $gas.spend_all();
            return Some(crate::macros::interpreter_result_revert_with_output(
                &mut $gas,
                revm::primitives::Bytes::new(),
            ));
        }

        match <[u8; 4]>::try_from(&$input[0..4]) {
            Ok(selector) => selector,
            Err(_) => {
                $gas.spend_all();
                return Some(crate::macros::interpreter_result_revert_with_output(
                    &mut $gas,
                    revm::primitives::Bytes::new(),
                ));
            }
        }
    }};
}

pub(crate) use selector_or_revert;

pub struct ArbitrumPrecompileProvider<CTX: ArbitrumContextTr> {
    registry: Arc<PrecompileRegistry<CTX>>,
    spec: SpecId,
}

impl<CTX: ArbitrumContextTr> ArbitrumPrecompileProvider<CTX> {
    #[inline]
    pub fn warm_addresses(&self) -> Box<impl Iterator<Item = Address>> {
        Box::new(self.registry.address_iter().copied())
    }

    #[inline]
    pub fn contains(&self, addr: &Address) -> bool {
        self.registry.contains(addr)
    }

    pub fn new(spec: SpecId) -> Self {
        let mut registry = PrecompileRegistry::new(PrecompileSpecId::from_spec_id(spec));

        registry.register_many([
            // Arbitrum specific precompiles can be added here
            Precompile::Extended(arb_address_table::arb_address_table_precompile::<CTX>()),
            Precompile::Extended(arb_aggregator::arb_aggregator_precompile::<CTX>()),
            Precompile::Extended(arb_bls::arb_bls_precompile::<CTX>()),
            Precompile::Extended(arb_debug::arb_debug_precompile::<CTX>()),
            Precompile::Extended(
                arb_filtered_transactions_manager::arb_filtered_transactions_manager_precompile::<
                    CTX,
                >(),
            ),
            Precompile::Extended(arb_function_table::arb_function_table_precompile::<CTX>()),
            Precompile::Extended(arbos_test::arbos_test_precompile::<CTX>()),
            Precompile::Extended(arb_gas_info::arb_gas_info_precompile::<CTX>()),
            Precompile::Extended(arb_info::arb_info_precompile::<CTX>()),
            Precompile::Extended(
                arb_native_token_manager::arb_native_token_manager_precompile::<CTX>(),
            ),
            Precompile::Extended(arb_owner_public::arb_owner_public_precompile::<CTX>()),
            Precompile::Extended(arb_owner::arb_owner_precompile::<CTX>()),
            Precompile::Extended(arb_retryable_tx::arb_retryable_tx_precompile::<CTX>()),
            Precompile::Extended(arb_statistics::arb_statistics_precompile::<CTX>()),
            Precompile::Extended(arb_sys::arb_sys_precompile::<CTX>()),
            Precompile::Extended(arb_wasm_precompile::<CTX>()),
            Precompile::Extended(arb_wasm_cache_precompile::<CTX>()),
        ]);
        Self {
            registry: Arc::new(registry),
            spec,
        }
    }
}

impl<CTX: ArbitrumContextTr> Clone for ArbitrumPrecompileProvider<CTX> {
    fn clone(&self) -> Self {
        Self {
            registry: Arc::clone(&self.registry),
            spec: self.spec,
        }
    }
}

impl<CTX: ArbitrumContextTr> Default for ArbitrumPrecompileProvider<CTX> {
    fn default() -> Self {
        Self::new(SpecId::default())
    }
}

impl<CTX: ArbitrumContextTr> PrecompileProvider<CTX> for ArbitrumPrecompileProvider<CTX> {
    type Output = InterpreterResult;

    /// Update spec and regenerate registry.
    fn set_spec(&mut self, spec: <CTX::Cfg as Cfg>::Spec) -> bool {
        let new_spec = spec.into();
        if new_spec == self.spec {
            return false;
        }

        *self = Self::new(new_spec);
        true
    }

    /// Executes a precompile if one exists for the address.
    fn run(
        &mut self,
        ctx: &mut CTX,
        inputs: &CallInputs,
    ) -> Result<Option<InterpreterResult>, String> {
        let Some(precompile) = self.registry.get(&inputs.bytecode_address) else {
            return Ok(None);
        };

        // extract input bytes
        let input_bytes = match &inputs.input {
            CallInput::SharedBuffer(range) => ctx
                .local()
                .shared_memory_buffer_slice(range.clone())
                .map(|s| s.to_vec())
                .unwrap_or_default(),
            CallInput::Bytes(b) => b.to_vec(),
        };

        // Nitro permits DELEGATECALL/CALLCODE only for pure methods. View and
        // state-mutating methods must execute against their registered address.
        if inputs.target_address != inputs.bytecode_address
            && precompile
                .state_mutability(&input_bytes)
                .is_some_and(|purity| purity >= StateMutability::View)
        {
            return Ok(Some(InterpreterResult {
                result: InstructionResult::Revert,
                output: Bytes::default(),
                gas: Gas::new_spent(inputs.gas_limit),
            }));
        }

        precompile.call(
            ctx,
            &input_bytes,
            &inputs.target_address,
            inputs.caller,
            inputs.call_value(),
            inputs.is_static,
            inputs.gas_limit,
        )
    }

    fn warm_addresses(&self) -> Box<impl Iterator<Item = Address>> {
        self.warm_addresses()
    }

    fn contains(&self, address: &Address) -> bool {
        self.contains(address)
    }
}

#[derive(Clone, Debug)]
pub struct PrecompileRegistry<CTX: ContextTr> {
    map: HashMap<Address, Precompile<CTX>>,
    address_set: HashSet<Address>,
    fast_lookup: Vec<Option<Precompile<CTX>>>,
    all_short: bool,
}

impl<CTX: ContextTr> Default for PrecompileRegistry<CTX> {
    fn default() -> Self {
        Self {
            map: HashMap::default(),
            address_set: HashSet::default(),
            fast_lookup: vec![None; SHORT_ADDRESS_CAP],
            all_short: true,
        }
    }
}

impl<CTX: ContextTr> PrecompileRegistry<CTX> {
    pub fn new(spec: PrecompileSpecId) -> Self {
        let mut reg = Self::default();

        let precompiles = Precompiles::new(spec);

        precompiles.addresses().for_each(|addr| {
            if let Some(p) = precompiles.get(addr) {
                reg.register(Precompile::Simple(p.clone()));
            }
        });

        reg
    }

    #[inline]
    pub fn contains(&self, address: &Address) -> bool {
        self.map.contains_key(address)
    }

    #[inline]
    pub fn get(&self, address: &Address) -> Option<&Precompile<CTX>> {
        if let Some(idx) = short_address(address) {
            return self.fast_lookup[idx].as_ref();
        }
        self.map.get(address)
    }

    #[inline]
    pub fn address_iter(&self) -> impl ExactSizeIterator<Item = &Address> {
        self.map.keys()
    }

    pub fn register(&mut self, item: Precompile<CTX>) {
        self.register_many(std::iter::once(item));
    }

    pub fn register_many(&mut self, items: impl IntoIterator<Item = Precompile<CTX>>) {
        let collected: Vec<_> = items.into_iter().collect();

        for p in &collected {
            if let Some(short) = short_address(p.address()) {
                self.fast_lookup[short] = Some(p.clone());
            } else {
                self.all_short = false;
            }
            self.address_set.insert(*p.address());
        }

        for p in collected.into_iter() {
            self.map.insert(*p.address(), p);
        }
    }

    pub fn difference(&self, other: &Self) -> Self {
        let mut out = Self::default();

        let missing = self
            .map
            .iter()
            .filter(|(addr, _)| !other.map.contains_key(*addr))
            .map(|(_, p)| p.clone());

        out.register_many(missing);
        out
    }

    pub fn intersection(&self, other: &Self) -> Self {
        let mut out = Self::default();

        let common = self
            .map
            .iter()
            .filter(|(addr, _)| other.map.contains_key(*addr))
            .map(|(_, p)| p.clone());

        out.register_many(common);
        out
    }
}

pub struct ExtendedPrecompile<CTX: ContextTr> {
    id: PrecompileId,
    address: Address,
    handler: Arc<ExtendedPrecompileFn<CTX>>,
    mutability: ExtendedPrecompileMutabilityFn,
}

impl<CTX: ContextTr> Clone for ExtendedPrecompile<CTX> {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            address: self.address,
            handler: Arc::clone(&self.handler),
            mutability: self.mutability,
        }
    }
}

impl<CTX: ContextTr> Debug for ExtendedPrecompile<CTX> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExtendedPrecompile")
            .field("id", &self.id)
            .field("address", &self.address)
            .finish()
    }
}

impl<CTX: ContextTr> ExtendedPrecompile<CTX> {
    pub fn new(
        id: PrecompileId,
        address: Address,
        implementation: ExtendedPrecompileImplementation<CTX>,
    ) -> Self {
        Self {
            id,
            address,
            handler: Arc::new(implementation.handler),
            mutability: implementation.mutability,
        }
    }

    #[inline]
    pub fn id(&self) -> &PrecompileId {
        &self.id
    }

    #[inline]
    pub fn address(&self) -> &Address {
        &self.address
    }

    fn state_mutability(&self, input: &[u8]) -> Option<StateMutability> {
        (self.mutability)(input)
    }

    #[inline]
    #[allow(clippy::too_many_arguments)]
    pub fn execute(
        &self,
        ctx: &mut CTX,
        input: &[u8],
        target: &Address,
        caller: Address,
        value: U256,
        is_static: bool,
        gas_limit: u64,
    ) -> Result<Option<InterpreterResult>, String> {
        (self.handler)(ctx, input, target, caller, value, is_static, gas_limit)
    }
}

pub type ExtendedPrecompileFn<CTX> = fn(
    &mut CTX,
    &[u8],
    &Address,
    Address,
    U256,
    bool,
    u64,
) -> Result<Option<InterpreterResult>, String>;

pub type ExtendedPrecompileMutabilityFn = fn(&[u8]) -> Option<StateMutability>;

pub struct ExtendedPrecompileImplementation<CTX: ContextTr> {
    handler: ExtendedPrecompileFn<CTX>,
    mutability: ExtendedPrecompileMutabilityFn,
}

impl<CTX: ContextTr> ExtendedPrecompileImplementation<CTX> {
    pub fn new(
        handler: ExtendedPrecompileFn<CTX>,
        mutability: ExtendedPrecompileMutabilityFn,
    ) -> Self {
        Self {
            handler,
            mutability,
        }
    }
}

impl<CTX: ArbitrumContextTr> ExtendedPrecompileImplementation<CTX> {
    pub(crate) fn for_logic<Logic: ArbPrecompileLogic<CTX>>() -> Self {
        Self::new(
            |context, input, target_address, caller_address, call_value, is_static, gas_limit| {
                Ok(Logic::run(
                    context,
                    input,
                    target_address,
                    caller_address,
                    call_value,
                    is_static,
                    gas_limit,
                ))
            },
            |input| {
                let selector: [u8; 4] = input.get(..4)?.try_into().ok()?;
                Logic::STATE_MUT_TABLE
                    .iter()
                    .find_map(|(candidate, purity)| (*candidate == selector).then_some(*purity))
            },
        )
    }
}

#[derive(Debug)]
pub enum Precompile<CTX: ContextTr> {
    Simple(revm::precompile::Precompile),
    Extended(ExtendedPrecompile<CTX>),
}

impl<CTX: ContextTr> Clone for Precompile<CTX> {
    fn clone(&self) -> Self {
        match self {
            Self::Simple(p) => Self::Simple(p.clone()),
            Self::Extended(p) => Self::Extended(p.clone()),
        }
    }
}

impl<CTX: ContextTr> Precompile<CTX> {
    #[inline]
    pub fn address(&self) -> &Address {
        match self {
            Self::Simple(p) => p.address(),
            Self::Extended(p) => p.address(),
        }
    }

    fn state_mutability(&self, input: &[u8]) -> Option<StateMutability> {
        match self {
            Self::Simple(_) => None,
            Self::Extended(precompile) => precompile.state_mutability(input),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn call(
        &self,
        ctx: &mut CTX,
        input: &[u8],
        target: &Address,
        caller: Address,
        value: U256,
        is_static: bool,
        gas_limit: u64,
    ) -> Result<Option<InterpreterResult>, String> {
        match self {
            Self::Simple(p) => {
                let raw = p.execute(input, gas_limit);

                let mut result = InterpreterResult {
                    result: InstructionResult::Return,
                    gas: Gas::new(gas_limit),
                    output: Bytes::new(),
                };

                match raw {
                    Ok(output) => {
                        _ = result.gas.record_cost(output.gas_used);
                        result.result = if output.reverted {
                            InstructionResult::Revert
                        } else {
                            InstructionResult::Return
                        };
                        result.output = output.bytes;
                    }
                    Err(PrecompileError::Fatal(e)) => return Err(e),
                    Err(e) => {
                        result.result = if e.is_oog() {
                            InstructionResult::PrecompileOOG
                        } else {
                            InstructionResult::PrecompileError
                        };
                    }
                }

                Ok(Some(result))
            }
            Self::Extended(ext) => {
                ext.execute(ctx, input, target, caller, value, is_static, gas_limit)
            }
        }
    }
}

pub(crate) trait ArbPrecompileLogic<CTX: ArbitrumContextTr> {
    /// First ArbOS version in which this precompile exists. Calls before this
    /// version behave like calls to an empty account and retain all gas.
    const MIN_ARBOS_VERSION: u64 = 0;

    /// Nitro's ArbOwner wrapper refunds all precompile gas when an authorized
    /// owner's underlying method succeeds.
    const REFUND_ALL_ON_SUCCESS: bool = false;

    /// Nitro's transaction-filter manager wrapper charges only its role lookup
    /// to unauthorized callers and makes authorized calls entirely free.
    const FREE_FOR_FILTERERS: bool = false;

    /// File-local state mutability table
    const STATE_MUT_TABLE: &'static [([u8; 4], StateMutability)];

    /// Per-method activation range, inclusive. Nitro treats methods outside
    /// this range as unknown selectors and consumes all supplied gas.
    fn method_version(_selector: [u8; 4]) -> (u64, Option<u64>) {
        (Self::MIN_ARBOS_VERSION, None)
    }

    /// Inner execution
    fn inner(
        context: &mut CTX,
        input: &[u8],
        target_address: &Address,
        caller_address: Address,
        call_value: U256,
        is_static: bool,
        gas_limit: u64,
    ) -> Option<InterpreterResult>;

    fn run(
        context: &mut CTX,
        input: &[u8],
        target_address: &Address,
        caller_address: Address,
        call_value: U256,
        is_static: bool,
        gas_limit: u64,
    ) -> Option<InterpreterResult> {
        let mut gas = Gas::new(gas_limit);

        let arbos_version = context.cfg().arbos_version();
        if arbos_version < Self::MIN_ARBOS_VERSION {
            return Some(crate::macros::interpreter_result_return(&mut gas));
        }

        let selector = selector_or_revert!(gas, input);

        if Self::FREE_FOR_FILTERERS {
            let authorized = match context
                .arb_state(Some(&mut gas), true)
                .transaction_filterers()
                .contains(caller_address)
            {
                Ok(authorized) => authorized,
                Err(_) => interpreter_revert!(gas, Bytes::new()),
            };
            let outcome = Self::inner(
                context,
                input,
                target_address,
                caller_address,
                call_value,
                is_static,
                gas_limit,
            )?;
            return Some(InterpreterResult {
                result: outcome.result,
                gas: if authorized { Gas::new(gas_limit) } else { gas },
                output: outcome.output,
            });
        }

        let purity = match Self::STATE_MUT_TABLE
            .iter()
            .find(|(sel, _)| *sel == selector)
        {
            Some((_, p)) => *p,
            None => {
                gas.spend_all();
                interpreter_revert!(gas, Bytes::new());
            }
        };

        let (minimum, maximum) = Self::method_version(selector);
        if arbos_version < minimum || maximum.is_some_and(|maximum| arbos_version > maximum) {
            gas.spend_all();
            interpreter_revert!(gas, Bytes::new());
        }

        let args_cost =
            revm::interpreter::gas::VERYLOW * (input.len() as u64).saturating_sub(4).div_ceil(32);

        try_record_cost!(gas, args_cost);

        if purity != StateMutability::Pure {
            try_record_cost!(gas, ISTANBUL_SLOAD_GAS);
        }

        if purity >= StateMutability::NonPayable && is_static {
            gas.spend_all();
            interpreter_revert!(gas, Bytes::new());
        }

        if call_value > U256::ZERO && purity < StateMutability::Payable {
            gas.spend_all();
            interpreter_revert!(gas, Bytes::new());
        }

        // call the inner logic
        let outcome = if let Some(outcome) = Self::inner(
            context,
            input,
            target_address,
            caller_address,
            call_value,
            is_static,
            gas.remaining(),
        ) {
            outcome
        } else {
            // final precompile return value must be error
            interpreter_revert!(gas);
        };

        if Self::REFUND_ALL_ON_SUCCESS && outcome.result.is_ok() {
            return Some(InterpreterResult {
                result: outcome.result,
                gas: Gas::new(gas_limit),
                output: outcome.output,
            });
        }

        gas.spend_all();
        gas.erase_cost(outcome.gas.remaining());

        let result_data_cost =
            revm::interpreter::gas::VERYLOW * (outcome.output.len() as u64).div_ceil(32);
        try_record_cost!(gas, result_data_cost);

        Some(InterpreterResult {
            result: outcome.result,
            gas,
            output: outcome.output,
        })
    }
}

macro_rules! decode_call {
    ($gas:expr, $call:path, $input:expr) => {{
        match <$call as alloy_sol_types::SolCall>::abi_decode($input) {
            Ok(value) => value,
            Err(_) => {
                $gas.spend_all();
                return Some(crate::macros::interpreter_result_revert_with_output(
                    &mut $gas,
                    revm::primitives::Bytes::new(),
                ));
            }
        }
    }};
}

pub(crate) use decode_call;

#[macro_export]
macro_rules! generate_state_mut_table {
    (
        $iface:ident => {
            $(
                $call:ident($mut:ident)
            ),* $(,)?
        }
    ) => {{
        const TABLE: &[([u8;4], StateMutability)] = &[
            $(
                (
                    <$iface::$call as alloy_sol_types::SolCall>::SELECTOR,
                    StateMutability::$mut
                )
            ),*
        ];

        TABLE
    }};
}
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub enum StateMutability {
    Pure,
    View,
    NonPayable,
    Payable,
}

#[macro_export]
macro_rules! precompile_impl {
    ($logic:ty) => {
        $crate::precompiles::ExtendedPrecompileImplementation::for_logic::<$logic>()
    };
}
