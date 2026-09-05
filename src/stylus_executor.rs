use std::{
    cmp::max,
    mem,
    num::NonZeroUsize,
    sync::{Arc, Mutex},
};

use arbutil::{
    Bytes20, Bytes32,
    evm::{
        EvmData,
        api::{EvmApiMethod, Gas as ArbGas, Ink, VecReader},
        req::EvmApiRequestor,
        user::{UserOutcome, UserOutcomeKind},
    },
};

use lru::LruCache;
use revm::{
    Inspector,
    context::{Block, Cfg, ContextSetters, ContextTr, JournalTr, LocalContextTr, Transaction},
    handler::{EvmTr, PrecompileProvider, instructions::InstructionProvider},
    inspector::{InspectorEvmTr, JournalExt},
    interpreter::{
        CallInput, FrameInput, Gas, InputsImpl, InstructionResult, InterpreterAction,
        InterpreterResult, gas::memory_gas, interpreter::EthInterpreter,
        interpreter_types::InputsTr,
    },
    primitives::{Address, B256, Bytes, FixedBytes, Log, U256, alloy_primitives::U64},
};
use stylus::{
    brotli::{self, Dictionary},
    native::{self, NativeInstance},
    prover::{
        machine::Module,
        programs::{
            StylusData,
            config::{CompileConfig, PricingParams, StylusConfig},
            meter::MeteredMachine,
        },
    },
    run::RunProgram,
};

use tracing::{debug, trace, warn};

use crate::{
    ArbitrumEvm, Utf8OrHex,
    config::ArbitrumConfigTr,
    constants::{
        ARBOS_VERSION_STYLUS_CONTRACT_LIMIT, ARBOS_VERSION_STYLUS_FIXES, COST_SCALAR_PERCENT,
        MEMORY_EXPONENTS, MIN_CACHED_GAS_UNITS, MIN_INIT_GAS_UNITS, STYLUS_DISCRIMINANT,
        STYLUS_FRAGMENT_DISCRIMINANT, STYLUS_ROOT_DISCRIMINANT,
    },
    context::ArbitrumContextTr,
    local_context::ArbitrumLocalContextTr,
    state::{
        ArbState, ArbStateGetter,
        program::{ProgramInfo, StylusParams},
        types::{ArbosStateError, StorageBackedTr},
    },
    stylus_api::StylusHandler,
};

type ProgramCacheEntry = (Vec<u8>, Module, StylusData);
type ProgramCacheKey = (FixedBytes<32>, u16, u64, bool);

lazy_static::lazy_static! {
    pub static ref PROGRAM_CACHE: Mutex<LruCache<ProgramCacheKey, ProgramCacheEntry>> = Mutex::new(LruCache::new(NonZeroUsize::new(1024).unwrap()));
}

type EvmApiHandler<'a> =
    Arc<Box<dyn Fn(EvmApiMethod, Vec<u8>) -> (Vec<u8>, VecReader, arbutil::evm::api::Gas) + 'a>>;

pub fn build_evm_data<CTX>(
    context: &CTX,
    input: InputsImpl,
    module_hash: Bytes32,
    l1_block_number: u64,
    reentrant: bool,
) -> EvmData
where
    CTX: ArbitrumContextTr,
{
    let config_env = context.cfg();
    let arbos_env = context.cfg();

    let block_env = context.block();
    let tx_env = context.tx();

    let base_fee = block_env.basefee();

    let evm_data: EvmData = EvmData {
        arbos_version: arbos_env.arbos_version(),
        block_basefee: Bytes32::from(U256::from(base_fee).to_be_bytes()),
        chainid: config_env.chain_id(),
        block_coinbase: Bytes20::try_from(block_env.beneficiary().as_slice()).unwrap(),
        block_gas_limit: U64::wrapping_from(block_env.gas_limit()).to::<u64>(),
        block_number: l1_block_number,
        block_timestamp: U64::wrapping_from(block_env.timestamp()).to::<u64>(),
        contract_address: Bytes20::try_from(input.target_address.as_slice()).unwrap(),
        module_hash,
        msg_sender: Bytes20::try_from(input.caller_address.as_slice()).unwrap(),
        msg_value: Bytes32::try_from(input.call_value.to_be_bytes_vec()).unwrap(),
        tx_gas_price: Bytes32::from(
            U256::from(tx_env.effective_gas_price(base_fee as u128)).to_be_bytes(),
        ),
        tx_origin: Bytes20::try_from(tx_env.caller().as_slice()).unwrap(),
        reentrant: u32::from(reentrant),
        return_data_len: 0,
        cached: true,
        tracing: true,
    };

    evm_data
}

