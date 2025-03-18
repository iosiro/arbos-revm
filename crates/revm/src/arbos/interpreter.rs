use core::mem;
use std::sync::{Arc, Mutex};

use arbutil::{
    evm::{
        api::{EvmApiMethod, VecReader},
        req::EvmApiRequestor,
        user::{UserOutcome, UserOutcomeKind},
        EvmData,
    },
    Bytes20, Bytes32,
};
use revm_interpreter::{Gas, Host, Interpreter, InterpreterAction, InterpreterResult};
use stylus::{
    brotli::{self, Dictionary}, native::{self, NativeInstance}, prover::programs::{
        config::{CompileConfig, StylusConfig},
        meter::MeteredMachine,
    }, run::RunProgram
};

use crate::{
    primitives::{keccak256, U256, U64},
    Context, Database, Frame,
};

use super::handler::StylusHandler;

type EvmApiHandler<'a> =
    Arc<Box<dyn Fn(EvmApiMethod, Vec<u8>) -> (Vec<u8>, VecReader, arbutil::evm::api::Gas) + 'a>>;

const STYLUS_EOF_MAGIC: u8 = 0xEF;
const STYLUS_EOF_MAGIC_SUFFIX: u8 = 0xF0;
const STYLUS_EOF_VERSION: u8 = 0x00;

pub const STYLUS_DISCRIMINANT: &[u8] = &[
    STYLUS_EOF_MAGIC,
    STYLUS_EOF_MAGIC_SUFFIX,
    STYLUS_EOF_VERSION,
];

pub fn is_stylus_bytecode(bytecode: &[u8]) -> bool {
    bytecode.starts_with(STYLUS_DISCRIMINANT) && bytecode.len() > STYLUS_DISCRIMINANT.len() + 1
}

pub fn run_stylus_interpreter<EXT, DB: Database>(
    context: &mut crate::Context<EXT, DB>,
    handler: &crate::Handler<'_, crate::Context<EXT, DB>, EXT, DB>,
    stack_frame: &mut Frame,
) -> revm_interpreter::InterpreterAction {
    let evm_data = build_evm_data(context, stack_frame.interpreter());

    let arbos_cfg = context.env().cfg.arbos_config.clone().unwrap_or_default();
    let compile_config = CompileConfig::version(arbos_cfg.stylus_version, arbos_cfg.debug_mode);
    let stylus_config = StylusConfig::new(
        arbos_cfg.stylus_version,
        arbos_cfg.max_depth,
        arbos_cfg.ink_price,
    );

    let prefixed_bytecode = stack_frame
        .interpreter()
        .contract()
        .bytecode
        .original_bytes();

    let bytecode = prefixed_bytecode.strip_prefix(STYLUS_DISCRIMINANT).unwrap();

    let (dictionary, compressed_bytecode) = bytecode.split_at(1);

    let dictionary = match dictionary[0] {
        0x00 => Dictionary::Empty,
        0x01 => Dictionary::StylusProgram,
        _ => unreachable!(),
    };

    let bytecode = brotli::decompress(compressed_bytecode, dictionary).or_else(|err| {
        if dictionary == Dictionary::Empty {
            Ok(compressed_bytecode.to_vec())
        } else {
            Err(err)
        }
    }).unwrap();

    let calldata = stack_frame.interpreter().contract().input.clone();

    let ink_limit = stylus_config.pricing.gas_to_ink(arbutil::evm::api::Gas(
        stack_frame.interpreter().gas().limit(),
    ));
    let mut gas = Gas::new(stack_frame.interpreter().gas().limit());
    gas.spend_all();

    let context = Arc::new(Mutex::new(context));
    let handler = Arc::new(Mutex::new(handler));
    let stack_frame = Arc::new(Mutex::new(stack_frame));

    let callback = {
        let context = context.clone();
        let handler = handler.clone();

        move |req_type: arbutil::evm::api::EvmApiMethod,
              req_data: Vec<u8>|
              -> (Vec<u8>, VecReader, arbutil::evm::api::Gas) {
            let mut ctx = context.lock().unwrap();
            let handler = handler.lock().unwrap();
            let mut stack_frame = stack_frame.lock().unwrap();
            super::handler::request(*ctx, *handler, &mut stack_frame, req_type, req_data)
        }
    };

    let callback: EvmApiHandler<'_> = Arc::new(Box::new(callback));
    let unsafe_callback: &'static EvmApiHandler<'_> = unsafe { mem::transmute(&callback) };
    let evm_api = EvmApiRequestor::new(StylusHandler::new(unsafe_callback.clone()));

    let serialized = native::compile(
        bytecode.as_slice(),
        compile_config.version,
        false,
        wasmer_types::compilation::target::Target::default(),
    )
    .unwrap();
    let mut instance = unsafe {
        NativeInstance::deserialize(serialized.as_slice(), compile_config, evm_api, evm_data)
            .unwrap()
    };

    let outcome = match instance.run_main(&calldata, stylus_config, ink_limit) {
        Err(e) | Ok(UserOutcome::Failure(e)) => UserOutcome::Failure(e.wrap_err("call failed")),
        Ok(outcome) => outcome,
    };

    let mut gas_left = stylus_config
        .pricing
        .ink_to_gas(instance.ink_left().into())
        .0;

    let (kind, data) = outcome.into_data();

    let result = match kind {
        UserOutcomeKind::Success => crate::interpreter::InstructionResult::Return,
        UserOutcomeKind::Revert => crate::interpreter::InstructionResult::Revert,
        UserOutcomeKind::Failure => crate::interpreter::InstructionResult::Revert,
        UserOutcomeKind::OutOfInk => crate::interpreter::InstructionResult::OutOfGas,
        UserOutcomeKind::OutOfStack => {
            gas_left = 0;
            crate::interpreter::InstructionResult::StackOverflow
        }
    };

    gas.erase_cost(gas_left);

    InterpreterAction::Return {
        result: InterpreterResult {
            result,
            output: data.into(),
            gas,
        },
    }
}

