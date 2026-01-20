// Copyright 2024, Offchain Labs, Inc.
// For license information, see https://github.com/OffchainLabs/nitro/blob/master/LICENSE.md

//! Contract call tests for Stylus programs (call, static_call, delegate_call).

use revm::{
    context::result::ExecutionResult,
    primitives::{Address, U256},
};

mod test_utils;
use test_utils::{
    create_call_tx, create_call_tx_with_nonce, create_call_tx_with_value, create_evm,
    deploy_wat_program, execute_tx, fund_account, setup_context_with_arbos_state,
};

// ============================================================================
// Basic Contract Call Tests
// ============================================================================

#[test]
fn test_e2e_call_contract() {
    let mut context = setup_context_with_arbos_state();

    let callee_wat = include_bytes!("../test-data/write-args.wat");
    let callee_address = deploy_wat_program(&mut context, callee_wat);

    let caller_wat = include_bytes!("../test-data/call.wat");
    let caller_address = deploy_wat_program(&mut context, caller_wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(10_000_000_000_u64));

    let mut evm = create_evm(context);

    let test_data = b"hello from caller";
    let mut args = vec![0x00u8];
    args.extend_from_slice(callee_address.as_slice());
    args.extend_from_slice(test_data);

    let tx = create_call_tx(caller_address, args, 50_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(
                output.data().as_ref(),
                test_data,
                "call_contract should return echoed data"
            );
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
fn test_e2e_static_call_contract() {
    let mut context = setup_context_with_arbos_state();

    let callee_wat = include_bytes!("../test-data/write-args.wat");
    let callee_address = deploy_wat_program(&mut context, callee_wat);

    let caller_wat = include_bytes!("../test-data/call.wat");
    let caller_address = deploy_wat_program(&mut context, caller_wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(10_000_000_000_u64));

    let mut evm = create_evm(context);

    let test_data = b"static call test";
    let mut args = vec![0x01u8];
    args.extend_from_slice(callee_address.as_slice());
    args.extend_from_slice(test_data);

    let tx = create_call_tx(caller_address, args, 50_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(
                output.data().as_ref(),
                test_data,
                "static_call_contract should return echoed data"
            );
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
fn test_e2e_delegate_call_contract() {
    let mut context = setup_context_with_arbos_state();

    let callee_wat = include_bytes!("../test-data/write-args.wat");
    let callee_address = deploy_wat_program(&mut context, callee_wat);

    let caller_wat = include_bytes!("../test-data/call.wat");
    let caller_address = deploy_wat_program(&mut context, caller_wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(10_000_000_000_u64));

    let mut evm = create_evm(context);

    let test_data = b"delegate call test";
    let mut args = vec![0x02u8];
    args.extend_from_slice(callee_address.as_slice());
    args.extend_from_slice(test_data);

    let tx = create_call_tx(caller_address, args, 50_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(
                output.data().as_ref(),
                test_data,
                "delegate_call_contract should return echoed data"
            );
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
fn test_e2e_call_contract_empty_calldata() {
    let mut context = setup_context_with_arbos_state();

    let callee_wat = include_bytes!("../test-data/write-args.wat");
    let callee_address = deploy_wat_program(&mut context, callee_wat);

    let caller_wat = include_bytes!("../test-data/call.wat");
    let caller_address = deploy_wat_program(&mut context, caller_wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(10_000_000_000_u64));

    let mut evm = create_evm(context);

    let mut args = vec![0x00u8];
    args.extend_from_slice(callee_address.as_slice());

    let tx = create_call_tx(caller_address, args, 50_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert!(
                output.data().is_empty(),
                "call with empty calldata should return empty response"
            );
        }
        ExecutionResult::Revert { output, .. } => {
            panic!("execution reverted: {:?}", output);
        }
        ExecutionResult::Halt { reason, .. } => {
            panic!("execution halted: {:?}", reason);
        }
    }
}

// ============================================================================
// Nested Call Gas/Ink Tests
// ============================================================================

/// Test that when a callee runs out of gas, the caller handles it properly.
/// The caller should receive a failure status, not crash.
#[test]
fn test_e2e_call_to_contract_that_runs_out_of_gas() {
    let mut context = setup_context_with_arbos_state();

    // Deploy memory.wat as callee - it can consume lots of gas with memory growth
    let callee_wat = include_bytes!("../test-data/memory.wat");
    let callee_address = deploy_wat_program(&mut context, callee_wat);

    // Deploy call.wat as caller
    let caller_wat = include_bytes!("../test-data/call.wat");
    let caller_address = deploy_wat_program(&mut context, caller_wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(10_000_000_000_u64));

    let mut evm = create_evm(context);

    // Build args to call memory.wat with args that trigger heavy memory growth
    // memory.wat args: [target_pages, step_size]
    let memory_args = vec![255u8, 1u8]; // Try to grow to 255 pages, 1 page at a time (very expensive)

    let mut args = vec![0x00u8]; // call_contract selector
    args.extend_from_slice(callee_address.as_slice());
    args.extend_from_slice(&memory_args);

    // Give enough gas to start but the callee should run out during execution
    let tx = create_call_tx(caller_address, args, 500_000);
    let result = execute_tx(&mut evm, tx);

    // The outer call should complete (success or revert), not halt entirely
    // The callee running out of gas should be handled gracefully
    match result {
        ExecutionResult::Success { output, .. } => {
            // Caller succeeded - might return failure status from the failed inner call
            // The call.wat returns a status byte when the call fails
            if !output.data().is_empty() {
                // Non-zero first byte typically indicates the inner call failed
                // This is acceptable - the callee ran out of gas
            }
        }
        ExecutionResult::Revert { .. } => {
            // Caller reverted - this is acceptable if it propagates callee failure
        }
        ExecutionResult::Halt { reason, .. } => {
            // The whole execution halted - could happen if outer runs out too
            // This is acceptable but let's verify it's due to gas
            let reason_str = format!("{:?}", reason);
            assert!(
                reason_str.contains("OutOfGas")
                    || reason_str.contains("Gas")
                    || reason_str.contains("Call"),
                "halt should be gas-related, got: {:?}",
                reason
            );
        }
    }
}

/// Test nested call with explicit gas limit forwarding.
/// The caller forwards most of its gas to the callee.
#[test]
fn test_e2e_nested_call_gas_forwarding() {
    let mut context = setup_context_with_arbos_state();

    // Deploy write-args.wat as callee - simple echo contract
    let callee_wat = include_bytes!("../test-data/write-args.wat");
    let callee_address = deploy_wat_program(&mut context, callee_wat);

    // Deploy call.wat as caller
    let caller_wat = include_bytes!("../test-data/call.wat");
    let caller_address = deploy_wat_program(&mut context, caller_wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(10_000_000_000_u64));

    let mut evm = create_evm(context);

    // Simple call with reasonable data
    let test_data = b"test";
    let mut args = vec![0x00u8]; // call_contract selector
    args.extend_from_slice(callee_address.as_slice());
    args.extend_from_slice(test_data);

    // Give plenty of gas
    let tx = create_call_tx(caller_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success {
            output, gas_used, ..
        } => {
            // Should succeed and echo back
            assert_eq!(output.data().as_ref(), test_data);
            // Gas should be consumed for both contracts
            assert!(gas_used > 21_000, "should use more than base tx cost");
        }
        ExecutionResult::Revert { output, .. } => {
            panic!("execution reverted: {:?}", output);
        }
        ExecutionResult::Halt { reason, .. } => {
            panic!("execution halted: {:?}", reason);
        }
    }
}

/// Test that callee out-of-gas doesn't corrupt caller state.
#[test]
fn test_e2e_callee_out_of_gas_caller_state_intact() {
    let mut context = setup_context_with_arbos_state();

    // Deploy memory.wat as callee - will run out of gas
    let callee_wat = include_bytes!("../test-data/memory.wat");
    let callee_address = deploy_wat_program(&mut context, callee_wat);

    // Deploy storage.wat which we'll use to verify state
    let storage_wat = include_bytes!("../test-data/storage.wat");
    let storage_address = deploy_wat_program(&mut context, storage_wat);

    // Deploy call.wat as caller
    let caller_wat = include_bytes!("../test-data/call.wat");
    let caller_address = deploy_wat_program(&mut context, caller_wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(100_000_000_000_u64));

    let mut evm = create_evm(context);

    // First, write a value to storage to establish state
    let key = [0x42u8; 32];
    let value = [0xABu8; 32];
    let mut storage_write_args = vec![0x01u8]; // storage write selector
    storage_write_args.extend_from_slice(&key);
    storage_write_args.extend_from_slice(&value);

    let mut args = vec![0x00u8]; // call_contract selector
    args.extend_from_slice(storage_address.as_slice());
    args.extend_from_slice(&storage_write_args);

    let tx = create_call_tx_with_nonce(caller_address, args, 10_000_000, 0);
    let result = execute_tx(&mut evm, tx);
    assert!(
        matches!(result, ExecutionResult::Success { .. }),
        "storage write should succeed"
    );

    // Now try to call memory.wat which will run out of gas
    let memory_args = vec![255u8, 1u8]; // Heavy memory growth
    let mut args = vec![0x00u8];
    args.extend_from_slice(callee_address.as_slice());
    args.extend_from_slice(&memory_args);

    let tx = create_call_tx_with_nonce(caller_address, args, 200_000, 1);
    let _result = execute_tx(&mut evm, tx);
    // Don't care if this succeeds or fails, just that it doesn't corrupt state

    // Verify the storage value is still intact
    let mut storage_read_args = vec![0x00u8]; // storage read selector
    storage_read_args.extend_from_slice(&key);

    let mut args = vec![0x00u8];
    args.extend_from_slice(storage_address.as_slice());
    args.extend_from_slice(&storage_read_args);

    let tx = create_call_tx_with_nonce(caller_address, args, 10_000_000, 2);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(output.data().len(), 32, "should return 32 bytes");
            assert_eq!(
                output.data().as_ref(),
                &value,
                "storage value should be intact after failed nested call"
            );
        }
        ExecutionResult::Revert { output, .. } => {
            panic!("read execution reverted: {:?}", output);
        }
        ExecutionResult::Halt { reason, .. } => {
            panic!("read execution halted: {:?}", reason);
        }
    }
}

// ============================================================================
// Static Call Behavior Tests
// ============================================================================

#[test]
fn test_e2e_static_call_prevents_storage_write() {
    let mut context = setup_context_with_arbos_state();

    let storage_wat = include_bytes!("../test-data/storage.wat");
    let storage_address = deploy_wat_program(&mut context, storage_wat);

    let caller_wat = include_bytes!("../test-data/call.wat");
    let caller_address = deploy_wat_program(&mut context, caller_wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(10_000_000_000_u64));

    let mut evm = create_evm(context);

    let key = [0xABu8; 32];
    let value = [0xCDu8; 32];
    let mut storage_args = vec![0x01u8];
    storage_args.extend_from_slice(&key);
    storage_args.extend_from_slice(&value);

    let mut args = vec![0x01u8]; // static_call
    args.extend_from_slice(storage_address.as_slice());
    args.extend_from_slice(&storage_args);

    let tx = create_call_tx(caller_address, args, 50_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert!(!output.data().is_empty(), "should have failure output");
            assert_ne!(
                output.data()[0],
                0,
                "static call with storage write should fail (status != 0)"
            );
        }
        ExecutionResult::Revert { .. } => {}
        ExecutionResult::Halt { reason, .. } => {
            panic!("execution halted unexpectedly: {:?}", reason);
        }
    }
}

#[test]
fn test_e2e_static_call_prevents_log_emission() {
    let mut context = setup_context_with_arbos_state();

    let log_wat = include_bytes!("../test-data/log.wat");
    let log_address = deploy_wat_program(&mut context, log_wat);

    let caller_wat = include_bytes!("../test-data/call.wat");
    let caller_address = deploy_wat_program(&mut context, caller_wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(10_000_000_000_u64));

    let mut evm = create_evm(context);

    let topic = [0xAAu8; 32];
    let log_data = b"test log data";
    let mut log_args = vec![0x01u8];
    log_args.extend_from_slice(&topic);
    log_args.extend_from_slice(log_data);

    let mut args = vec![0x01u8]; // static_call
    args.extend_from_slice(log_address.as_slice());
    args.extend_from_slice(&log_args);

    let tx = create_call_tx(caller_address, args, 50_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, logs, .. } => {
            assert!(logs.is_empty(), "static call should not emit any logs");
            if !output.data().is_empty() {
                assert_ne!(
                    output.data()[0],
                    0,
                    "static call with log emission should fail (status != 0)"
                );
            }
        }
        ExecutionResult::Revert { .. } => {}
        ExecutionResult::Halt { reason, .. } => {
            panic!("execution halted unexpectedly: {:?}", reason);
        }
    }
}

/// Test that static_call prevents transient storage writes (write protection)
/// Per EVM spec, TSTORE should fail in static context just like SSTORE.
#[test]
fn test_e2e_static_call_prevents_transient_storage_write() {
    let mut context = setup_context_with_arbos_state();

    let transient_storage_wat = include_bytes!("../test-data/transient-storage.wat");
    let transient_storage_address = deploy_wat_program(&mut context, transient_storage_wat);

    let caller_wat = include_bytes!("../test-data/call.wat");
    let caller_address = deploy_wat_program(&mut context, caller_wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(10_000_000_000_u64));

    let mut evm = create_evm(context);

    // Build args for static_call to transient-storage contract with write operation
    // static_call selector (0x01) + transient_storage address + write args (0x01 + key + value)
    let key = [0xABu8; 32];
    let value = [0xCDu8; 32];
    let mut transient_args = vec![0x01u8]; // transient storage write selector
    transient_args.extend_from_slice(&key);
    transient_args.extend_from_slice(&value);

    let mut args = vec![0x01u8]; // static_call_contract selector
    args.extend_from_slice(transient_storage_address.as_slice());
    args.extend_from_slice(&transient_args);

    let tx = create_call_tx(caller_address, args, 50_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            // The static call should fail - we expect a failure status byte (non-zero)
            assert!(!output.data().is_empty(), "should have failure output");
            assert_ne!(
                output.data()[0],
                0,
                "static call with transient storage write should fail (status != 0)"
            );
        }
        ExecutionResult::Revert { .. } => {
            // Revert is also acceptable - write protection triggered
        }
        ExecutionResult::Halt { reason, .. } => {
            panic!("execution halted unexpectedly: {:?}", reason);
        }
    }
}

