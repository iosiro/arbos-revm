use std::{cmp::min, mem, sync::Arc};

use arbutil::evm::{
    api::{EvmApiMethod, Gas as ArbGas, VecReader},
    req::RequestHandler,
};
use revm::{
    Database,
    context::{Cfg, ContextError, ContextTr, CreateScheme, FrameStack, JournalTr},
    handler::{
        EvmTr, FrameResult, ItemOrResult, PrecompileProvider, instructions::InstructionProvider,
    },
    interpreter::{
        CallInput, CallInputs, CreateInputs, FrameInput, Gas, InputsImpl, InstructionResult,
        InterpreterAction, InterpreterResult, interpreter::EthInterpreter,
        interpreter_action::FrameInit,
    },
    primitives::{Address, Log, U256, hardfork::SpecId},
};
use tracing::{debug, trace, warn};

use crate::{
    ArbitrumContextTr, ArbitrumEvm, Utf8OrHex, buffer,
    local_context::ArbitrumLocalContextTr,
    state::{ArbState, ArbStateGetter},
    stylus_executor::stylus_call_cost,
};

pub(crate) type HostCallFunc = dyn Fn(
    arbutil::evm::api::EvmApiMethod,
    Vec<u8>,
) -> (Vec<u8>, VecReader, arbutil::evm::api::Gas);

pub(crate) struct StylusHandler {
    pub handler: Arc<Box<HostCallFunc>>,
}

unsafe impl Send for StylusHandler {}

impl StylusHandler {
    pub(crate) fn new(handler: Arc<Box<HostCallFunc>>) -> Self {
        Self { handler }
    }
}

impl RequestHandler<VecReader> for StylusHandler {
    fn request(
        &mut self,
        req_type: EvmApiMethod,
        req_data: impl AsRef<[u8]>,
    ) -> (Vec<u8>, VecReader, ArbGas) {
        let data = req_data.as_ref().to_vec();
        let api = self.handler.clone();
        (api)(req_type, data)
    }
}

pub fn wasm_account_touch<CTX>(context: CTX, is_cold: bool, with_code: bool) -> u64
where
    CTX: ArbitrumContextTr,
{
    let code_cost = if with_code {
        context.cfg().max_code_size() as u64 / 24576 * 700
    } else {
        0
    };
    code_cost + if is_cold { 2_600 } else { 100 }
}

const CALL_VALUE_TRANSFER_GAS: u64 = 9_000;
const CALL_NEW_ACCOUNT_GAS: u64 = 25_000;
const CALL_STIPEND: u64 = 2_300;

fn stylus_call_base_cost(is_cold: bool, is_empty: bool, transfers_value: bool) -> u64 {
    let mut cost: u64 = if is_cold { 2_600 } else { 100 };
    if transfers_value {
        cost = cost.saturating_add(CALL_VALUE_TRANSFER_GAS);
        if is_empty {
            cost = cost.saturating_add(CALL_NEW_ACCOUNT_GAS);
        }
    }
    cost
}

fn stylus_call_gas(
    gas_left: u64,
    gas_requested: u64,
    base_cost: u64,
    transfers_value: bool,
) -> Option<u64> {
    if base_cost > gas_left {
        return None;
    }
    let available = gas_left - base_cost;
    let eip150_limit = (available / 64) * 63 + (available % 64) * 63 / 64;
    let mut gas = min(eip150_limit, gas_requested);
    if transfers_value {
        gas = gas.saturating_add(CALL_STIPEND);
    }
    Some(gas)
}

fn stylus_call_scheme(
    req_type: EvmApiMethod,
    requested_value: U256,
    parent_value: U256,
) -> (revm::interpreter::CallScheme, revm::interpreter::CallValue) {
    match req_type {
        EvmApiMethod::DelegateCall => (
            revm::interpreter::CallScheme::DelegateCall,
            revm::interpreter::CallValue::Apparent(parent_value),
        ),
        EvmApiMethod::StaticCall => (
            revm::interpreter::CallScheme::StaticCall,
            revm::interpreter::CallValue::Transfer(U256::ZERO),
        ),
        EvmApiMethod::ContractCall => (
            revm::interpreter::CallScheme::Call,
            revm::interpreter::CallValue::Transfer(requested_value),
        ),
        _ => unreachable!("non-call request passed to handle_contract_call"),
    }
}

