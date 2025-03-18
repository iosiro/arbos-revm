use std::{cmp::min, sync::Arc};

use arbutil::evm::{
    api::{EvmApiMethod, Gas, VecReader},
    req::RequestHandler,
};
use revm_interpreter::{
    opcode::{self, InstructionTables},
    CreateInputs, Interpreter, SharedMemory,
};

use crate::{
    primitives::{Address, SpecId, U256},
    Context, Evm, Frame, FrameOrResult, FrameResult, Handler,
};

use super::buffer;
use crate::interpreter::{
    self,
    gas::{sload_cost, sstore_cost, Gas as RevmGas},
    CallInputs, CreateScheme, Host,
};
use crate::Database;

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
    ) -> (Vec<u8>, VecReader, Gas) {
        let data = req_data.as_ref().to_vec();
        let context = self.handler.clone();
        (context)(req_type, data)
    }
}

fn wasm_account_touch<EXT, DB: Database>(
    context: &mut Context<EXT, DB>,
    is_cold: bool,
    with_code: bool,
) -> u64 {
    let code_cost = if with_code {
        context.env().cfg.max_code_size() as u64 / 24576 * 700
    } else {
        0
    };
    code_cost + revm_interpreter::gas::warm_cold_cost(is_cold)
}

#[inline]
fn step<FN, H: Host + ?Sized>(
    opcode: usize,
    interpeter: &mut Interpreter,
    instruction_table: &[FN; 256],
    host: &mut H,
) where
    FN: Fn(&mut Interpreter, &mut H),
{
    // execute instruction.
    (instruction_table[opcode])(interpeter, host)
}

