// Copyright 2024, Offchain Labs, Inc.
// For license information, see https://github.com/OffchainLabs/nitro/blob/master/LICENSE.md

//! Revert behavior tests for Stylus programs.

use revm::{
    context::result::ExecutionResult,
    primitives::{Address, U256},
};

mod test_utils;
use test_utils::{
    create_call_tx, create_call_tx_with_nonce, create_evm, deploy_wat_program, execute_tx,
    fund_account, setup_context_with_arbos_state,
};

// ============================================================================
// Basic Revert Tests
// ============================================================================

#[test]
fn test_e2e_revert_with_data() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/revert.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);

    // Selector 0x01 = revert with input as revert data
    let revert_data = b"revert reason here";
    let mut args = vec![0x01u8];
    args.extend_from_slice(revert_data);

    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Revert { output, .. } => {
            assert_eq!(
                output.as_ref(),
                revert_data,
                "revert data should match input"
            );
        }
        ExecutionResult::Success { .. } => {
            panic!("expected revert, got success");
        }
        ExecutionResult::Halt { reason, .. } => {
            panic!("execution halted: {:?}", reason);
        }
    }
}

#[test]
fn test_e2e_revert_with_custom_error() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/revert.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);

    // Selector 0x02 = revert with "CustomError"
    let args = vec![0x02u8];

    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Revert { output, .. } => {
            assert_eq!(
                output.as_ref(),
                b"CustomError",
                "revert data should be 'CustomError'"
            );
        }
        ExecutionResult::Success { .. } => {
            panic!("expected revert, got success");
        }
        ExecutionResult::Halt { reason, .. } => {
            panic!("execution halted: {:?}", reason);
        }
    }
}

#[test]
fn test_e2e_revert_with_empty_data() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/revert.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);

    // Selector 0x03 = revert with empty data
    let args = vec![0x03u8];

    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Revert { output, .. } => {
            assert!(output.is_empty(), "revert data should be empty");
        }
        ExecutionResult::Success { .. } => {
            panic!("expected revert, got success");
        }
        ExecutionResult::Halt { reason, .. } => {
            panic!("execution halted: {:?}", reason);
        }
    }
}

#[test]
fn test_e2e_success_does_not_revert() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/revert.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);

    // Selector 0x00 = succeed with echo
    let echo_data = b"hello world";
    let mut args = vec![0x00u8];
    args.extend_from_slice(echo_data);

    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(
                output.data().as_ref(),
                echo_data,
                "output should echo input"
            );
        }
        ExecutionResult::Revert { output, .. } => {
            panic!("unexpected revert: {:?}", output);
        }
        ExecutionResult::Halt { reason, .. } => {
            panic!("execution halted: {:?}", reason);
        }
    }
}

// ============================================================================
// Nested Revert Propagation Tests
// ============================================================================

/// Test that revert data propagates through nested calls
#[test]
fn test_e2e_nested_call_revert_propagation() {
    let mut context = setup_context_with_arbos_state();

    // Deploy the revert contract as callee
    let revert_wat = include_bytes!("../test-data/revert.wat");
    let revert_address = deploy_wat_program(&mut context, revert_wat);

    // Deploy the call contract as caller
    let caller_wat = include_bytes!("../test-data/call.wat");
    let caller_address = deploy_wat_program(&mut context, caller_wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(10_000_000_000_u64));

    let mut evm = create_evm(context);

    // Build args to call revert contract with selector 0x01 (revert with data)
    let revert_data = b"nested revert";
    let mut revert_args = vec![0x01u8];
    revert_args.extend_from_slice(revert_data);

    let mut args = vec![0x00u8]; // call_contract selector
    args.extend_from_slice(revert_address.as_slice());
    args.extend_from_slice(&revert_args);

    let tx = create_call_tx(caller_address, args, 50_000_000);
    let result = execute_tx(&mut evm, tx);

    // The outer call should either:
    // 1. Return a failure status (if call.wat catches the revert)
    // 2. Propagate the revert
    match result {
        ExecutionResult::Success { output, .. } => {
            // call.wat returns a status byte when inner call fails
            assert!(!output.data().is_empty(), "should have output");
            // Non-zero status indicates failure
            assert_ne!(
                output.data()[0],
                0,
                "inner revert should cause non-zero status"
            );
        }
        ExecutionResult::Revert { .. } => {
            // Revert propagation is also acceptable
        }
        ExecutionResult::Halt { reason, .. } => {
            panic!("execution halted: {:?}", reason);
        }
    }
}