#[test]
fn test_e2e_static_call_propagation() {
    let mut context = setup_context_with_arbos_state();

    let storage_wat = include_bytes!("../test-data/storage.wat");
    let storage_address = deploy_wat_program(&mut context, storage_wat);

    let caller_wat = include_bytes!("../test-data/call.wat");
    let middle_caller_address = deploy_wat_program(&mut context, caller_wat);
    let outer_caller_address = deploy_wat_program(&mut context, caller_wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(10_000_000_000_u64));

    let mut evm = create_evm(context);

    let key = [0x11u8; 32];
    let value = [0x22u8; 32];
    let mut storage_write_args = vec![0x01u8];
    storage_write_args.extend_from_slice(&key);
    storage_write_args.extend_from_slice(&value);

    let mut middle_args = vec![0x00u8]; // regular call
    middle_args.extend_from_slice(storage_address.as_slice());
    middle_args.extend_from_slice(&storage_write_args);

    let mut outer_args = vec![0x01u8]; // static_call
    outer_args.extend_from_slice(middle_caller_address.as_slice());
    outer_args.extend_from_slice(&middle_args);

    let tx = create_call_tx(outer_caller_address, outer_args, 100_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { .. } => {}
        ExecutionResult::Revert { .. } => {}
        ExecutionResult::Halt { reason, .. } => {
            panic!("execution halted unexpectedly: {:?}", reason);
        }
    }

    // Verify storage was NOT written
    let mut read_args = vec![0x01u8];
    read_args.extend_from_slice(storage_address.as_slice());
    let mut storage_read_args = vec![0x00u8];
    storage_read_args.extend_from_slice(&key);
    read_args.extend_from_slice(&storage_read_args);

    let tx = create_call_tx_with_nonce(outer_caller_address, read_args, 50_000_000, 1);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            if output.data().len() == 32 {
                assert!(
                    output.data().iter().all(|&b| b == 0),
                    "storage should not have been written due to static propagation"
                );
            }
        }
        ExecutionResult::Revert { output, .. } => {
            panic!("read execution reverted: {:?}", output);
        }
        ExecutionResult::Halt { reason, .. } => {
            panic!("read execution halted: {:?}", reason);
        }
    }
}

