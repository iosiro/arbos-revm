use core::mem;
use std::num::NonZeroUsize;
use std::{boxed::Box, vec::Vec};

use crate::constants::STYLUS_DISCRIMINANT;
use crate::frame::ArbitrumFrameTr;
use crate::stylus_api::{self, StylusHandler};
use crate::{ArbitrumCfgTr, ArbitrumContextTr};
use arbutil::{
    evm::{
        api::{EvmApiMethod, VecReader},
        req::EvmApiRequestor,
        user::{UserOutcome, UserOutcomeKind},
        EvmData,
    },
    Bytes20, Bytes32,
};

use lru::LruCache;
use revm::context::{JournalTr, LocalContextTr};
use revm::handler::{Frame, FrameResult};
use revm::inspector::{InspectorEvmTr, InspectorFrame, InspectorHandler};
use revm::interpreter::interpreter::EthInterpreter;
use revm::interpreter::interpreter_types::{InputsTr, LegacyBytecode, LoopControl, RuntimeFlag};
use revm::interpreter::{CallInput, FrameInput, InterpreterAction};
use revm::primitives::{Bytes, FixedBytes};
use revm::Inspector;
use revm::{
    context::{Block, Cfg, ContextTr, Transaction},
    handler::{EvmTr, Handler},
    interpreter::{Gas, InputsImpl, InterpreterResult},
    primitives::{alloy_primitives::U64, keccak256, U256},
};
use stylus::brotli::{self, Dictionary};
use stylus::prover::programs::meter::MeteredMachine;
use stylus::{
    native::{self, NativeInstance},
    prover::programs::config::{CompileConfig, StylusConfig},
    run::RunProgram,
};
use wasmer_types::lib::std::sync::{Arc, Mutex};

type EvmApiHandler<'a> =
    Arc<Box<dyn Fn(EvmApiMethod, Vec<u8>) -> (Vec<u8>, VecReader, arbutil::evm::api::Gas) + 'a>>;

lazy_static::lazy_static! {
    pub static ref PROGRAM_CACHE: Mutex<LruCache<FixedBytes<32>, Vec<u8>>> = Mutex::new(LruCache::new(NonZeroUsize::new(1024).unwrap()));
}

