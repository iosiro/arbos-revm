use std::{boxed::Box, cmp::min, sync::Arc, vec::Vec};

use arbutil::evm::{
    api::{EvmApiMethod, Gas, VecReader},
    req::RequestHandler,
};
use revm::{
    context::{Cfg, ContextTr, CreateScheme},
    handler::{EvmTr, Frame, FrameOrResult, FrameResult, Handler},
    inspector::{handler::frame_start, InspectorEvmTr, InspectorFrame, InspectorHandler},
    interpreter::{
        gas::{
            create2_cost, initcode_cost, sload_cost, sstore_cost, warm_cold_cost, Gas as RevmGas,
            CREATE,
        },
        interpreter::EthInterpreter,
        CallInput, CallInputs, CreateInputs, FrameInput, Host, InputsImpl, InstructionResult,
    },
    primitives::{hardfork::SpecId, Address, Log},
    Inspector,
};

use crate::{buffer, context::ArbitrumContextTr, frame::ArbitrumFrameTr};

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
        let api = self.handler.clone();
        (api)(req_type, data)
    }
}

fn wasm_account_touch<CTX>(context: CTX, is_cold: bool, with_code: bool) -> u64
where
    CTX: ArbitrumContextTr,
{
    let code_cost = if with_code {
        context.cfg().max_code_size() as u64 / 24576 * 700
    } else {
        0
    };
    code_cost + warm_cold_cost(is_cold)
}