// Shared data structure for Stylus execution context
pub(crate) struct StylusExecutionContext {
    target_address: Address,
    bytecode_address: Address,
    caller_address: Address,
    call_value: revm::primitives::U256,
    is_static: bool,
    gas_limit: u64,
    calldata: Bytes,
}

pub fn stylus_call_cost(new: u16, open: u16, ever: u16, free_pages: u16, page_gas: u16) -> u64 {
    let new_open = open.saturating_add(new);
    let new_ever = max(ever, new_open);

    if new_ever <= free_pages {
        return 0;
    }

    let sub_free = |pages: u16| pages.saturating_sub(free_pages);

    let adding = sub_free(new_open).saturating_sub(sub_free(open));
    let linear = (adding as u64).saturating_mul(page_gas as u64);
    let exp = |x: u16| -> u64 {
        if x < MEMORY_EXPONENTS.len() as u16 {
            return MEMORY_EXPONENTS[x as usize] as u64;
        }

        u64::MAX
    };

    let expand = exp(new_ever) - exp(ever);

    linear.saturating_add(expand)
}

pub fn init_gas_cost(init_cost: u16, min_init_gas: u8, init_cost_scaler: u8) -> u64 {
    let base = min_init_gas as u64 * MIN_INIT_GAS_UNITS;
    let dyno = (init_cost as u64).saturating_mul(init_cost_scaler as u64 * COST_SCALAR_PERCENT);
    base.saturating_add(dyno.div_ceil(100))
}

pub fn cached_gas_cost(
    cached_init_cost: u16,
    min_cached_init_gas: u8,
    cached_init_cost_scaler: u8,
) -> u64 {
    let base = min_cached_init_gas as u64 * MIN_CACHED_GAS_UNITS;
    let dyno = (cached_init_cost as u64)
        .saturating_mul(cached_init_cost_scaler as u64 * COST_SCALAR_PERCENT);
    base.saturating_add(dyno.div_ceil(100))
}

pub(crate) fn stylus_page_limit_penalty(arbos_version: u64, page_limit: u16, new_open: u16) -> u64 {
    if arbos_version >= 59 && page_limit > 0 && new_open > page_limit {
        u64::MAX
    } else {
        0
    }
}

fn fragment_read_gas_cost(is_cold: bool, code_size: u64) -> Option<u64> {
    let access = if is_cold { 2_600_u64 } else { 100_u64 };
    let words = code_size.checked_add(31)? / 32;
    access.checked_add(words.checked_mul(3)?)
}

fn restore_open_pages_on_error<T, E>(
    local: &mut impl ArbitrumLocalContextTr,
    previous_open: u16,
    result: Result<T, E>,
) -> Result<T, E> {
    if result.is_err() {
        local.set_stylus_pages_open(previous_open);
    }
    result
}

