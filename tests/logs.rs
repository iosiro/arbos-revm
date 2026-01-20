// Copyright 2024, Offchain Labs, Inc.
// For license information, see https://github.com/OffchainLabs/nitro/blob/master/LICENSE.md

//! Log emission tests for Stylus programs.

use revm::{
    context::result::ExecutionResult,
    primitives::{Address, U256},
};

mod test_utils;
use test_utils::{
    create_call_tx, create_evm, deploy_wat_program, execute_tx, fund_account,
    setup_context_with_arbos_state,
};

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