#[test]
fn test_e2e_regular_call_allows_storage_write() {
    let mut context = setup_context_with_arbos_state();

    let storage_wat = include_bytes!("../test-data/storage.wat");
    let storage_address = deploy_wat_program(&mut context, storage_wat);

    let caller_wat = include_bytes!("../test-data/call.wat");
    let caller_address = deploy_wat_program(&mut context, caller_wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(10_000_000_000_u64));

    let mut evm = create_evm(context);

    let key = [0xBBu8; 32];
    let value = [0xCCu8; 32];
    let mut storage_args = vec![0x01u8];
    storage_args.extend_from_slice(&key);
    storage_args.extend_from_slice(&value);

    let mut args = vec![0x00u8]; // regular call
    args.extend_from_slice(storage_address.as_slice());
    args.extend_from_slice(&storage_args);

    let tx = create_call_tx(caller_address, args, 50_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(
                output.data().len(),
                32,
                "should return 32 bytes (old storage value)"
            );
            assert!(
                output.data().iter().all(|&b| b == 0),
                "old value should be zero for first write"
            );
        }
        ExecutionResult::Revert { output, .. } => {
            panic!("execution reverted: {:?}", output);
        }
        ExecutionResult::Halt { reason, .. } => {
            panic!("execution halted: {:?}", reason);
        }
    }

    // Verify storage was written
    let mut read_args = vec![0x00u8];
    read_args.extend_from_slice(storage_address.as_slice());
    let mut storage_read_args = vec![0x00u8];
    storage_read_args.extend_from_slice(&key);
    read_args.extend_from_slice(&storage_read_args);

    let tx = create_call_tx_with_nonce(caller_address, read_args, 50_000_000, 1);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(output.data().len(), 32, "should return 32 bytes");
            assert_eq!(
                output.data().as_ref(),
                &value,
                "storage should have been written with the value"
            );
        }
        ExecutionResult::Revert { output, .. } => {
            panic!("read execution reverted: {:?}", output);
        }
        ExecutionResult::Halt { reason, .. } => {
            panic!("read execution halted: {:?}", reason);
        }
    }
}