impl<CTX, INSP, P, I> ArbitrumEvm<CTX, INSP, P, I>
where
    CTX: ArbitrumContextTr,
    I: InstructionProvider<Context = CTX, InterpreterTypes = EthInterpreter>,
    P: PrecompileProvider<CTX, Output = InterpreterResult>,
{
    /// Common method to build API requestor for both inspected and non-inspected modes
    fn build_api_requestor(
        &mut self,
        input: InputsImpl,
        is_static: bool,
        request_handler: impl Fn(
            &mut Self,
            InputsImpl,
            bool,
            EvmApiMethod,
            Vec<u8>,
        ) -> (Vec<u8>, VecReader, ArbGas),
    ) -> EvmApiRequestor<VecReader, StylusHandler> {
        let evm = Arc::new(Mutex::new(self));

        let callback = {
            let evm = evm.clone();

            move |req_type: arbutil::evm::api::EvmApiMethod,
                  req_data: Vec<u8>|
                  -> (Vec<u8>, VecReader, arbutil::evm::api::Gas) {
                let mut evm = evm.lock().unwrap();
                request_handler(&mut evm, input.clone(), is_static, req_type, req_data)
            }
        };

        let callback: EvmApiHandler<'_> = Arc::new(Box::new(callback));
        let unsafe_callback: &'static EvmApiHandler<'_> = unsafe { mem::transmute(&callback) };
        EvmApiRequestor::new(StylusHandler::new(unsafe_callback.clone()))
    }

    /// Extract common Stylus execution context from frame input
    fn extract_stylus_context(&mut self) -> Option<(StylusExecutionContext, B256)> {
        let frame_input = {
            let frame = self.frame_stack().get();
            match frame.input {
                FrameInput::Call(ref input) => input.clone(),
                _ => return None,
            }
        };

        let bytecode_address = frame_input.bytecode_address;

        let code_hash = {
            let ctx = self.ctx();
            if let Ok(code_hash) = ctx.journal_mut().code_hash(bytecode_address) {
                code_hash.data
            } else {
                return None;
            }
        };

        let calldata = match &frame_input.input {
            CallInput::Bytes(calldata) => calldata.clone(),
            CallInput::SharedBuffer(range) => {
                if let Some(slice) = self.ctx().local().shared_memory_buffer_slice(range.clone()) {
                    Bytes::from(slice.to_vec())
                } else {
                    Bytes::new()
                }
            }
        };

        let context = StylusExecutionContext {
            target_address: frame_input.target_address,
            bytecode_address,
            caller_address: frame_input.caller,
            call_value: frame_input.value.get(),
            is_static: frame_input.is_static,
            gas_limit: frame_input.gas_limit,
            calldata,
        };

        Some((context, code_hash))
    }

    /// Core Stylus execution logic shared between inspected and non-inspected modes
    pub(crate) fn execute_stylus_program(
        &mut self,
        stylus_ctx: StylusExecutionContext,
        code_hash: B256,
        reentrant: bool,
        api_request_handler: impl Fn(
            &mut Self,
            InputsImpl,
            bool,
            EvmApiMethod,
            Vec<u8>,
        ) -> (Vec<u8>, VecReader, ArbGas),
    ) -> Option<InterpreterAction> {
        debug!(
            target: "arbos-revm::stylus",
            bytecode_address = %stylus_ctx.bytecode_address,
            target_address = %stylus_ctx.target_address,
            caller_address = %stylus_ctx.caller_address,
            gas_limit = stylus_ctx.gas_limit,
            is_static = stylus_ctx.is_static,
            calldata_len = stylus_ctx.calldata.len(),
            "Executing Stylus program",
        );
        let mut gas = Gas::new(stylus_ctx.gas_limit);

        let stylus_params = match self
            .ctx()
            .arb_state(None, true)
            .programs()
            .stylus_params()
            .get()
        {
            Ok(params) => params,
            Err(e) => return Some(e.into()),
        };
        let arbos_version = self.ctx().cfg().arbos_version();
        let debug = self.ctx().cfg().debug_mode();
        let cache_key = (code_hash, stylus_params.version, arbos_version, debug);

        let (serialized, _module, stylus_data, stylus_params) = {
            // Use read lock to get cached program if available
            // if not available drop the read lock and acquire write lock to compile and insert
            let maybe_cached = {
                let mut cache = PROGRAM_CACHE.lock().unwrap();
                if let Some((serialized, module, stylus_data)) = cache.get(&cache_key).cloned() {
                    trace!(
                        target: "arbos-revm::stylus",
                        code_hash = %code_hash,
                        "Using cached Stylus program"
                    );
                    Some((serialized, module, stylus_data))
                } else {
                    None
                }
            };

            if let Some((serialized, module, stylus_data)) = maybe_cached {
                (serialized, module, stylus_data, stylus_params)
            } else {
                let context = self.ctx();

                let bytecode = context
                    .journal_mut()
                    .code(stylus_ctx.bytecode_address)
                    .ok()?
                    .data;

                let bytecode = match stylus_code_with_fragments(
                    context,
                    &bytecode,
                    &stylus_params,
                    false,
                    None,
                ) {
                    Ok(Some(code)) => code,
                    Ok(None) => return None,
                    Err(e) => {
                        debug!(
                            target: "arbos-revm::stylus",
                            bytecode_address = %stylus_ctx.bytecode_address,
                            error = %String::from_utf8_or_hex(e.clone()),
                            "Stylus bytecode decode failed"
                        );
                        return Some(InterpreterAction::Return(InterpreterResult {
                            result: InstructionResult::Revert,
                            output: e.into(),
                            gas,
                        }));
                    }
                };

                let compile_config = CompileConfig::version(stylus_params.version, debug);

                let mut cache = PROGRAM_CACHE.lock().unwrap();
                match cache.try_get_or_insert::<_, String>(cache_key, || {
                    let serialized = stylus_compile(&bytecode, &compile_config)?;

                    let (module, stylus_data) = stylus_activate(
                        None,
                        &bytecode,
                        code_hash,
                        arbos_version,
                        stylus_params.version,
                        stylus_params.page_limit,
                        debug,
                    )?;

                    Ok((serialized, module, stylus_data))
                }) {
                    Ok((serialized, module, stylus_data)) => (
                        serialized.clone(),
                        module.clone(),
                        *stylus_data,
                        stylus_params,
                    ),
                    Err(e) => {
                        warn!(
                            target: "arbos-revm::stylus",
                            bytecode_address = %stylus_ctx.bytecode_address,
                            target_address = %stylus_ctx.target_address,
                            error = %e,
                            "Stylus compile/activate failed"
                        );
                        return Some(InterpreterAction::Return(InterpreterResult {
                            result: InstructionResult::Revert,
                            output: e.as_bytes().to_vec().into(),
                            gas,
                        }));
                    }
                }
            }
        };

        let (stylus_config, compile_config, evm_data) = {
            let context = self.ctx();

            let stylus_config = StylusConfig::new(
                stylus_params.version,
                stylus_params.max_stack_depth,
                stylus_params.ink_price,
            );

            let compile_config =
                CompileConfig::version(stylus_params.version, context.cfg().debug_mode());

            let module_hash = context
                .arb_state(None, true)
                .programs()
                .module_hash(&code_hash)
                .get()
                .map(|hash| Bytes32::from(hash.0))
                .unwrap_or_default();
            let l1_block_number = context
                .arb_state(None, true)
                .blockhashes()
                .l1_block_number()
                .get()
                .unwrap_or_else(|_| context.block().number().to::<u64>());

            let evm_data = build_evm_data(
                self.ctx(),
                InputsImpl {
                    target_address: stylus_ctx.target_address,
                    caller_address: stylus_ctx.caller_address,
                    input: CallInput::Bytes(stylus_ctx.calldata.clone()),
                    call_value: stylus_ctx.call_value,
                    bytecode_address: Some(stylus_ctx.target_address),
                },
                module_hash,
                l1_block_number,
                reentrant,
            );

            (stylus_config, compile_config, evm_data)
        };

        let program_info = match self
            .ctx()
            .arb_state(None, false)
            .programs()
            .get_active_program(&stylus_params, &code_hash)
        {
            Ok(info) => info,
            Err(e) => {
                if !self.ctx().cfg().disable_auto_activate() {
                    trace!(
                        target: "arbos-revm::stylus",
                        bytecode_address = %stylus_ctx.bytecode_address,
                        code_hash = %code_hash,
                        "Program not active, building auto-activation info"
                    );
                    ProgramInfo {
                        version: stylus_params.version,
                        init_cost: stylus_data.init_cost,
                        cached_cost: stylus_data.cached_init_cost,
                        footprint: stylus_data.footprint,
                        asm_estimated_kb: stylus_data.asm_estimate,
                        age: 0,
                        cached: !self.ctx().cfg().disable_auto_cache(),
                    }
                } else {
                    match e {
                        ArbosStateError::ProgramNotActivated => {
                            debug!(
                                target: "arbos-revm::stylus",
                                bytecode_address = %stylus_ctx.bytecode_address,
                                code_hash = %code_hash,
                                "Stylus program not activated"
                            );
                            gas.spend_all();
                            return Some(InterpreterAction::Return(InterpreterResult {
                                result: InstructionResult::OutOfGas,
                                output: ArbosStateError::ProgramNotActivated.into(),
                                gas,
                            }));
                        }
                        _ => {
                            debug!(
                                target: "arbos-revm::stylus",
                                bytecode_address = %stylus_ctx.bytecode_address,
                                code_hash = %code_hash,
                                error = ?e,
                                "Stylus program unavailable"
                            );
                            return Some(InterpreterAction::Return(InterpreterResult {
                                result: InstructionResult::Revert,
                                output: e.into(),
                                gas,
                            }));
                        }
                    }
                }
            }
        };

        let recent_cache_hit =
            if self.ctx().cfg().arbos_version() >= ARBOS_VERSION_STYLUS_CONTRACT_LIMIT {
                let block_number = self.ctx().block().number().saturating_to();
                self.ctx().local_mut().insert_recent_wasm(
                    code_hash,
                    stylus_params.block_cache_size,
                    block_number,
                )
            } else {
                false
            };
        let cached = program_info.cached || recent_cache_hit;

        let inputs = InputsImpl {
            target_address: stylus_ctx.target_address,
            caller_address: stylus_ctx.caller_address,
            input: CallInput::Bytes(Bytes::from(stylus_ctx.calldata.to_vec())),
            call_value: stylus_ctx.call_value,
            bytecode_address: Some(stylus_ctx.target_address),
        };

        let (call_cost, stylus_open_pages) = {
            let context = self.ctx();
            let wasm_open_pages = context.local().stylus_pages_open();

            let page_grow_cost = stylus_call_cost(
                stylus_data.footprint,
                wasm_open_pages,
                context.local().stylus_pages_ever(),
                stylus_params.free_pages,
                stylus_params.page_gas,
            );

            let program_cost = cached_gas_cost(
                stylus_data.cached_init_cost,
                stylus_params.min_cached_init_gas,
                stylus_params.cached_cost_scalar,
            );

            let init_cost = init_gas_cost(
                stylus_data.init_cost,
                stylus_params.min_init_gas,
                stylus_params.init_cost_scalar,
            );

            let mut cost = page_grow_cost;
            if cached || stylus_params.version > 1 {
                cost = cost.saturating_add(program_cost);
            }
            if !cached {
                cost = cost.saturating_add(init_cost);
            }
            cost = cost.saturating_add(stylus_page_limit_penalty(
                arbos_version,
                stylus_params.page_limit,
                wasm_open_pages.saturating_add(stylus_data.footprint),
            ));

            (cost, wasm_open_pages)
        };

        if !gas.record_cost(call_cost) {
            debug!(
                target: "arbos-revm::stylus",
                bytecode_address = %stylus_ctx.bytecode_address,
                call_cost,
                gas_limit = gas.limit(),
                "Not enough gas to begin Stylus program"
            );
            return Some(InterpreterAction::Return(InterpreterResult {
                result: InstructionResult::OutOfGas,
                output: Default::default(),
                gas: Default::default(),
            }));
        }

        {
            self.ctx()
                .local_mut()
                .add_stylus_pages_open(stylus_data.footprint);
        }

        let evm_api =
            self.build_api_requestor(inputs.clone(), stylus_ctx.is_static, api_request_handler);

        let deserialized = unsafe {
            NativeInstance::deserialize(serialized.as_slice(), compile_config, evm_api, evm_data)
        };
        let deserialized =
            restore_open_pages_on_error(self.ctx().local_mut(), stylus_open_pages, deserialized);
        let mut instance = match deserialized {
            Ok(instance) => instance,
            Err(err) => {
                warn!(
                    target: "arbos-revm::stylus",
                    bytecode_address = %stylus_ctx.bytecode_address,
                    error = %err,
                    "Failed to deserialize cached Stylus program"
                );
                return Some(InterpreterAction::Return(InterpreterResult {
                    result: InstructionResult::Revert,
                    output: err.to_string().into_bytes().into(),
                    gas,
                }));
            }
        };

        let gas_before_stylus = gas.remaining();
        let ink_limit = stylus_config
            .pricing
            .gas_to_ink(arbutil::evm::api::Gas(gas_before_stylus));
        gas.spend_all();

        let bytecode = match inputs.input() {
            CallInput::Bytes(bytes) => bytes.clone(),
            CallInput::SharedBuffer(range) => self
                .ctx()
                .local()
                .shared_memory_buffer_slice(range.clone())
                .map(|slice| Bytes::copy_from_slice(&slice))
                .unwrap_or_default(),
        };

        let outcome = match instance.run_main(&bytecode, stylus_config, ink_limit) {
            Err(e) | Ok(UserOutcome::Failure(e)) => {
                debug!(
                    target: "arbos-revm::stylus",
                    bytecode_address = %stylus_ctx.bytecode_address,
                    target_address = %stylus_ctx.target_address,
                    error = ?e,
                    "Stylus runtime returned failure"
                );
                UserOutcome::Failure(e.wrap_err("call failed"))
            }
            Ok(outcome) => outcome,
        };

        let ink_left: Ink = instance.ink_left().into();
        let ink_used = ink_limit.0.saturating_sub(ink_left.0);
        let gas_used_by_wasm = ink_to_gas_ceil(stylus_config.pricing, Ink(ink_used));
        let mut gas_left = gas_before_stylus.saturating_sub(gas_used_by_wasm);

        let (kind, data) = outcome.into_data();

        trace!(
            target: "arbos-revm::stylus",
            bytecode_address = %stylus_ctx.bytecode_address,
            target_address = %stylus_ctx.target_address,
            outcome = ?kind,
            output_len = data.len(),
            output = %String::from_utf8_or_hex(data.clone()),
            ink_left = ink_left.0,
            gas_left,
            "Stylus program finished"
        );

        let (result, output) = match kind {
            UserOutcomeKind::Success => (
                revm::interpreter::InstructionResult::Return,
                Bytes::from(data),
            ),
            UserOutcomeKind::Revert => (
                revm::interpreter::InstructionResult::Revert,
                Bytes::from(data),
            ),
            UserOutcomeKind::Failure => {
                (revm::interpreter::InstructionResult::Revert, Bytes::new())
            }
            UserOutcomeKind::OutOfInk => (
                revm::interpreter::InstructionResult::OutOfGas,
                Bytes::from(data),
            ),
            UserOutcomeKind::OutOfStack => {
                gas_left = 0;
                (
                    revm::interpreter::InstructionResult::StackOverflow,
                    Bytes::from(data),
                )
            }
        };

        gas.erase_cost(gas_left);

        self.ctx()
            .local_mut()
            .set_stylus_pages_open(stylus_open_pages);

        if !output.is_empty() && self.ctx().cfg().arbos_version() >= ARBOS_VERSION_STYLUS_FIXES {
            let evm_cost = memory_gas(output.len().div_ceil(32));

            if gas.limit() < evm_cost {
                debug!(
                    target: "arbos-revm::stylus",
                    bytecode_address = %stylus_ctx.bytecode_address,
                    target_address = %stylus_ctx.target_address,
                    output_len = output.len(),
                    evm_cost,
                    gas_limit = gas.limit(),
                    "Not enough gas to return Stylus output"
                );
                gas.spend_all();
                return Some(InterpreterAction::Return(InterpreterResult {
                    result: InstructionResult::OutOfGas,
                    output: Default::default(),
                    gas,
                }));
            }

            let max_gas_to_return = gas.limit().saturating_sub(evm_cost);
            // gas.remaining = min(gas.remaining, max_gas_to_return)
            // gas.spent = gas.limit - gas.remaining
            if gas.remaining() > max_gas_to_return {
                gas.set_spent(gas.limit() - max_gas_to_return);
            }
        }
        Some(InterpreterAction::Return(InterpreterResult {
            result,
            output,
            gas,
        }))
    }

    pub fn frame_run_stylus(&mut self) -> Option<InterpreterAction> {
        let (stylus_ctx, code_hash) = self.extract_stylus_context()?;
        let address = stylus_ctx.target_address;
        let reentrant = self.ctx().local_mut().enter_stylus(address);
        let result = self.execute_stylus_program(
            stylus_ctx,
            code_hash,
            reentrant,
            |evm, inputs, is_static, req_type, data| evm.request(inputs, is_static, req_type, data),
        );
        self.ctx().local_mut().exit_stylus(address);
        result
    }
}

