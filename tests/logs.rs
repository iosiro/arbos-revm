// Copyright 2024, Offchain Labs, Inc.
// For license information, see https://github.com/OffchainLabs/nitro/blob/master/LICENSE.md

//! Log emission tests for Stylus programs.

use revm::{
    InspectEvm, Inspector,
    context::{ContextTr, JournalTr, result::ExecutionResult},
    interpreter::{
        InstructionResult, Interpreter, InterpreterAction, interpreter_types::LoopControl,
    },
    primitives::{Address, Bytes, Log, U256},
};

mod test_utils;
use test_utils::{
    TestContext, create_call_tx, create_evm, deploy_wat_program, execute_tx, fund_account,
    setup_context_with_arbos_state,
};

#[derive(Default)]
struct LogInspector {
    logs: Vec<Log>,
    stop: bool,
}

impl Inspector<TestContext> for LogInspector {
    fn log_full(&mut self, interpreter: &mut Interpreter, _: &mut TestContext, log: Log) {
        assert_eq!(interpreter.input.target_address, log.address);
        self.logs.push(log);
        if self.stop {
            interpreter
                .bytecode
                .set_action(InterpreterAction::new_return(
                    InstructionResult::Revert,
                    Bytes::from_static(b"inspector rejected log"),
                    interpreter.gas,
                ));
        }
    }
}

#[test]
fn inspected_stylus_logs_use_full_callback() {
    let mut context = setup_context_with_arbos_state();
    let program = deploy_wat_program(&mut context, include_bytes!("../test-data/log.wat"));
    fund_account(
        &mut context,
        Address::repeat_byte(1),
        U256::from(1_000_000_000),
    );
    let mut evm = create_evm(context).with_inspector(LogInspector::default());
    let result = evm
        .inspect_one_tx(create_call_tx(program, vec![0, 42], 1_000_000).into())
        .unwrap();
    let ExecutionResult::Success { logs, .. } = result else {
        panic!("{result:?}")
    };
    assert_eq!(logs, evm.0.inspector.logs);
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].data.data.as_ref(), &[42]);
}

#[test]
fn log_inspector_revert_stops_wasm_and_rolls_back_storage() {
    let mut context = setup_context_with_arbos_state();
    let program = deploy_wat_program(
        &mut context,
        br#"
        (module
            (import "vm_hooks" "emit_log" (func $log (param i32 i32 i32)))
            (import "vm_hooks" "storage_cache_bytes32" (func $store (param i32 i32)))
            (import "vm_hooks" "storage_flush_cache" (func $flush (param i32)))
            (memory (export "memory") 1 1)
            (func (export "user_entrypoint") (param i32) (result i32)
                (i32.store8 (i32.const 63) (i32.const 1))
                (call $store (i32.const 0) (i32.const 32))
                (call $flush (i32.const 0))
                (call $log (i32.const 0) (i32.const 0) (i32.const 0))
                (call $log (i32.const 0) (i32.const 0) (i32.const 0))
                ;; If execution continues after rejection it burns all remaining gas.
                (loop $forever (br $forever))
                i32.const 0))
    "#,
    );
    fund_account(
        &mut context,
        Address::repeat_byte(1),
        U256::from(1_000_000_000),
    );
    let mut evm = create_evm(context).with_inspector(LogInspector {
        stop: true,
        ..Default::default()
    });
    let result = evm
        .inspect_one_tx(create_call_tx(program, vec![], 1_000_000).into())
        .unwrap();
    let ExecutionResult::Revert { output, gas, .. } = result else {
        panic!("{result:?}")
    };
    assert_eq!(output.as_ref(), b"inspector rejected log");
    assert!(
        gas.tx_gas_used() < 500_000,
        "execution continued after the inspector stopped it"
    );
    assert_eq!(evm.0.inspector.logs.len(), 1);
    assert_eq!(
        evm.0
            .ctx
            .journal_mut()
            .sload(program, U256::ZERO)
            .unwrap()
            .data,
        U256::ZERO
    );
}