fn build_evm_data<EXT, DB: Database>(
    context: &Context<EXT, DB>,
    interpreter: &Interpreter,
) -> EvmData {
    // find target_address in context.evm.journaled_state.call_stack excluding last
    // if found, set reentrant to true
    // else set reentrant to false
    let reentrant = if context
        .evm
        .journaled_state
        .call_stack
        .iter()
        .filter(|&x| *x == interpreter.contract().target_address)
        .count()
        > 1
    {
        1
    } else {
        0
    };

    let evm_data: EvmData = EvmData {
        arbos_version: context
            .env()
            .cfg
            .arbos_config
            .clone()
            .unwrap_or_default()
            .arbos_version as u64,
        block_basefee: Bytes32::from(U256::from(context.env().block.basefee).to_be_bytes()),
        chainid: context.env().cfg.chain_id,
        block_coinbase: Bytes20::try_from(context.env().block.coinbase.as_slice()).unwrap(),
        block_gas_limit: U64::wrapping_from(context.env().block.gas_limit).to::<u64>(),
        block_number: U64::wrapping_from(context.env().block.number).to::<u64>(),
        block_timestamp: U64::wrapping_from(context.env().block.timestamp).to::<u64>(),
        contract_address: Bytes20::try_from(interpreter.contract().target_address.as_slice())
            .unwrap(),
        module_hash: Bytes32::try_from(
            keccak256(interpreter.contract().target_address.as_slice()).as_slice(),
        )
        .unwrap(),
        msg_sender: Bytes20::try_from(interpreter.contract().caller.as_slice()).unwrap(),
        msg_value: Bytes32::try_from(interpreter.contract().call_value.to_be_bytes_vec()).unwrap(),
        tx_gas_price: Bytes32::from(U256::from(context.env().effective_gas_price()).to_be_bytes()),
        tx_origin: Bytes20::try_from(context.env().tx.caller.as_slice()).unwrap(),
        reentrant,
        return_data_len: 0,
        cached: false,
        tracing: false,
    };

    evm_data
}
//}