impl<CTX, INSP, P, I> ArbitrumEvm<CTX, INSP, P, I>
where
    CTX: ArbitrumContextTr,
    CTX::Journal: JournalExt,
    I: InstructionProvider<Context = CTX, InterpreterTypes = EthInterpreter>,
    P: PrecompileProvider<CTX, Output = InterpreterResult>,
    CTX: ContextSetters,
    INSP: Inspector<CTX>,
{
    pub fn inspect_frame_run_stylus(&mut self) -> Option<InterpreterAction> {
        let (stylus_ctx, code_hash) = self.extract_stylus_context()?;
        let address = stylus_ctx.target_address;
        let reentrant = self.ctx().local_mut().enter_stylus(address);
        let result = self.execute_stylus_program(
            stylus_ctx,
            code_hash,
            reentrant,
            |evm, inputs, is_static, req_type, data| {
                evm.inspect_request(inputs, is_static, req_type, data)
            },
        );
        self.ctx().local_mut().exit_stylus(address);
        result
    }

    pub(crate) fn inspect_request(
        &mut self,
        input: InputsImpl,
        is_static: bool,
        req_type: EvmApiMethod,
        data: Vec<u8>,
    ) -> (Vec<u8>, VecReader, ArbGas) {
        match req_type {
            EvmApiMethod::ContractCall | EvmApiMethod::DelegateCall | EvmApiMethod::StaticCall => {
                self.handle_contract_call(input, is_static, req_type, data, |evm, frame_init| {
                    evm.inspect_run_exec_loop(frame_init)
                })
            }

            EvmApiMethod::Create1 | EvmApiMethod::Create2 => self.handle_contract_creation(
                input,
                is_static,
                req_type,
                data,
                |evm, frame_init| evm.inspect_run_exec_loop(frame_init),
            ),

            EvmApiMethod::EmitLog => {
                self.handle_emit_log(input, data, |(evm, log): (&mut Self, Log)| {
                    let (context, inspector) = evm.ctx_inspector();
                    context.log(log.clone());
                    inspector.log(context, log);
                })
            }
            _ => self.request_inner(input, is_static, req_type, data),
        }
    }
}