impl<CTX, INSP, P, I> ArbitrumEvm<CTX, INSP, P, I>
where
    CTX: ArbitrumContextTr,
    I: InstructionProvider<Context = CTX, InterpreterTypes = EthInterpreter>,
    P: PrecompileProvider<CTX, Output = InterpreterResult>,
{
    /// Handle contract calls (ContractCall, DelegateCall, StaticCall)
    pub(crate) fn handle_contract_call(
        &mut self,
        input: InputsImpl,
        is_static: bool,
        req_type: EvmApiMethod,
        data: Vec<u8>,
        call_handler: impl FnOnce(
            &mut Self,
            FrameInit,
        ) -> Result<
            FrameResult,
            ContextError<<<CTX as ContextTr>::Db as Database>::Error>,
        >,
    ) -> (Vec<u8>, VecReader, ArbGas) {
        let mut data = data;
        let bytecode_address = buffer::take_address(&mut data);
        let value = buffer::take_u256(&mut data);
        let gas_left = buffer::take_u64(&mut data);
        let gas_limit = buffer::take_u64(&mut data);
        let calldata = buffer::take_rest(&mut data);

        let is_static = matches!(req_type, EvmApiMethod::StaticCall) || is_static;
        let (target_address, caller) = if matches!(req_type, EvmApiMethod::DelegateCall) {
            (input.target_address, input.caller_address)
        } else {
            (bytecode_address, input.target_address)
        };

        trace!(
            target: "arbos-revm::stylus-api",
            ?req_type,
            bytecode_address = %bytecode_address,
            target_address = %target_address,
            caller = %caller,
            is_static,
            gas_left,
            gas_limit,
            call_value = %value,
            calldata_len = calldata.len(),
            "Stylus host contract call"
        );

        if is_static && !value.is_zero() {
            debug!(
                target: "arbos-revm::stylus-api",
                target_address = %target_address,
                bytecode_address = %bytecode_address,
                call_value = %value,
                "Rejecting Stylus call with value in static context"
            );
            return (
                // Nitro's call request protocol reports every EVM call error as
                // status byte 2 and charges zero for this pre-frame rejection.
                Status::OutOfGas.into(),
                VecReader::new(vec![]),
                ArbGas(0),
            );
        }

        let account = self
            .ctx()
            .journal_mut()
            .load_account_with_code(bytecode_address)
            .unwrap();
        let transfers_value = matches!(req_type, EvmApiMethod::ContractCall) && !value.is_zero();
        let base_cost =
            stylus_call_base_cost(account.is_cold, account.data.is_empty(), transfers_value);
        let Some(call_gas_limit) = stylus_call_gas(gas_left, gas_limit, base_cost, transfers_value)
        else {
            return (
                Status::OutOfGas.into(),
                VecReader::new(vec![]),
                ArbGas(gas_left),
            );
        };

        let mut gas = Gas::new(call_gas_limit);

        let (scheme, call_value) = stylus_call_scheme(req_type, value, input.call_value);

        let first_frame_input = FrameInput::Call(Box::new(CallInputs {
            input: CallInput::Bytes(calldata),
            return_memory_offset: 0..0,
            gas_limit: gas.remaining(),
            bytecode_address,
            target_address,
            caller,
            value: call_value,
            scheme,
            is_static,
            known_bytecode: (
                account.data.info.code_hash,
                account.data.info.code.clone().unwrap_or_default(),
            ),
            reservoir: 0,
            charged_new_account_state_gas: false,
        }));

        let next_action = InterpreterAction::NewFrame(first_frame_input);

        let frame_result: Result<_, ContextError<<<CTX as ContextTr>::Db as Database>::Error>> =
            self.0
                .frame_stack
                .get()
                .process_next_action(&mut self.0.ctx, next_action);

        let original_frame_stack = mem::replace(&mut self.0.frame_stack, FrameStack::new());

        gas.spend_all();

        if let Ok(ItemOrResult::Item(frame_init)) = frame_result {
            let result = call_handler(self, frame_init);

            self.0.frame_stack = original_frame_stack;
            self.0
                .frame_stack()
                .get()
                .interpreter
                .memory
                .free_child_context();

            if let Ok(FrameResult::Call(call_outcome)) = result {
                gas.erase_cost(call_outcome.gas().remaining());

                let instruction_result = *call_outcome.instruction_result();
                let status = if instruction_result.is_ok() {
                    Status::Success
                } else {
                    Status::Failure
                };

                let status_label = status.as_str();
                let output = call_outcome.output().to_vec();

                debug!(
                    target: "arbos-revm::stylus-api",
                    target_address = %target_address,
                    bytecode_address = %bytecode_address,
                    ?instruction_result,
                    status = status_label,
                    output_len = output.len(),
                    output = %String::from_utf8_or_hex(output.clone()),
                    gas_spent = gas.total_gas_spent(),
                    gas_remaining = call_outcome.gas().remaining(),
                    "Stylus host call finished"
                );

                return (
                    status.into(),
                    VecReader::new(output),
                    ArbGas(base_cost.saturating_add(gas.total_gas_spent())),
                );
            }
        }

        warn!(
            target: "arbos-revm::stylus-api",
            target_address = %target_address,
            bytecode_address = %bytecode_address,
            gas_spent = gas.total_gas_spent(),
            "Stylus host call returning failure response without call outcome"
        );
        (
            Status::Failure.into(),
            VecReader::new(vec![]),
            ArbGas(base_cost.saturating_add(gas.total_gas_spent())),
        )
    }

    /// Handle contract creation (Create1, Create2)
    pub(crate) fn handle_contract_creation(
        &mut self,
        input: InputsImpl,
        is_static: bool,
        req_type: EvmApiMethod,
        data: Vec<u8>,
        call_handler: impl FnOnce(
            &mut Self,
            FrameInit,
        ) -> Result<
            FrameResult,
            ContextError<<<CTX as ContextTr>::Db as Database>::Error>,
        >,
    ) -> (Vec<u8>, VecReader, ArbGas) {
        let is_create_2 = matches!(req_type, EvmApiMethod::Create2);
        let mut data = data;
        let gas_remaining = buffer::take_u64(&mut data);
        let value = buffer::take_u256(&mut data);
        let salt = is_create_2.then(|| buffer::take_u256(&mut data));
        let init_code = buffer::take_rest(&mut data);

        let spec = self.ctx().cfg().spec().into();

        trace!(
            target: "arbos-revm::stylus-api",
            ?req_type,
            target_address = %input.target_address,
            caller_address = %input.caller_address,
            is_static,
            gas_remaining,
            value = %value,
            salt = ?salt,
            init_code_len = init_code.len(),
            "Stylus contract creation request"
        );

        if is_static {
            debug!(
                target: "arbos-revm::stylus-api",
                target_address = %input.target_address,
                "Rejecting create in static context"
            );
            return (
                [vec![0x00], "write protection".as_bytes().to_vec()].concat(),
                VecReader::new(vec![]),
                ArbGas(0),
            );
        }

        let error_response = (
            [vec![0x01], Address::ZERO.to_vec()].concat(),
            VecReader::new(vec![]),
            ArbGas(gas_remaining),
        );

        if is_create_2 && !spec.is_enabled_in(SpecId::PETERSBURG) {
            debug!(
                target: "arbos-revm::stylus-api",
                target_address = %input.target_address,
                "CREATE2 not enabled for current spec"
            );
            return error_response;
        }

        let mut gas_cost: u64 = 0;
        let len = init_code.len();

        if len != 0 && spec.is_enabled_in(SpecId::SHANGHAI) {
            let max_initcode_size = self.ctx().cfg().max_code_size().saturating_mul(2);
            if len > max_initcode_size {
                debug!(
                    target: "arbos-revm::stylus-api",
                    target_address = %input.target_address,
                    init_code_len = init_code.len(),
                    max_initcode_size,
                    "Init code too large for Stylus create"
                );
                return error_response;
            }
            gas_cost = self.ctx().cfg().gas_params().initcode_cost(len);
        }

        let scheme = if is_create_2 {
            if let Some(check_cost) =
                gas_cost.checked_add(self.ctx().cfg().gas_params().create2_cost(len))
            {
                gas_cost = check_cost;
            } else {
                return error_response;
            };
            CreateScheme::Create2 {
                salt: salt.unwrap(),
            }
        } else {
            gas_cost += revm::interpreter::gas::CREATE;
            CreateScheme::Create
        };

        if gas_remaining < gas_cost {
            debug!(
                target: "arbos-revm::stylus-api",
                target_address = %input.target_address,
                gas_cost,
                gas_remaining,
                "Insufficient gas for Stylus create"
            );
            return (
                [vec![0x00], "out of gas".as_bytes().to_vec()].concat(),
                VecReader::new(vec![]),
                ArbGas(0),
            );
        }

        let gas_limit = gas_remaining - gas_cost;

        let gas_stipend = if spec.is_enabled_in(SpecId::TANGERINE) {
            gas_limit / 64
        } else {
            0
        };

        let mut gas = Gas::new(gas_limit);
        _ = gas.record_regular_cost(gas_stipend);

        let first_frame_input = FrameInput::Create(Box::new(CreateInputs::new(
            input.target_address,
            scheme,
            value,
            init_code,
            gas.remaining(),
            0,
        )));

        gas.spend_all();

        let next_action = InterpreterAction::NewFrame(first_frame_input);

        let frame_result: Result<_, ContextError<<<CTX as ContextTr>::Db as Database>::Error>> =
            self.0
                .frame_stack
                .get()
                .process_next_action(&mut self.0.ctx, next_action);

        let original_frame_stack = mem::replace(&mut self.0.frame_stack, FrameStack::new());

        if let Ok(ItemOrResult::Item(frame_init)) = frame_result {
            let result = call_handler(self, frame_init);

            self.0.frame_stack = original_frame_stack;
            self.0
                .frame_stack()
                .get()
                .interpreter
                .memory
                .free_child_context();

            if let Ok(FrameResult::Create(create_outcome)) = result {
                if InstructionResult::Revert == *create_outcome.instruction_result() {
                    let output = create_outcome.output().to_vec();
                    debug!(
                        target: "arbos-revm::stylus-api",
                        target_address = %input.target_address,
                        output_len = output.len(),
                        output = %String::from_utf8_or_hex(output.clone()),
                        gas_spent = gas.total_gas_spent(),
                        gas_remaining = create_outcome.gas().remaining(),
                        "Stylus create reverted"
                    );
                    return (
                        [vec![0x00], output].concat(),
                        VecReader::new(vec![]),
                        ArbGas(gas.total_gas_spent()),
                    );
                }

                if let Some(address) = create_outcome.address {
                    gas.erase_cost(create_outcome.gas().remaining() + gas_stipend);

                    debug!(
                        target: "arbos-revm::stylus-api",
                        target_address = %input.target_address,
                        new_address = %address,
                        gas_spent = gas.total_gas_spent(),
                        gas_remaining = create_outcome.gas().remaining(),
                        "Stylus create succeeded"
                    );

                    return (
                        [vec![0x01], address.to_vec()].concat(),
                        VecReader::new(vec![]),
                        ArbGas(gas.total_gas_spent()),
                    );
                }
            }
        }

        warn!(
            target: "arbos-revm::stylus-api",
            target_address = %input.target_address,
            "Stylus create returning default failure response"
        );
        error_response
    }

    /// Handle log emission with closure-based log handling
    pub(crate) fn handle_emit_log<F>(
        &mut self,
        input: InputsImpl,
        data: Vec<u8>,
        log_handler: F,
    ) -> (Vec<u8>, VecReader, ArbGas)
    where
        F: FnOnce((&mut Self, Log)),
    {
        let mut data = data;
        let topic_count = buffer::take_u32(&mut data);
        let mut topics = Vec::with_capacity(topic_count as usize);
        for _ in 0..topic_count {
            topics.push(buffer::take_bytes32(&mut data));
        }
        let log_data = buffer::take_rest(&mut data);

        let log = Log::new_unchecked(input.target_address, topics, log_data);

        log_handler((self, log));

        (vec![], VecReader::new(vec![]), ArbGas(0))
    }

    pub(crate) fn request(
        &mut self,
        input: InputsImpl,
        is_static: bool,
        req_type: EvmApiMethod,
        data: Vec<u8>,
    ) -> (Vec<u8>, VecReader, ArbGas) {
        trace!(
            target: "arbos-revm::stylus-api",
            ?req_type,
            target_address = %input.target_address,
            caller_address = %input.caller_address,
            is_static,
            payload_len = data.len(),
            "Stylus host request dispatch"
        );

        match req_type {
            EvmApiMethod::ContractCall | EvmApiMethod::DelegateCall | EvmApiMethod::StaticCall => {
                self.handle_contract_call(input, is_static, req_type, data, |evm, frame_init| {
                    evm.run_exec_loop(frame_init)
                })
            }

            EvmApiMethod::Create1 | EvmApiMethod::Create2 => self.handle_contract_creation(
                input,
                is_static,
                req_type,
                data,
                |evm, frame_init| evm.run_exec_loop(frame_init),
            ),

            EvmApiMethod::EmitLog => {
                // LOG opcodes are not allowed in static context (write protection)
                if is_static {
                    return (
                        Status::WriteProtection.into(),
                        VecReader::new(vec![]),
                        ArbGas(0),
                    );
                }
                self.handle_emit_log(input, data, |(evm, log): (&mut Self, Log)| {
                    let context = evm.ctx();
                    context.log(log);
                })
            }

            _ => self.request_inner(input, is_static, req_type, data),
        }
    }

    pub(crate) fn request_inner(
        &mut self,
        input: InputsImpl,
        is_static: bool,
        req_type: EvmApiMethod,
        data: Vec<u8>,
    ) -> (Vec<u8>, VecReader, ArbGas) {
        let context = self.ctx();
        let mut data = data;

        let spec = context.cfg().spec();

        match req_type {
            EvmApiMethod::GetBytes32 => {
                let slot = buffer::take_u256(&mut data);
                if let Some(result) = context.sload(input.target_address, slot) {
                    let gas_params = context.cfg().gas_params();
                    let gas = if spec.clone().into().is_enabled_in(SpecId::BERLIN) {
                        if result.is_cold {
                            gas_params.cold_storage_cost()
                        } else {
                            gas_params.warm_storage_read_cost()
                        }
                    } else if spec.clone().into().is_enabled_in(SpecId::ISTANBUL) {
                        800
                    } else {
                        200
                    };
                    (
                        result.to_be_bytes_vec(),
                        VecReader::new(vec![]),
                        ArbGas(gas),
                    )
                } else {
                    (vec![], VecReader::new(vec![]), ArbGas(0))
                }
            }

            EvmApiMethod::SetTrieSlots => {
                let gas_left = buffer::take_u64(&mut data);

                if is_static {
                    debug!(
                        target: "arbos-revm::stylus-api",
                        target_address = %input.target_address,
                        "Rejecting SetTrieSlots in static context"
                    );
                    return (
                        Status::WriteProtection.into(),
                        VecReader::new(vec![]),
                        ArbGas(gas_left),
                    );
                }

                let mut total_cost = 0;
                while !data.is_empty() {
                    let (key, value) = (buffer::take_u256(&mut data), buffer::take_u256(&mut data));

                    match context.sstore(input.target_address, key, value) {
                        Some(result) => {
                            total_cost += context.cfg().gas_params().sstore_dynamic_gas(
                                spec.clone().into().is_enabled_in(SpecId::ISTANBUL),
                                &result.data,
                                result.is_cold,
                            );

                            if gas_left < total_cost {
                                debug!(
                                    target: "arbos-revm::stylus-api",
                                    target_address = %input.target_address,
                                    gas_left,
                                    total_cost,
                                    "SetTrieSlots ran out of gas"
                                );
                                return (
                                    Status::OutOfGas.into(),
                                    VecReader::new(vec![]),
                                    ArbGas(gas_left),
                                );
                            }
                        }
                        _ => {
                            warn!(
                                target: "arbos-revm::stylus-api",
                                target_address = %input.target_address,
                                "SetTrieSlots failed during storage update"
                            );
                            return (
                                Status::Failure.into(),
                                VecReader::new(vec![]),
                                ArbGas(gas_left),
                            );
                        }
                    }
                }

                (
                    Status::Success.into(),
                    VecReader::new(vec![]),
                    ArbGas(total_cost),
                )
            }

            EvmApiMethod::GetTransientBytes32 => {
                let slot = buffer::take_u256(&mut data);
                let result = context.tload(input.target_address, slot);
                (result.to_be_bytes_vec(), VecReader::new(vec![]), ArbGas(0))
            }

            EvmApiMethod::SetTransientBytes32 => {
                if is_static {
                    return (
                        Status::WriteProtection.into(),
                        VecReader::new(vec![]),
                        ArbGas(0),
                    );
                }
                let key = buffer::take_u256(&mut data);
                let value = buffer::take_u256(&mut data);
                context.tstore(input.target_address, key, value);
                (Status::Success.into(), VecReader::new(vec![]), ArbGas(0))
            }
            EvmApiMethod::AccountBalance => {
                let address = buffer::take_address(&mut data);
                let balance = context.balance(address).unwrap();
                let gas = wasm_account_touch(context, balance.is_cold, false);
                (
                    balance.to_be_bytes_vec(),
                    VecReader::new(vec![]),
                    ArbGas(gas),
                )
            }

            EvmApiMethod::AccountCode => {
                let address = buffer::take_address(&mut data);
                let gas_left = buffer::take_u64(&mut data);

                // Load account info (without code) to determine warm/cold and compute gas
                let account = context.load_account_code_hash(address).unwrap();
                let gas = wasm_account_touch(&mut *context, account.is_cold, true);

                // If not enough gas, return empty code (matching nitro behavior)
                if gas_left < gas {
                    return (vec![], VecReader::new(vec![]), ArbGas(gas));
                }

                let code = context.load_account_code(address).unwrap();
                (vec![], VecReader::new(code.to_vec()), ArbGas(gas))
            }

            EvmApiMethod::AccountCodeHash => {
                let address = buffer::take_address(&mut data);
                // Use load_account to get the raw code_hash (KECCAK_EMPTY for no-code
                // accounts) instead of Host::load_account_code_hash which applies
                // EIP-1052 EXTCODEHASH semantics (B256::ZERO for empty accounts).
                // The Stylus host wraps StateDB.GetCodeHash, not the EXTCODEHASH opcode.
                let account = context.journal_mut().load_account(address).unwrap();
                let is_cold = account.is_cold;
                let code_hash = account.data.info.code_hash;
                let gas = wasm_account_touch(&mut *context, is_cold, false);
                (code_hash.to_vec(), VecReader::new(vec![]), ArbGas(gas))
            }

            EvmApiMethod::AddPages => {
                let count = buffer::take_u16(&mut data);
                let open = context.local().stylus_pages_open();
                let ever = context.local().stylus_pages_ever();

                let stylus_params = context
                    .arb_state(None, true)
                    .programs()
                    .stylus_params()
                    .get()
                    .unwrap();

                let free_pages = stylus_params.free_pages;
                let page_gas = stylus_params.page_gas;

                let cost = stylus_call_cost(count, open, ever, free_pages, page_gas);
                context.local_mut().add_stylus_pages_open(count);
                (Status::Success.into(), VecReader::new(vec![]), ArbGas(cost))
            }

            EvmApiMethod::CaptureHostIO => {
                //let data = buffer::take_rest(&mut data);
                //println!("CaptureHostIO: {:?}", String::from_utf8_lossy(&data));
                (Status::Success.into(), VecReader::new(vec![]), ArbGas(0))
            }
            _ => unimplemented!("EVM API method not implemented: {:?}", req_type),
        }
    }
}