#[test]
fn inspected_stylus_logs_preserve_legacy_callback() {
    #[derive(Default)]
    struct LegacyInspector(usize);
    impl Inspector<TestContext> for LegacyInspector {
        fn log(&mut self, _: &mut TestContext, _: Log) {
            self.0 += 1;
        }
    }
    let mut context = setup_context_with_arbos_state();
    let program = deploy_wat_program(&mut context, include_bytes!("../test-data/log.wat"));
    fund_account(
        &mut context,
        Address::repeat_byte(1),
        U256::from(1_000_000_000),
    );
    let mut evm = create_evm(context).with_inspector(LegacyInspector::default());
    let result = evm
        .inspect_one_tx(create_call_tx(program, vec![0], 1_000_000).into())
        .unwrap();
    assert!(result.is_success());
    assert_eq!(evm.0.inspector.0, 1);
}

#[test]
fn test_e2e_log_no_topics() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/log.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);

    let log_data = b"Hello, logs!";
    let mut args = vec![0x00u8];
    args.extend_from_slice(log_data);

    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { logs, .. } => {
            assert_eq!(logs.len(), 1, "should emit one log");
            let log = &logs[0];
            assert_eq!(
                log.address, program_address,
                "log address should match program"
            );
            assert!(log.topics().is_empty(), "should have no topics");
            assert_eq!(log.data.data.as_ref(), log_data, "log data should match");
        }
        ExecutionResult::Revert { output, .. } => {
            panic!("execution reverted: {:?}", output);
        }
        ExecutionResult::Halt { reason, .. } => {
            panic!("execution halted: {:?}", reason);
        }
    }
}

#[test]
fn test_e2e_log_one_topic() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/log.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);

    let topic = [0xABu8; 32];
    let log_data = b"One topic log";
    let mut args = vec![0x01u8];
    args.extend_from_slice(&topic);
    args.extend_from_slice(log_data);

    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { logs, .. } => {
            assert_eq!(logs.len(), 1, "should emit one log");
            let log = &logs[0];
            assert_eq!(
                log.address, program_address,
                "log address should match program"
            );
            assert_eq!(log.topics().len(), 1, "should have one topic");
            assert_eq!(log.topics()[0].as_slice(), &topic, "topic should match");
            assert_eq!(log.data.data.as_ref(), log_data, "log data should match");
        }
        ExecutionResult::Revert { output, .. } => {
            panic!("execution reverted: {:?}", output);
        }
        ExecutionResult::Halt { reason, .. } => {
            panic!("execution halted: {:?}", reason);
        }
    }
}

#[test]
fn test_e2e_log_four_topics() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/log.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);

    let topics = [[0x11u8; 32], [0x22u8; 32], [0x33u8; 32], [0x44u8; 32]];
    let log_data = b"Four topics log";
    let mut args = vec![0x04u8];
    for topic in &topics {
        args.extend_from_slice(topic);
    }
    args.extend_from_slice(log_data);

    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { logs, .. } => {
            assert_eq!(logs.len(), 1, "should emit one log");
            let log = &logs[0];
            assert_eq!(
                log.address, program_address,
                "log address should match program"
            );
            assert_eq!(log.topics().len(), 4, "should have four topics");
            for (i, expected_topic) in topics.iter().enumerate() {
                assert_eq!(
                    log.topics()[i].as_slice(),
                    expected_topic,
                    "topic {} should match",
                    i
                );
            }
            assert_eq!(log.data.data.as_ref(), log_data, "log data should match");
        }
        ExecutionResult::Revert { output, .. } => {
            panic!("execution reverted: {:?}", output);
        }
        ExecutionResult::Halt { reason, .. } => {
            panic!("execution halted: {:?}", reason);
        }
    }
}

#[test]
fn test_e2e_log_empty_data() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/log.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);

    let topics = [[0xAAu8; 32], [0xBBu8; 32]];
    let mut args = vec![0x02u8];
    for topic in &topics {
        args.extend_from_slice(topic);
    }

    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { logs, .. } => {
            assert_eq!(logs.len(), 1, "should emit one log");
            let log = &logs[0];
            assert_eq!(log.topics().len(), 2, "should have two topics");
            assert!(log.data.data.is_empty(), "log data should be empty");
        }
        ExecutionResult::Revert { output, .. } => {
            panic!("execution reverted: {:?}", output);
        }
        ExecutionResult::Halt { reason, .. } => {
            panic!("execution halted: {:?}", reason);
        }
    }
}