pub fn stylus_code(bytecode: &[u8], max_wasm_size: u32) -> Result<Option<Bytes>, Vec<u8>> {
    if let Some(bytecode) = bytecode.strip_prefix(STYLUS_DISCRIMINANT) {
        let (dictionary, compressed_bytecode) =
            if let Some((dictionary, compressed_bytecode)) = bytecode.split_at_checked(1) {
                (dictionary, compressed_bytecode)
            } else {
                return Err("specified bytecode is not a Stylus program"
                    .as_bytes()
                    .to_vec());
            };

        let dictionary = match dictionary[0] {
            0x00 => Dictionary::Empty,
            0x01 => Dictionary::StylusProgram,
            t => return Err(format!("unsupported dictionary {t}").as_bytes().to_vec()),
        };

        // Foundry also accepts explicitly uncompressed Wasm with the empty
        // dictionary. Detect that format before Brotli decoding so an
        // oversized valid Brotli stream can never fall back to raw bytes.
        let bytecode =
            if dictionary == Dictionary::Empty && compressed_bytecode.starts_with(b"\0asm") {
                if compressed_bytecode.len() > max_wasm_size as usize {
                    return Err(b"failed decompression: 0".to_vec());
                }
                compressed_bytecode.to_vec()
            } else {
                let mut output = vec![std::mem::MaybeUninit::uninit(); max_wasm_size as usize];
                brotli::decompress_fixed(compressed_bytecode, &mut output, dictionary)
                    .map(|bytes| bytes.to_vec())
                    .map_err(|err| {
                        format!("failed decompression: {}", err as u8)
                            .as_bytes()
                            .to_vec()
                    })?
            };

        Ok(Some(Bytes::from(bytecode)))
    } else {
        Ok(None)
    }
}