pub(crate) fn request<EVM, HANDLER, FRAME>(
    frame: &mut FRAME,
    evm: &mut EVM,
    handler: &mut HANDLER,
    input: InputsImpl,
    is_static: bool,
    req_type: EvmApiMethod,
    data: Vec<u8>,
) -> (Vec<u8>, VecReader, Gas)
where
    EVM: InspectorEvmTr<
        Context: ArbitrumContextTr,
        Inspector: Inspector<EVM::Context, EthInterpreter>,
    >,
    HANDLER: InspectorHandler<Evm = EVM, Frame = FRAME>,
    <EVM as InspectorEvmTr>::Inspector:
        Inspector<<EVM as EvmTr>::Context, <HANDLER as InspectorHandler>::IT>,
    FRAME: ArbitrumFrameTr<
            Evm = EVM,
            Error = <HANDLER as Handler>::Error,
            IT = <HANDLER as InspectorHandler>::IT,
        > + InspectorFrame<IT = <HANDLER as InspectorHandler>::IT>,
    <HANDLER as Handler>::Frame: InspectorFrame<IT = <HANDLER as InspectorHandler>::IT>,
    <FRAME as Frame>::FrameInit: From<FrameInput>,
    FrameResult: From<<FRAME as Frame>::FrameResult>,
    <FRAME as Frame>::FrameResult: From<FrameResult>,
{
    let mut data = data;

    match req_type {
        EvmApiMethod::ContractCall | EvmApiMethod::DelegateCall | EvmApiMethod::StaticCall => {
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

            if is_static && !value.is_zero() {
                return (
                    Status::WriteProtection.into(),
                    VecReader::new(vec![]),
                    Gas(gas_left),
                );
            }

            let gas_limit = if evm
                .ctx()
                .cfg()
                .spec()
                .into_eth_spec()
                .is_enabled_in(SpecId::TANGERINE)
            {
                min(gas_left - gas_left / 64, gas_limit)
            } else {
                gas_limit
            };

            let mut gas = RevmGas::new(gas_limit);
            gas.spend_all();

            let mut frame_input = FrameInput::Call(Box::new(CallInputs {
                input: CallInput::Bytes(calldata),
                return_memory_offset: 0..0,
                gas_limit,
                bytecode_address,
                target_address,
                caller,
                value: revm::interpreter::CallValue::Transfer(value),
                scheme: revm::interpreter::CallScheme::Call,
                is_static,
                is_eof: false,
            }));

            let new_frame = {
                let (context, inspector) = evm.ctx_inspector();
                frame_start::<_, EthInterpreter>(context, inspector, &mut frame_input);
                handler.frame_init(frame, evm, frame_input.clone().into())
            };

            let result = match new_frame {
                Ok(frame) => match frame {
                    FrameOrResult::Item(first_frame) => {
                        handler.inspect_run_exec_loop(evm, first_frame)
                    }
                    FrameOrResult::Result(result) => Ok(result.into()),
                },
                Err(_) => {
                    return (
                        Status::Failure.into(),
                        VecReader::new(vec![]),
                        Gas(gas_left),
                    );
                }
            };

            match result {
                Ok(frame_result) => {
                    // TODO implement a way to propogate the error
                    if handler
                        .frame_return_result(frame, evm, frame_result.clone().into())
                        .is_ok()
                    {
                        match frame_result {
                            FrameResult::Call(result) => {
                                gas.erase_cost(result.gas().remaining());
                                return (
                                    Status::Success.into(),
                                    VecReader::new(result.result.output.to_vec()),
                                    Gas(gas.spent()),
                                );
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }

            (vec![], VecReader::new(vec![]), Gas(gas.spent()))
        }

        EvmApiMethod::Create1 | EvmApiMethod::Create2 => {
            let is_create_2 = matches!(req_type, EvmApiMethod::Create2);
            let gas_remaining = buffer::take_u64(&mut data);
            let value = buffer::take_u256(&mut data);
            let salt = is_create_2.then(|| buffer::take_u256(&mut data));
            let init_code = buffer::take_rest(&mut data);

            let spec = evm.ctx().cfg().spec().into_eth_spec();

            if is_static {
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

            if is_create_2 && !spec.is_enabled_in(SpecId::PETERSBURG) {
                return error_response;
            }

            let mut gas_cost = 0;
            let len = init_code.len();

            if len != 0 && spec.is_enabled_in(SpecId::SHANGHAI) {
                let max_initcode_size = evm.ctx().cfg().max_code_size().saturating_mul(2);
                if len > max_initcode_size {
                    return error_response;
                }
                gas_cost = initcode_cost(len);
            }

            let scheme = if is_create_2 {
                if let Some(check_cost) =
                    create2_cost(len).and_then(|cost| gas_cost.checked_add(cost))
                {
                    gas_cost = check_cost;
                } else {
                    return error_response;
                };
                CreateScheme::Create2 {
                    salt: salt.unwrap(),
                }
            } else {
                gas_cost += CREATE;
                CreateScheme::Create
            };

            if gas_remaining < gas_cost {
                return (
                    [vec![0x00], "out of gas".as_bytes().to_vec()].concat(),
                    VecReader::new(vec![]),
                    Gas(gas_remaining),
                );
            }

            let gas_limit = gas_remaining - gas_cost;

            let gas_stipend = if spec.is_enabled_in(SpecId::TANGERINE) {
                gas_limit / 64
            } else {
                0
            };

            let mut gas = RevmGas::new(gas_limit);
            _ = gas.record_cost(gas_stipend);

            let mut frame_input = FrameInput::Create(Box::new(CreateInputs {
                caller: input.target_address,
                scheme,
                value,
                init_code,
                gas_limit: gas.remaining(),
            }));

            gas.spend_all();

            let new_frame = {
                let (context, inspector) = evm.ctx_inspector();
                frame_start::<_, EthInterpreter>(context, inspector, &mut frame_input);
                handler.frame_init(frame, evm, frame_input.clone().into())
            };

            let result = match new_frame {
                Ok(frame) => match frame {
                    FrameOrResult::Item(first_frame) => {
                        handler.inspect_run_exec_loop(evm, first_frame)
                    }
                    FrameOrResult::Result(result) => Ok(result.into()),
                },
                Err(_) => {
                    return (
                        Status::Failure.into(),
                        VecReader::new(vec![]),
                        Gas(gas.spent()),
                    );
                }
            };

            match result {
                Ok(frame_result) => {
                    // TODO implement a way to propogate the error
                    if handler
                        .frame_return_result(frame, evm, frame_result.clone().into())
                        .is_ok()
                    {
                        match frame_result {
                            FrameResult::Create(outcome) => {
                                if matches!(outcome.instruction_result(), InstructionResult::Revert)
                                {
                                    return (
                                        [vec![0x00], outcome.output().to_vec()].concat(),
                                        VecReader::new(vec![]),
                                        Gas(gas.spent()),
                                    );
                                }

                                if let Some(address) = outcome.address {
                                    gas.erase_cost(outcome.gas().remaining() + gas_stipend);

                                    return (
                                        [vec![0x01], address.to_vec()].concat(),
                                        VecReader::new(vec![]),
                                        Gas(gas.spent()),
                                    );
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }

            (
                [vec![0x01], Address::ZERO.to_vec()].concat(),
                VecReader::new(vec![]),
                Gas(gas.spent()),
            )
        }
        _ => {
            let (context, inspector) = evm.ctx_inspector();

            let spec = context.cfg().spec();

            match req_type {
                EvmApiMethod::GetBytes32 => {
                    let slot = buffer::take_u256(&mut data);
                    if let Some(result) = context.sload(input.target_address, slot) {
                        let gas = sload_cost(spec.into(), result.is_cold);
                        (result.to_be_bytes_vec(), VecReader::new(vec![]), Gas(gas))
                    } else {
                        (vec![], VecReader::new(vec![]), Gas(0))
                    }
                }

                EvmApiMethod::SetTrieSlots => {
                    let gas_left = buffer::take_u64(&mut data);

                    if is_static {
                        return (
                            Status::WriteProtection.into(),
                            VecReader::new(vec![]),
                            Gas(gas_left),
                        );
                    }

                    let mut total_cost = 0;
                    while !data.is_empty() {
                        let (key, value) =
                            (buffer::take_u256(&mut data), buffer::take_u256(&mut data));

                        match context.sstore(input.target_address, key, value) {
                            Some(result) => {
                                total_cost +=
                                    sstore_cost(spec.into(), &result.data, result.is_cold);

                                if gas_left < total_cost {
                                    return (
                                        Status::OutOfGas.into(),
                                        VecReader::new(vec![]),
                                        Gas(gas_left),
                                    );
                                }
                            }
                            _ => {
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
                    let result = context.tload(input.target_address, slot);
                    (result.to_be_bytes_vec(), VecReader::new(vec![]), Gas(0))
                }

                EvmApiMethod::SetTransientBytes32 => {
                    if is_static {
                        return (
                            Status::WriteProtection.into(),
                            VecReader::new(vec![]),
                            Gas(0),
                        );
                    }
                    let key = buffer::take_u256(&mut data);
                    let value = buffer::take_u256(&mut data);
                    context.tstore(input.target_address, key, value);
                    (Status::Success.into(), VecReader::new(vec![]), Gas(0))
                }

                EvmApiMethod::EmitLog => {
                    let topic_count = buffer::take_u32(&mut data);
                    let mut topics = Vec::with_capacity(topic_count as usize);
                    for _ in 0..topic_count {
                        topics.push(buffer::take_bytes32(&mut data));
                    }
                    let data = buffer::take_rest(&mut data);

                    let log = Log::new_unchecked(input.target_address, topics, data);

                    context.log(log.clone());

                    inspector.log(frame.interpreter(), context, log);

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
                    let code = context.load_account_code(address).unwrap();
                    let gas = wasm_account_touch(context, code.is_cold, true);
                    (vec![], VecReader::new(code.to_vec()), Gas(gas))
                }

                EvmApiMethod::AccountCodeHash => {
                    let address = buffer::take_address(&mut data);
                    let code_hash = context.load_account_code_hash(address).unwrap();
                    let gas = wasm_account_touch(context, code_hash.is_cold, false);
                    (code_hash.to_vec(), VecReader::new(vec![]), Gas(gas))
                }

                EvmApiMethod::AddPages => {
                    let _count = buffer::take_u16(&mut data);
                    (Status::Success.into(), VecReader::new(vec![]), Gas(0))
                }

                EvmApiMethod::CaptureHostIO => {
                    //let data = buffer::take_rest(&mut data);
                    //println!("CaptureHostIO: {:?}", String::from_utf8_lossy(&data));
                    (Status::Success.into(), VecReader::new(vec![]), Gas(0))
                }
                _ => unimplemented!("EVM API method not implemented: {:?}", req_type),
            }
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