enum Status {
    Success,
    Failure,
    OutOfGas,
    WriteProtection,
}

impl Status {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::OutOfGas => "out_of_gas",
            Self::WriteProtection => "write_protection",
        }
    }
}

impl From<Status> for Vec<u8> {
    fn from(status: Status) -> Self {
        match status {
            Status::Success => vec![0],
            Status::Failure => vec![1],
            Status::OutOfGas => vec![2],
            Status::WriteProtection => vec![3],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stylus_call_cost_matches_nitro_vectors() {
        assert_eq!(stylus_call_base_cost(false, false, false), 100);
        assert_eq!(stylus_call_base_cost(true, false, false), 2_600);
        assert_eq!(stylus_call_base_cost(false, false, true), 9_100);
        assert_eq!(stylus_call_base_cost(true, true, true), 36_600);

        // Nitro applies EIP-150 after deducting the complete base cost.
        assert_eq!(
            stylus_call_gas(100_000, u64::MAX, 36_600, true),
            Some(62_409 + CALL_STIPEND)
        );
        assert_eq!(stylus_call_gas(36_599, u64::MAX, 36_600, true), None);

        // The stipend is added after the requested-gas cap.
        assert_eq!(stylus_call_gas(100_000, 7_000, 9_100, true), Some(9_300));
        assert_eq!(stylus_call_gas(100_000, 7_000, 100, false), Some(7_000));
    }

    #[test]
    fn stylus_call_schemes_preserve_nitro_value_semantics() {
        use revm::interpreter::{CallScheme, CallValue};

        let requested = U256::from(7);
        let parent = U256::from(11);
        assert_eq!(
            stylus_call_scheme(EvmApiMethod::ContractCall, requested, parent),
            (CallScheme::Call, CallValue::Transfer(requested))
        );
        assert_eq!(
            stylus_call_scheme(EvmApiMethod::DelegateCall, U256::ZERO, parent),
            (CallScheme::DelegateCall, CallValue::Apparent(parent))
        );
        assert_eq!(
            stylus_call_scheme(EvmApiMethod::StaticCall, U256::ZERO, parent),
            (CallScheme::StaticCall, CallValue::Transfer(U256::ZERO))
        );
    }
}