/// Resolves either classic inline Stylus bytecode or the ArbOS 60 root/fragment
/// representation. Fragment count and declared decompressed length are consensus
/// activation checks; already-active programs remain callable after owner params change.
pub fn stylus_code_with_fragments<CTX: ArbitrumContextTr>(
    context: &mut CTX,
    bytecode: &[u8],
    params: &StylusParams,
    activating: bool,
    mut gas: Option<&mut Gas>,
) -> Result<Option<Bytes>, Vec<u8>> {
    if bytecode.starts_with(STYLUS_FRAGMENT_DISCRIMINANT) {
        return Err(b"fragmented stylus programs cannot be activated directly; activate the root program instead".to_vec());
    }
    if !bytecode.starts_with(STYLUS_ROOT_DISCRIMINANT) {
        return stylus_code(bytecode, params.max_wasm_size);
    }
    if context.cfg().arbos_version() < ARBOS_VERSION_STYLUS_CONTRACT_LIMIT {
        return Err(b"specified bytecode is not a Stylus program".to_vec());
    }
    if bytecode.len() < 8 || !(bytecode.len() - 8).is_multiple_of(20) {
        return Err(b"invalid stylus program root".to_vec());
    }
    let dictionary = bytecode[3];
    let decompressed_len = u32::from_be_bytes(bytecode[4..8].try_into().unwrap());
    let addresses = &bytecode[8..];
    let count = addresses.len() / 20;
    if count == 0 {
        return Err(b"invalid wasm: fragment count cannot be zero".to_vec());
    }
    if activating {
        if decompressed_len > params.max_wasm_size {
            return Err(format!(
                "invalid wasm: decompressedLength {decompressed_len} is greater then MaxWasmSize {}",
                params.max_wasm_size
            ).into_bytes());
        }
        if count > params.max_fragment_count as usize {
            return Err(format!(
                "invalid wasm: fragment count exceeds limit of {}",
                params.max_fragment_count
            )
            .into_bytes());
        }
    }
    let mut compressed = Vec::new();
    for raw in addresses.as_chunks::<20>().0 {
        let address = Address::from_slice(raw);
        // JournalTr has no read-only access-list query. Loading only the account
        // metadata reports its pre-access warmth; reserve a maximum-sized copy
        // before asking the database to materialize the fragment code.
        let account = context
            .journal_mut()
            .load_account(address)
            .map_err(|_| b"failed to read stylus fragment account".to_vec())?;
        let was_cold = account.is_cold;
        if let Some(gas) = gas.as_deref_mut() {
            let reserve = fragment_read_gas_cost(was_cold, context.cfg().max_code_size() as u64)
                .ok_or_else(|| b"fragment copy gas overflow".to_vec())?;
            if gas.remaining() < reserve {
                gas.spend_all();
                return Err(b"out of gas".to_vec());
            }
        }
        let loaded = context
            .journal_mut()
            .code(address)
            .map_err(|_| b"failed to read stylus fragment".to_vec())?;
        let fragment = loaded.data.clone();
        if let Some(gas) = gas.as_deref_mut() {
            let cost = fragment_read_gas_cost(was_cold, fragment.len() as u64)
                .ok_or_else(|| b"fragment copy gas overflow".to_vec())?;
            if !gas.record_cost(cost) {
                return Err(b"out of gas".to_vec());
            }
        }
        let Some(payload) = fragment.strip_prefix(STYLUS_FRAGMENT_DISCRIMINANT) else {
            return Err(b"invalid stylus program fragment".to_vec());
        };
        compressed.extend_from_slice(payload);
    }
    let mut classic = STYLUS_DISCRIMINANT.to_vec();
    classic.push(dictionary);
    classic.extend_from_slice(&compressed);
    let wasm = stylus_code(&classic, params.max_wasm_size)?;
    if activating
        && wasm
            .as_ref()
            .is_some_and(|wasm| wasm.len() != decompressed_len as usize)
    {
        return Err(b"invalid wasm: decompressed length mismatch".to_vec());
    }
    Ok(wasm)
}