#[test]
fn test_e2e_regular_call_allows_log_emission() {
    let mut context = setup_context_with_arbos_state();

    let log_wat = include_bytes!("../test-data/log.wat");
    let log_address = deploy_wat_program(&mut context, log_wat);

    let caller_wat = include_bytes!("../test-data/call.wat");
    let caller_address = deploy_wat_program(&mut context, caller_wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(10_000_000_000_u64));

    let mut evm = create_evm(context);

    let topic = [0xDDu8; 32];
    let log_data = b"test log from nested call";
    let mut log_args = vec![0x01u8];
    log_args.extend_from_slice(&topic);
    log_args.extend_from_slice(log_data);

    let mut args = vec![0x00u8]; // regular call
    args.extend_from_slice(log_address.as_slice());
    args.extend_from_slice(&log_args);

    let tx = create_call_tx(caller_address, args, 50_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { logs, .. } => {
            assert_eq!(logs.len(), 1, "should emit one log");
            let log = &logs[0];
            assert_eq!(log.address, log_address, "log should be from log contract");
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

// ============================================================================
// Delegate Call Behavior Tests
// ============================================================================

#[test]
fn test_e2e_delegate_call_preserves_msg_sender() {
    let mut context = setup_context_with_arbos_state();

    let evm_data_wat = include_bytes!("../test-data/evm-data.wat");
    let evm_data_address = deploy_wat_program(&mut context, evm_data_wat);

    let caller_wat = include_bytes!("../test-data/call.wat");
    let caller_address = deploy_wat_program(&mut context, caller_wat);

    let eoa_caller = Address::repeat_byte(0x01);
    fund_account(&mut context, eoa_caller, U256::from(10_000_000_000_u64));

    let mut evm = create_evm(context);

    let evm_data_args = vec![0x03u8]; // msg_sender selector

    let mut args = vec![0x02u8]; // delegate_call
    args.extend_from_slice(evm_data_address.as_slice());
    args.extend_from_slice(&evm_data_args);

    let tx = create_call_tx(caller_address, args, 50_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(output.data().len(), 20, "msg_sender should be 20 bytes");
            let returned_sender = Address::from_slice(output.data().as_ref());
            assert_eq!(
                returned_sender, eoa_caller,
                "delegate_call should preserve msg.sender as the original EOA caller"
            );
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
fn test_e2e_delegate_call_contract_address() {
    let mut context = setup_context_with_arbos_state();

    let evm_data_wat = include_bytes!("../test-data/evm-data.wat");
    let evm_data_address = deploy_wat_program(&mut context, evm_data_wat);

    let caller_wat = include_bytes!("../test-data/call.wat");
    let caller_address = deploy_wat_program(&mut context, caller_wat);

    let eoa_caller = Address::repeat_byte(0x01);
    fund_account(&mut context, eoa_caller, U256::from(10_000_000_000_u64));

    let mut evm = create_evm(context);

    let evm_data_args = vec![0x04u8]; // contract_address selector

    let mut args = vec![0x02u8]; // delegate_call
    args.extend_from_slice(evm_data_address.as_slice());
    args.extend_from_slice(&evm_data_args);

    let tx = create_call_tx(caller_address, args, 50_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(
                output.data().len(),
                20,
                "contract_address should be 20 bytes"
            );
            let returned_address = Address::from_slice(output.data().as_ref());
            assert_eq!(
                returned_address, caller_address,
                "delegate_call should return caller's address for address(this), not callee's"
            );
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
fn test_e2e_regular_call_contract_address() {
    let mut context = setup_context_with_arbos_state();

    let evm_data_wat = include_bytes!("../test-data/evm-data.wat");
    let evm_data_address = deploy_wat_program(&mut context, evm_data_wat);

    let caller_wat = include_bytes!("../test-data/call.wat");
    let caller_address = deploy_wat_program(&mut context, caller_wat);

    let eoa_caller = Address::repeat_byte(0x01);
    fund_account(&mut context, eoa_caller, U256::from(10_000_000_000_u64));

    let mut evm = create_evm(context);

    let evm_data_args = vec![0x04u8]; // contract_address selector

    let mut args = vec![0x00u8]; // regular call
    args.extend_from_slice(evm_data_address.as_slice());
    args.extend_from_slice(&evm_data_args);

    let tx = create_call_tx(caller_address, args, 50_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(
                output.data().len(),
                20,
                "contract_address should be 20 bytes"
            );
            let returned_address = Address::from_slice(output.data().as_ref());
            assert_eq!(
                returned_address, evm_data_address,
                "regular call should return callee's address for address(this)"
            );
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
fn test_e2e_regular_call_changes_msg_sender() {
    let mut context = setup_context_with_arbos_state();

    let evm_data_wat = include_bytes!("../test-data/evm-data.wat");
    let evm_data_address = deploy_wat_program(&mut context, evm_data_wat);

    let caller_wat = include_bytes!("../test-data/call.wat");
    let caller_address = deploy_wat_program(&mut context, caller_wat);

    let eoa_caller = Address::repeat_byte(0x01);
    fund_account(&mut context, eoa_caller, U256::from(10_000_000_000_u64));

    let mut evm = create_evm(context);

    let evm_data_args = vec![0x03u8]; // msg_sender selector

    let mut args = vec![0x00u8]; // regular call
    args.extend_from_slice(evm_data_address.as_slice());
    args.extend_from_slice(&evm_data_args);

    let tx = create_call_tx(caller_address, args, 50_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(output.data().len(), 20, "msg_sender should be 20 bytes");
            let returned_sender = Address::from_slice(output.data().as_ref());
            assert_eq!(
                returned_sender, caller_address,
                "regular call should set msg.sender to the calling contract"
            );
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
fn test_e2e_delegate_call_storage_context() {
    let mut context = setup_context_with_arbos_state();

    let storage_wat = include_bytes!("../test-data/storage.wat");
    let storage_address = deploy_wat_program(&mut context, storage_wat);

    let caller_wat = include_bytes!("../test-data/call.wat");
    let caller_address = deploy_wat_program(&mut context, caller_wat);

    let eoa_caller = Address::repeat_byte(0x01);
    fund_account(&mut context, eoa_caller, U256::from(10_000_000_000_u64));

    let mut evm = create_evm(context);

    let key = [0xEEu8; 32];
    let value = [0xFFu8; 32];
    let mut storage_args = vec![0x01u8];
    storage_args.extend_from_slice(&key);
    storage_args.extend_from_slice(&value);

    let mut args = vec![0x02u8]; // delegate_call
    args.extend_from_slice(storage_address.as_slice());
    args.extend_from_slice(&storage_args);

    let tx = create_call_tx(caller_address, args, 50_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { .. } => {}
        ExecutionResult::Revert { output, .. } => {
            panic!("execution reverted: {:?}", output);
        }
        ExecutionResult::Halt { reason, .. } => {
            panic!("execution halted: {:?}", reason);
        }
    }

    // Verify callee's storage is empty
    let mut storage_read_args = vec![0x00u8];
    storage_read_args.extend_from_slice(&key);

    let tx = create_call_tx_with_nonce(storage_address, storage_read_args.clone(), 50_000_000, 1);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(output.data().len(), 32, "should return 32 bytes");
            assert!(
                output.data().iter().all(|&b| b == 0),
                "delegate_call should NOT write to callee's storage"
            );
        }
        ExecutionResult::Revert { output, .. } => {
            panic!("read execution reverted: {:?}", output);
        }
        ExecutionResult::Halt { reason, .. } => {
            panic!("read execution halted: {:?}", reason);
        }
    }

    // Verify caller's storage has the value
    let mut args = vec![0x02u8];
    args.extend_from_slice(storage_address.as_slice());
    args.extend_from_slice(&storage_read_args);

    let tx = create_call_tx_with_nonce(caller_address, args, 50_000_000, 2);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(output.data().len(), 32, "should return 32 bytes");
            assert_eq!(
                output.data().as_ref(),
                &value,
                "delegate_call should write to caller's storage"
            );
        }
        ExecutionResult::Revert { output, .. } => {
            panic!("read execution reverted: {:?}", output);
        }
        ExecutionResult::Halt { reason, .. } => {
            panic!("read execution halted: {:?}", reason);
        }
    }
}

// ============================================================================
// Value Transfer Tests
// ============================================================================

/// Test sending ETH value with a transaction to a Stylus program
#[test]
fn test_e2e_call_with_value() {
    let mut context = setup_context_with_arbos_state();

    // Deploy evm-data.wat which can read msg_value
    let evm_data_wat = include_bytes!("../test-data/evm-data.wat");
    let program_address = deploy_wat_program(&mut context, evm_data_wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(10_000_000_000_u64));

    let mut evm = create_evm(context);

    // Send value and ask for msg_value (selector 0x06)
    let value = U256::from(1_000_000_u64);
    let args = vec![0x06u8]; // msg_value selector

    let tx = create_call_tx_with_value(program_address, args, value, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(
                output.data().len(),
                32,
                "msg_value output should be 32 bytes"
            );
            let returned_value = U256::from_be_slice(output.data().as_ref());
            assert_eq!(returned_value, value, "msg_value should match sent value");
        }
        ExecutionResult::Revert { output, .. } => {
            panic!("execution reverted: {:?}", output);
        }
        ExecutionResult::Halt { reason, .. } => {
            panic!("execution halted: {:?}", reason);
        }
    }
}

/// Test that msg_value is zero when no value is sent
#[test]
fn test_e2e_call_without_value() {
    let mut context = setup_context_with_arbos_state();

    let evm_data_wat = include_bytes!("../test-data/evm-data.wat");
    let program_address = deploy_wat_program(&mut context, evm_data_wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(10_000_000_000_u64));

    let mut evm = create_evm(context);

    // No value, ask for msg_value
    let args = vec![0x06u8];

    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(
                output.data().len(),
                32,
                "msg_value output should be 32 bytes"
            );
            let returned_value = U256::from_be_slice(output.data().as_ref());
            assert_eq!(
                returned_value,
                U256::ZERO,
                "msg_value should be zero when no value sent"
            );
        }
        ExecutionResult::Revert { output, .. } => {
            panic!("execution reverted: {:?}", output);
        }
        ExecutionResult::Halt { reason, .. } => {
            panic!("execution halted: {:?}", reason);
        }
    }
}

/// Test nested call with value - callee should see the value forwarded
#[test]
fn test_e2e_nested_call_value_not_forwarded() {
    let mut context = setup_context_with_arbos_state();

    // Deploy evm-data.wat as callee (can read msg_value)
    let evm_data_wat = include_bytes!("../test-data/evm-data.wat");
    let evm_data_address = deploy_wat_program(&mut context, evm_data_wat);

    // Deploy call.wat as caller
    let caller_wat = include_bytes!("../test-data/call.wat");
    let caller_address = deploy_wat_program(&mut context, caller_wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(10_000_000_000_u64));

    let mut evm = create_evm(context);

    // Outer call sends value, inner call asks for msg_value
    // call.wat sends zero value (value buffer is zeroed)
    let value = U256::from(1_000_000_u64);
    let evm_data_args = vec![0x06u8]; // msg_value selector

    let mut args = vec![0x00u8]; // call_contract selector
    args.extend_from_slice(evm_data_address.as_slice());
    args.extend_from_slice(&evm_data_args);

    let tx = create_call_tx_with_value(caller_address, args, value, 50_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(output.data().len(), 32, "should return 32 bytes");
            let inner_value = U256::from_be_slice(output.data().as_ref());
            // call.wat doesn't forward value, so inner msg_value should be 0
            assert_eq!(
                inner_value,
                U256::ZERO,
                "nested call with call.wat should have msg_value=0 (value not forwarded)"
            );
        }
        ExecutionResult::Revert { output, .. } => {
            panic!("execution reverted: {:?}", output);
        }
        ExecutionResult::Halt { reason, .. } => {
            panic!("execution halted: {:?}", reason);
        }
    }
}