/// Test that revert in nested call doesn't corrupt caller state
#[test]
fn test_e2e_nested_revert_caller_state_preserved() {
    let mut context = setup_context_with_arbos_state();

    // Deploy storage contract to verify state
    let storage_wat = include_bytes!("../test-data/storage.wat");
    let storage_address = deploy_wat_program(&mut context, storage_wat);

    // Deploy revert contract as callee
    let revert_wat = include_bytes!("../test-data/revert.wat");
    let revert_address = deploy_wat_program(&mut context, revert_wat);

    // Deploy call contract as caller
    let caller_wat = include_bytes!("../test-data/call.wat");
    let caller_address = deploy_wat_program(&mut context, caller_wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(100_000_000_000_u64));

    let mut evm = create_evm(context);

    // First, write a value to storage
    let key = [0x42u8; 32];
    let value = [0xABu8; 32];
    let mut storage_write_args = vec![0x01u8];
    storage_write_args.extend_from_slice(&key);
    storage_write_args.extend_from_slice(&value);

    let mut args = vec![0x00u8]; // call_contract
    args.extend_from_slice(storage_address.as_slice());
    args.extend_from_slice(&storage_write_args);

    let tx = create_call_tx_with_nonce(caller_address, args, 10_000_000, 0);
    let result = execute_tx(&mut evm, tx);
    assert!(matches!(result, ExecutionResult::Success { .. }));

    // Now call the revert contract which will fail
    let mut revert_args = vec![0x01u8]; // revert
    revert_args.extend_from_slice(b"test revert");

    let mut args = vec![0x00u8];
    args.extend_from_slice(revert_address.as_slice());
    args.extend_from_slice(&revert_args);

    let tx = create_call_tx_with_nonce(caller_address, args, 10_000_000, 1);
    let _result = execute_tx(&mut evm, tx);
    // Don't care about result, just verifying state isn't corrupted

    // Verify storage is still intact
    let mut storage_read_args = vec![0x00u8];
    storage_read_args.extend_from_slice(&key);

    let mut args = vec![0x00u8];
    args.extend_from_slice(storage_address.as_slice());
    args.extend_from_slice(&storage_read_args);

    let tx = create_call_tx_with_nonce(caller_address, args, 10_000_000, 2);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(output.data().len(), 32);
            assert_eq!(
                output.data().as_ref(),
                &value,
                "storage should be preserved after nested revert"
            );
        }
        ExecutionResult::Revert { output, .. } => {
            panic!("read reverted: {:?}", output);
        }
        ExecutionResult::Halt { reason, .. } => {
            panic!("read halted: {:?}", reason);
        }
    }
}

// ============================================================================
// Edge Case Tests
// ============================================================================

/// Test calling a non-existent address (no code)
#[test]
fn test_e2e_call_to_nonexistent_address() {
    let mut context = setup_context_with_arbos_state();

    // Deploy call contract
    let caller_wat = include_bytes!("../test-data/call.wat");
    let caller_address = deploy_wat_program(&mut context, caller_wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(10_000_000_000_u64));

    let mut evm = create_evm(context);

    // Call to an address with no code (0xdead...dead)
    let nonexistent = Address::repeat_byte(0xDE);
    let calldata = b"hello";

    let mut args = vec![0x00u8]; // call_contract
    args.extend_from_slice(nonexistent.as_slice());
    args.extend_from_slice(calldata);

    let tx = create_call_tx(caller_address, args, 50_000_000);
    let result = execute_tx(&mut evm, tx);

    // Call to non-existent address should succeed with empty return data (EVM behavior)
    match result {
        ExecutionResult::Success { output, .. } => {
            // Should succeed - calling empty address succeeds in EVM
            // Return data should be empty (no code = no return)
            assert!(
                output.data().is_empty() || output.data()[0] == 0,
                "call to empty address should succeed with empty/zero return"
            );
        }
        ExecutionResult::Revert { .. } => {
            // Also acceptable depending on implementation
        }
        ExecutionResult::Halt { reason, .. } => {
            panic!("execution halted: {:?}", reason);
        }
    }
}

/// Test calling an EOA (externally owned account with balance but no code)
#[test]
fn test_e2e_call_to_eoa() {
    let mut context = setup_context_with_arbos_state();

    // Deploy call contract
    let caller_wat = include_bytes!("../test-data/call.wat");
    let caller_address = deploy_wat_program(&mut context, caller_wat);

    let eoa = Address::repeat_byte(0x02);
    let caller = Address::repeat_byte(0x01);

    // Fund both accounts
    fund_account(&mut context, caller, U256::from(10_000_000_000_u64));
    fund_account(&mut context, eoa, U256::from(1_000_000_u64)); // EOA has balance but no code

    let mut evm = create_evm(context);

    // Call to EOA
    let calldata = b"test";

    let mut args = vec![0x00u8]; // call_contract
    args.extend_from_slice(eoa.as_slice());
    args.extend_from_slice(calldata);

    let tx = create_call_tx(caller_address, args, 50_000_000);
    let result = execute_tx(&mut evm, tx);

    // Call to EOA should succeed with empty return (EVM behavior)
    match result {
        ExecutionResult::Success { output, .. } => {
            assert!(
                output.data().is_empty() || output.data()[0] == 0,
                "call to EOA should succeed with empty/zero return"
            );
        }
        ExecutionResult::Revert { .. } => {
            // Also acceptable
        }
        ExecutionResult::Halt { reason, .. } => {
            panic!("execution halted: {:?}", reason);
        }
    }
}

/// Test self-call (contract calling itself)
#[test]
fn test_e2e_self_call() {
    let mut context = setup_context_with_arbos_state();

    // Deploy the revert contract which can echo back data
    let revert_wat = include_bytes!("../test-data/revert.wat");
    let contract_address = deploy_wat_program(&mut context, revert_wat);

    // Deploy call contract
    let caller_wat = include_bytes!("../test-data/call.wat");
    let caller_address = deploy_wat_program(&mut context, caller_wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(10_000_000_000_u64));

    let mut evm = create_evm(context);

    // Have caller_address call itself (well, call contract calling contract)
    // This tests reentrancy-like behavior
    let inner_data = b"inner call";
    let mut inner_args = vec![0x00u8]; // echo selector for revert.wat
    inner_args.extend_from_slice(inner_data);

    let mut args = vec![0x00u8]; // call_contract
    args.extend_from_slice(contract_address.as_slice());
    args.extend_from_slice(&inner_args);

    let tx = create_call_tx(caller_address, args, 50_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            // Should echo the inner data back
            assert_eq!(
                output.data().as_ref(),
                inner_data,
                "should echo inner call data"
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