/// Compile Stylus bytecode
pub fn stylus_compile(bytecode: &Bytes, compile_config: &CompileConfig) -> Result<Vec<u8>, String> {
    let serialized = native::compile(
        bytecode,
        compile_config.version,
        compile_config.debug.debug_funcs,
        wasmer_types::compilation::target::Target::default(),
        false,
    )
    .map_err(|e| e.to_string())?;

    Ok(serialized)
}

pub fn stylus_activate(
    mut gas: Option<&mut Gas>,
    bytecode: &Bytes,
    code_hash: B256,
    arbos_version: u64,
    stylus_version: u16,
    page_limit: u16,
    debug: bool,
) -> Result<(Module, StylusData), String> {
    let mut activation_gas = if let Some(gas) = gas.as_deref_mut() {
        let remaining = gas.remaining();
        gas.spend_all();
        remaining
    } else {
        u64::MAX
    };

    let (module, stylus_data) = match native::activate(
        bytecode,
        &Bytes32::from(code_hash.0),
        stylus_version,
        arbos_version,
        page_limit,
        debug,
        &mut activation_gas,
    ) {
        Ok(res) => res,
        Err(e) => return Err(e.to_string()),
    };

    if let Some(gas) = gas {
        gas.erase_cost(activation_gas);
    }

    Ok((module, stylus_data))
}