pub fn run<EVM, HANDLER, FRAME>(
    frame: &mut FRAME,
    evm: &mut EVM,
    handler: &mut HANDLER,
) -> Option<InterpreterAction>
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
    let interpreter = frame.interpreter();
    let bytecode_address = if let Some(bytecode_address) = interpreter.input.bytecode_address().copied() {
        bytecode_address
    } else {
        return None;
    };

    let code_hash = if let Ok(code_hash) = evm
        .ctx()
        .journal()
        .code_hash(bytecode_address)
    {
        code_hash.data
    } else {
        return None;
    };

    let context = evm.ctx();
    let config_env = context.cfg();

    let target_address = interpreter.input.target_address();
    let caller_address = interpreter.input.caller_address();
    let call_value = interpreter.input.call_value();
    
    let is_static = interpreter.runtime_flag.is_static();
    let gas_limit = interpreter.control.gas().limit();

    let compile_config =
        CompileConfig::version(config_env.stylus_version(), config_env.debug_mode());
    let stylus_config = StylusConfig::new(
        config_env.stylus_version(),
        config_env.max_depth(),
        config_env.ink_price(),
    );

    let serialized = {
        let mut cache = PROGRAM_CACHE.lock().unwrap();

        if let Ok(serialized) = cache.try_get_or_insert(code_hash, || {
            if let Some(bytecode) = interpreter
                .bytecode
                .bytecode_slice()
                .strip_prefix(STYLUS_DISCRIMINANT)
            {
                let (dictionary, compressed_bytecode) =
                    if let Some((dictionary, compressed_bytecode)) = bytecode.split_at_checked(1) {
                        (dictionary, compressed_bytecode)
                    } else {
                        return Err(());
                    };

                let dictionary = match dictionary[0] {
                    0x00 => Dictionary::Empty,
                    0x01 => Dictionary::StylusProgram,
                    _ => unreachable!(),
                };

                let bytecode = brotli::decompress(compressed_bytecode, dictionary)
                    .or_else(|err| {
                        if dictionary == Dictionary::Empty {
                            Ok(compressed_bytecode.to_vec())
                        } else {
                            Err(err)
                        }
                    })
                    .unwrap();

                let mut activation_gas = gas_limit;
                native::activate(
                    bytecode.as_slice(),
                    &Bytes32::from(code_hash.0),
                    config_env.stylus_version(),
                    config_env.arbos_version() as u64,
                    128,
                    false,
                    &mut activation_gas,
                )
                .unwrap();

                return Ok(native::compile(
                    bytecode.as_slice(),
                    compile_config.version,
                    false,
                    wasmer_types::compilation::target::Target::default(),
                )
                .unwrap());
            }

            Err(())
        }) {
            serialized.clone()
        } else {
            return None;
        }
    };

    let calldata = match interpreter.input.input() {
        CallInput::Bytes(calldata) => calldata.clone(),
        CallInput::SharedBuffer(range) => {
            if let Some(slice) = context.local().shared_memory_buffer_slice(range.clone()) {
                Bytes::from(slice.to_vec())
            } else {
                Bytes::new()
            }
        }
    };

    let inputs = InputsImpl {
        target_address,
        caller_address,
        input: revm::interpreter::CallInput::Bytes(Bytes::from(calldata.to_vec())),
        call_value,
        bytecode_address: Some(bytecode_address),
    };

    let evm_data = build_evm_data(context, inputs.clone());

    let evm = Arc::new(Mutex::new(evm));
    let handler = Arc::new(Mutex::new(handler));
    let frame = Arc::new(Mutex::new(frame));

    let callback = {
        let evm = evm.clone();
        let inputs = inputs.clone();

        move |req_type: arbutil::evm::api::EvmApiMethod,
              req_data: Vec<u8>|
              -> (Vec<u8>, VecReader, arbutil::evm::api::Gas) {
            let mut evm = evm.lock().unwrap();
            let mut handler = handler.lock().unwrap();
            let mut frame = frame.lock().unwrap();

            stylus_api::request::<EVM, HANDLER, FRAME>(
                *frame,
                *evm,
                *handler,
                inputs.clone(),
                is_static,
                req_type,
                req_data,
            )
        }
    };

    let callback: EvmApiHandler<'_> = Arc::new(Box::new(callback));
    let unsafe_callback: &'static EvmApiHandler<'_> = unsafe { mem::transmute(&callback) };
    let evm_api = EvmApiRequestor::new(StylusHandler::new(unsafe_callback.clone()));

    let mut instance = unsafe {
        NativeInstance::deserialize(serialized.as_slice(), compile_config, evm_api, evm_data)
            .unwrap()
    };

    let ink_limit = stylus_config
        .pricing
        .gas_to_ink(arbutil::evm::api::Gas(gas_limit));
    let mut gas = Gas::new(gas_limit);
    gas.spend_all();

    let bytecode = match inputs.input() {
        CallInput::Bytes(bytes) => bytes,
        CallInput::SharedBuffer(_) => todo!(),
    };

    let outcome = match instance.run_main(bytecode, stylus_config, ink_limit) {
        Err(e) | Ok(UserOutcome::Failure(e)) => UserOutcome::Failure(e.wrap_err("call failed")),
        Ok(outcome) => outcome,
    };

    let mut gas_left = stylus_config
        .pricing
        .ink_to_gas(instance.ink_left().into())
        .0;

    let (kind, data) = outcome.into_data();

    let result = match kind {
        UserOutcomeKind::Success => revm::interpreter::InstructionResult::Return,
        UserOutcomeKind::Revert => revm::interpreter::InstructionResult::Revert,
        UserOutcomeKind::Failure => revm::interpreter::InstructionResult::Revert,
        UserOutcomeKind::OutOfInk => revm::interpreter::InstructionResult::OutOfGas,
        UserOutcomeKind::OutOfStack => {
            gas_left = 0;
            revm::interpreter::InstructionResult::StackOverflow
        }
    };

    gas.erase_cost(gas_left);

    Some(InterpreterAction::Return {
        result: InterpreterResult {
            result,
            output: data.into(),
            gas,
        },
    })
}

fn build_evm_data<CTX>(context: &mut CTX, input: InputsImpl) -> EvmData
where
    CTX: ArbitrumContextTr,
{
    // find target_address in context.evm.journaled_state.call_stack excluding last
    // if found, set reentrant to true
    // else set reentrant to false
    // let reentrant = if context
    //     .evm
    //     .journaled_state
    //     .call_stack
    //     .iter()
    //     .filter(|&x| *x == self.inputs.target_address)
    //     .count()
    //     > 1
    // {
    //     1
    // } else {
    //     0
    // };
    let reentrant = 0;

    let config_env = context.cfg();

    let block_env = context.block();
    let tx_env = context.tx();

    let base_fee = block_env.basefee();

    let evm_data: EvmData = EvmData {
        arbos_version: config_env.arbos_version() as u64,
        block_basefee: Bytes32::from(U256::from(base_fee).to_be_bytes()),
        chainid: config_env.chain_id(),
        block_coinbase: Bytes20::try_from(block_env.beneficiary().as_slice()).unwrap(),
        block_gas_limit: U64::wrapping_from(block_env.gas_limit()).to::<u64>(),
        block_number: U64::wrapping_from(block_env.number()).to::<u64>(),
        block_timestamp: U64::wrapping_from(block_env.timestamp()).to::<u64>(),
        contract_address: Bytes20::try_from(input.target_address.as_slice()).unwrap(),
        module_hash: Bytes32::try_from(keccak256(input.target_address.as_slice()).as_slice())
            .unwrap(),
        msg_sender: Bytes20::try_from(input.caller_address.as_slice()).unwrap(),
        msg_value: Bytes32::try_from(input.call_value.to_be_bytes_vec()).unwrap(),
        tx_gas_price: Bytes32::from(
            U256::from(tx_env.effective_gas_price(base_fee as u128)).to_be_bytes(),
        ),
        tx_origin: Bytes20::try_from(tx_env.caller().as_slice()).unwrap(),
        reentrant,
        return_data_len: 0,
        cached: true,
        tracing: true,
    };

    evm_data
}