pub(crate) fn request<EXT, DB: Database>(
    context: &mut Context<EXT, DB>,
    handler: &Handler<'_, Context<EXT, DB>, EXT, DB>,
    stack_frame: &mut Frame,
    req_type: EvmApiMethod,
    data: Vec<u8>,
) -> (Vec<u8>, VecReader, Gas) {
    let mut data = data;

    match req_type {
        EvmApiMethod::GetBytes32 => {
            let slot = buffer::take_u256(&mut data);
            if let Some(result) =
                context.sload(stack_frame.interpreter().contract().target_address, slot)
            {
                let gas = sload_cost(context.evm.spec_id(), result.is_cold);
                (result.to_be_bytes_vec(), VecReader::new(vec![]), Gas(gas))
            } else {
                (vec![], VecReader::new(vec![]), Gas(0))
            }
        }

        EvmApiMethod::SetTrieSlots => {
            let gas_left = buffer::take_u64(&mut data);

            if stack_frame.interpreter().is_static {
                return (
                    Status::WriteProtection.into(),
                    VecReader::new(vec![]),
                    Gas(gas_left),
                );
            }

            let mut total_cost = 0;
            while !data.is_empty() {
                let (key, value) = (buffer::take_u256(&mut data), buffer::take_u256(&mut data));

                match context.sstore(
                    stack_frame.interpreter().contract().target_address,
                    key,
                    value,
                ) {
                    Some(result) => {
                        total_cost += sstore_cost(
                            context.evm.spec_id(),
                            &result.data,
                            gas_left,
                            result.is_cold,
                        )
                        .unwrap();
                        if gas_left < total_cost {
                            return (
                                Status::OutOfGas.into(),
                                VecReader::new(vec![]),
                                Gas(gas_left),
                            );
                        }
                    }
                    None => {
                        return (
                            Status::Failure.into(),
                            VecReader::new(vec![]),
                            Gas(gas_left),
                        )
                    }
                }
            }

            (
                Status::Success.into(),
                VecReader::new(vec![]),
                Gas(total_cost),
            )
        }

        EvmApiMethod::GetTransientBytes32 => {
            let slot = buffer::take_u256(&mut data);
            let result = context.tload(stack_frame.interpreter().contract().target_address, slot);
            (result.to_be_bytes_vec(), VecReader::new(vec![]), Gas(0))
        }

        EvmApiMethod::SetTransientBytes32 => {
            if stack_frame.interpreter().is_static {
                return (
                    Status::WriteProtection.into(),
                    VecReader::new(vec![]),
                    Gas(0),
                );
            }
            let key = buffer::take_u256(&mut data);
            let value = buffer::take_u256(&mut data);
            context.tstore(
                stack_frame.interpreter().contract().target_address,
                key,
                value,
            );
            (Status::Success.into(), VecReader::new(vec![]), Gas(0))
        }

        EvmApiMethod::ContractCall | EvmApiMethod::DelegateCall | EvmApiMethod::StaticCall => {
            let bytecode_address = buffer::take_address(&mut data);
            let value = buffer::take_u256(&mut data);
            let gas_left = buffer::take_u64(&mut data);
            let gas_limit = buffer::take_u64(&mut data);
            let calldata = buffer::take_rest(&mut data);

            let (target_address, caller, is_static, value, scheme) = match req_type {
                EvmApiMethod::ContractCall => (
                    bytecode_address,
                    stack_frame.interpreter().contract().target_address,
                    stack_frame.interpreter().is_static,
                    value,
                    interpreter::CallScheme::Call,
                ),
                EvmApiMethod::DelegateCall => {
                    // copy value as stylus uses zero for all delegate calls
                    (
                        stack_frame.interpreter().contract().target_address,
                        stack_frame.interpreter().contract().caller,
                        stack_frame.interpreter().is_static,
                        stack_frame.interpreter().contract().call_value,
                        interpreter::CallScheme::DelegateCall,
                    )
                }
                EvmApiMethod::StaticCall => (
                    bytecode_address,
                    stack_frame.interpreter().contract().target_address,
                    true,
                    value,
                    interpreter::CallScheme::StaticCall,
                ),
                _ => unreachable!(),
            };

            if is_static && !value.is_zero() {
                return (
                    Status::WriteProtection.into(),
                    VecReader::new(vec![]),
                    Gas(gas_left),
                );
            }

            let gas_limit = if context.evm.spec_id().is_enabled_in(SpecId::TANGERINE) {
                min(gas_left - gas_left / 64, gas_limit)
            } else {
                gas_limit
            };

            let mut gas = RevmGas::new(gas_limit);
            gas.spend_all();

            let result = match handler.execution().call(
                context,
                Box::new(CallInputs {
                    input: calldata,
                    return_memory_offset: 0..0,
                    gas_limit,
                    bytecode_address,
                    target_address,
                    caller,
                    value: crate::interpreter::CallValue::Transfer(value),
                    scheme,
                    is_static,
                    is_eof: false,
                }),
            ) {
                Ok(FrameOrResult::Frame(first_frame)) => {
                    Evm::run_the_loop(context, handler, first_frame)
                }
                Ok(FrameOrResult::Result(result)) => Ok(result),
                Err(e) => {
                    context.evm.error = Err(e);
                    return (
                        Status::Failure.into(),
                        VecReader::new(vec![]),
                        Gas(gas_left),
                    );
                }
            };

            match result {
                Ok(FrameResult::Call(result)) => {
                    _ = handler.execution().insert_call_outcome(
                        context,
                        stack_frame,
                        &mut SharedMemory::new(),
                        result.clone(),
                    );
                    gas.erase_cost(result.gas().remaining());
                    (
                        Status::Success.into(),
                        VecReader::new(result.result.output.to_vec()),
                        Gas(gas.spent()),
                    )
                }
                _ => (vec![], VecReader::new(vec![]), Gas(gas.spent())),
            }
        }

        EvmApiMethod::Create1 | EvmApiMethod::Create2 => {
            let is_create_2 = matches!(req_type, EvmApiMethod::Create2);
            let gas_remaining = buffer::take_u64(&mut data);
            let value = buffer::take_u256(&mut data);
            let salt = is_create_2.then(|| buffer::take_u256(&mut data));
            let init_code = buffer::take_rest(&mut data);

            if stack_frame.interpreter().is_static {
                return (
                    [vec![0x00], "write protection".as_bytes().to_vec()].concat(),
                    VecReader::new(vec![]),
                    Gas(0),
                );
            }

            let error_response = (
                [vec![0x01], Address::ZERO.to_vec()].concat(),
                VecReader::new(vec![]),
                Gas(gas_remaining),
            );

            if is_create_2 && !context.evm.spec_id().is_enabled_in(SpecId::PETERSBURG) {
                return error_response;
            }

            let mut gas_cost = 0;
            let len = init_code.len();

            if len != 0 && context.evm.spec_id().is_enabled_in(SpecId::SHANGHAI) {
                let max_initcode_size = context.env().cfg.max_code_size().saturating_mul(2);
                if len > max_initcode_size {
                    return error_response;
                }
                gas_cost = revm_interpreter::gas::initcode_cost(len as u64);
            }

            let scheme = if is_create_2 {
                if let Some(check_cost) = revm_interpreter::gas::create2_cost(len as u64)
                    .and_then(|cost| gas_cost.checked_add(cost))
                {
                    gas_cost = check_cost;
                } else {
                    return error_response;
                };
                CreateScheme::Create2 {
                    salt: salt.unwrap(),
                }
            } else {
                gas_cost += revm_interpreter::gas::CREATE;
                CreateScheme::Create
            };

            if gas_remaining < gas_cost {
                return (
                    [vec![0x00], "out of gas".as_bytes().to_vec()].concat(),
                    VecReader::new(vec![]),
                    Gas(gas_remaining),
                );
            }

            let mut gas_limit = gas_remaining - gas_cost;

            let gas_stipend = if context.evm.spec_id().is_enabled_in(SpecId::TANGERINE) {
                gas_limit / 64
            } else {
                0
            };

            gas_limit = gas_limit.saturating_sub(gas_stipend);

            let result = handler.execution().create(
                context,
                Box::new(CreateInputs {
                    caller: stack_frame.interpreter().contract().target_address,
                    scheme,
                    value,
                    init_code,
                    gas_limit,
                }),
            );

            // Starts the main running loop.
            let frame_or_return = match result {
                Ok(FrameOrResult::Frame(first_frame)) => {
                    Evm::run_the_loop(context, handler, first_frame)
                }
                Ok(FrameOrResult::Result(result)) => Ok(result),
                Err(e) => {
                    context.evm.error = Err(e);
                    return error_response;
                }
            };

            let mut gas = RevmGas::new(gas_remaining);
            gas.spend_all();

            if let Ok(FrameResult::Create(create_outcome)) = frame_or_return {
                gas.erase_cost(create_outcome.gas().remaining());

                if matches!(
                    create_outcome.instruction_result(),
                    interpreter::InstructionResult::Revert
                ) {
                    return (
                        [vec![0x00], create_outcome.output().to_vec()].concat(),
                        VecReader::new(vec![]),
                        Gas(gas.spent()),
                    );
                }

                if let Some(address) = create_outcome.address {
                    return (
                        [vec![0x01], address.to_vec()].concat(),
                        VecReader::new(vec![]),
                        Gas(gas.spent()),
                    );
                }
            }

            (
                [vec![0x01], Address::ZERO.to_vec()].concat(),
                VecReader::new(vec![]),
                Gas(gas.spent()),
            )
        }

        EvmApiMethod::EmitLog => {
            if stack_frame.interpreter().is_static {
                return (
                    "write protection".as_bytes().to_vec(),
                    VecReader::new(vec![]),
                    Gas(0),
                );
            }

            let topic_count = buffer::take_u32(&mut data);
            let mut topics = Vec::with_capacity(topic_count as usize);
            for _ in 0..topic_count {
                topics.push(buffer::take_bytes32(&mut data));
            }

            topics.reverse();

            let data = buffer::take_rest(&mut data);

            // push the logs to the evm stack
            // this is to allow the inspector to detect the logs
            stack_frame
                .interpreter_mut()
                .shared_memory
                .resize(data.len());
            stack_frame
                .interpreter_mut()
                .shared_memory
                .set_data(0, 0, data.len(), &data.0);

            for topic in topics {
                _ = stack_frame.interpreter_mut().stack.push(topic.into());
            }

            _ = stack_frame
                .interpreter_mut()
                .stack
                .push(U256::from(data.len()));
            _ = stack_frame.interpreter_mut().stack.push(U256::ZERO);

            let opcode = match topic_count {
                0 => opcode::LOG0,
                1 => opcode::LOG1,
                2 => opcode::LOG2,
                3 => opcode::LOG3,
                4 => opcode::LOG4,
                _ => unreachable!(),
            };

            match &handler.instruction_table {
                InstructionTables::Plain(table) => step(
                    opcode as usize,
                    stack_frame.interpreter_mut(),
                    table,
                    context,
                ),
                InstructionTables::Boxed(table) => step(
                    opcode as usize,
                    stack_frame.interpreter_mut(),
                    table,
                    context,
                ),
            };

            (vec![], VecReader::new(vec![]), Gas(0))
        }

        EvmApiMethod::AccountBalance => {
            let address = buffer::take_address(&mut data);
            let balance = context.balance(address).unwrap();
            let gas = wasm_account_touch(context, balance.is_cold, false);
            (balance.to_be_bytes_vec(), VecReader::new(vec![]), Gas(gas))
        }

        EvmApiMethod::AccountCode => {
            let address = buffer::take_address(&mut data);
            let code = context.code(address).unwrap();
            let gas = wasm_account_touch(context, code.is_cold, true);
            (vec![], VecReader::new(code.to_vec()), Gas(gas))
        }

        EvmApiMethod::AccountCodeHash => {
            let address = buffer::take_address(&mut data);
            let code_hash = context.code_hash(address).unwrap();
            let gas = wasm_account_touch(context, code_hash.is_cold, false);
            (code_hash.to_vec(), VecReader::new(vec![]), Gas(gas))
        }

        EvmApiMethod::AddPages => {
            let _count = buffer::take_u16(&mut data);
            (Status::Success.into(), VecReader::new(vec![]), Gas(0))
        }

        EvmApiMethod::CaptureHostIO => {
            //let data = revm_types::take_rest(&mut data);
            //println!("CaptureHostIO: {:?}", String::from_utf8_lossy(&data));
            (Status::Success.into(), VecReader::new(vec![]), Gas(0))
        }
    }
}

enum Status {
    Success,
    Failure,
    OutOfGas,
    WriteProtection,
}

impl From<Status> for Vec<u8> {
    fn from(status: Status) -> Vec<u8> {
        match status {
            Status::Success => vec![0],
            Status::Failure => vec![1],
            Status::OutOfGas => vec![2],
            Status::WriteProtection => vec![3],
        }
    }
}