pub fn ink_to_gas_ceil(pricing: PricingParams, ink: Ink) -> u64 {
    ink.0.div_ceil(pricing.ink_price as u64)
}

pub fn cache_program(
    code_hash: B256,
    stylus_version: u16,
    arbos_version: u64,
    debug: bool,
    serialized: Vec<u8>,
    module: Module,
    stylus_data: StylusData,
) {
    let mut cache = PROGRAM_CACHE.lock().unwrap();
    cache.get_or_insert((code_hash, stylus_version, arbos_version, debug), || {
        (serialized, module, stylus_data)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stylus_code_rejects_decompression_over_configured_limit() {
        let wasm = vec![0u8; 1024];
        let compressed = brotli::compress(&wasm, 11, 22, Dictionary::Empty).unwrap();
        let mut bytecode = STYLUS_DISCRIMINANT.to_vec();
        bytecode.push(0);
        bytecode.extend_from_slice(&compressed);

        assert!(stylus_code(&bytecode, 1023).is_err());
        assert_eq!(stylus_code(&bytecode, 1024).unwrap().unwrap().len(), 1024);
    }

    #[test]
    fn stylus_page_limit_is_consensus_enforced_from_arbos_59() {
        assert_eq!(stylus_page_limit_penalty(58, 128, 129), 0);
        assert_eq!(stylus_page_limit_penalty(59, 128, 128), 0);
        assert_eq!(stylus_page_limit_penalty(59, 128, 129), u64::MAX);
        assert_eq!(stylus_page_limit_penalty(61, 0, u16::MAX), 0);
    }

    #[test]
    fn fragment_read_gas_matches_nitro_word_vectors() {
        assert_eq!(fragment_read_gas_cost(true, 0), Some(2_600));
        assert_eq!(fragment_read_gas_cost(true, 1), Some(2_603));
        assert_eq!(fragment_read_gas_cost(true, 32), Some(2_603));
        assert_eq!(fragment_read_gas_cost(true, 33), Some(2_606));
        assert_eq!(fragment_read_gas_cost(false, 33), Some(106));
        assert_eq!(fragment_read_gas_cost(false, u64::MAX), None);
    }

    #[test]
    fn deserialize_failure_restores_pre_call_open_pages() {
        let mut local = crate::local_context::ArbitrumLocalContext::default();
        local.add_stylus_pages_open(7);
        local.add_stylus_pages_open(11);
        let failed: Result<(), &str> = Err("bad cached module");
        assert!(restore_open_pages_on_error(&mut local, 7, failed).is_err());
        assert_eq!(local.stylus_pages_open(), 7);
        // Ever-open pages are transaction-scoped and intentionally remain high.
        assert_eq!(local.stylus_pages_ever(), 18);
    }

    #[test]
    fn crate_path_rejects_multi_value_starting_with_stylus_v3() {
        let wasm = wat::parse_str(
            r#"(module
                (func (result i32 i32) i32.const 1 i32.const 2)
            )"#,
        )
        .unwrap();
        let path = std::path::Path::new("multi-value-test");
        assert!(stylus::prover::binary::parse_with_stylus_version(&wasm, path, 2).is_ok());
        assert!(stylus::prover::binary::parse_with_stylus_version(&wasm, path, 3).is_err());
    }
}
