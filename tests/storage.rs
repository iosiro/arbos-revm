// Copyright 2024, Offchain Labs, Inc.
// For license information, see https://github.com/OffchainLabs/nitro/blob/master/LICENSE.md

//! Storage and transient storage tests for Stylus programs.

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
// Storage Tests
// ============================================================================

#[test]
fn test_e2e_storage_read_uninitialized() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/storage.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);

    let key = [0xABu8; 32];
    let mut args = vec![0x00u8];
    args.extend_from_slice(&key);

    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(output.data().len(), 32, "output should be 32 bytes");
            assert!(
                output.data().iter().all(|&b| b == 0),
                "uninitialized slot should be zero"
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
fn test_e2e_storage_write_read() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/storage.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(10_000_000_000_u64));

    let mut evm = create_evm(context);

    let key = [0xABu8; 32];
    let value = [0xCDu8; 32];
    let mut write_args = vec![0x01u8];
    write_args.extend_from_slice(&key);
    write_args.extend_from_slice(&value);

    let tx = create_call_tx_with_nonce(program_address, write_args, 10_000_000, 0);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(output.data().len(), 32, "output should be 32 bytes");
            assert!(
                output.data().iter().all(|&b| b == 0),
                "first write should return zero (old value)"
            );
        }
        ExecutionResult::Revert { output, .. } => {
            panic!("write execution reverted: {:?}", output);
        }
        ExecutionResult::Halt { reason, .. } => {
            panic!("write execution halted: {:?}", reason);
        }
    }

    let mut read_args = vec![0x00u8];
    read_args.extend_from_slice(&key);

    let tx = create_call_tx_with_nonce(program_address, read_args, 10_000_000, 1);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(output.data().len(), 32, "output should be 32 bytes");
            assert_eq!(
                output.data().as_ref(),
                &value,
                "read should return written value"
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
fn test_e2e_storage_overwrite() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/storage.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(10_000_000_000_u64));

    let mut evm = create_evm(context);

    let key = [0x11u8; 32];
    let value1 = [0x22u8; 32];
    let value2 = [0x33u8; 32];

    let mut write_args1 = vec![0x01u8];
    write_args1.extend_from_slice(&key);
    write_args1.extend_from_slice(&value1);

    let tx = create_call_tx_with_nonce(program_address, write_args1, 10_000_000, 0);
    let _ = execute_tx(&mut evm, tx);

    let mut write_args2 = vec![0x01u8];
    write_args2.extend_from_slice(&key);
    write_args2.extend_from_slice(&value2);

    let tx = create_call_tx_with_nonce(program_address, write_args2, 10_000_000, 1);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(
                output.data().as_ref(),
                &value1,
                "overwrite should return old value"
            );
        }
        ExecutionResult::Revert { output, .. } => {
            panic!("overwrite execution reverted: {:?}", output);
        }
        ExecutionResult::Halt { reason, .. } => {
            panic!("overwrite execution halted: {:?}", reason);
        }
    }

    let mut read_args = vec![0x00u8];
    read_args.extend_from_slice(&key);

    let tx = create_call_tx_with_nonce(program_address, read_args, 10_000_000, 2);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(
                output.data().as_ref(),
                &value2,
                "should read the latest value"
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
// Transient Storage Tests (EIP-1153)
// ============================================================================

#[test]
fn test_e2e_transient_storage_read_uninitialized() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/transient-storage.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);

    let key = [0xABu8; 32];
    let mut args = vec![0x00u8];
    args.extend_from_slice(&key);

    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(output.data().len(), 32, "output should be 32 bytes");
            assert!(
                output.data().iter().all(|&b| b == 0),
                "uninitialized transient slot should be zero"
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
fn test_e2e_transient_storage_write_read() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/transient-storage.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(10_000_000_000_u64));

    let mut evm = create_evm(context);

    let key = [0xABu8; 32];
    let value = [0xCDu8; 32];
    let mut args = vec![0x02u8]; // write-then-read operation
    args.extend_from_slice(&key);
    args.extend_from_slice(&value);

    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(output.data().len(), 32, "output should be 32 bytes");
            assert_eq!(
                output.data().as_ref(),
                &value,
                "read should return written value"
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
fn test_e2e_transient_storage_cleared_between_txs() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/transient-storage.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(10_000_000_000_u64));

    let mut evm = create_evm(context);

    let key = [0xABu8; 32];
    let value = [0xCDu8; 32];
    let mut write_args = vec![0x01u8];
    write_args.extend_from_slice(&key);
    write_args.extend_from_slice(&value);

    let tx = create_call_tx_with_nonce(program_address, write_args, 10_000_000, 0);
    let result = execute_tx(&mut evm, tx);
    assert!(
        matches!(result, ExecutionResult::Success { .. }),
        "write should succeed"
    );

    let mut read_args = vec![0x00u8];
    read_args.extend_from_slice(&key);

    let tx = create_call_tx_with_nonce(program_address, read_args, 10_000_000, 1);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(output.data().len(), 32, "output should be 32 bytes");
            assert!(
                output.data().iter().all(|&b| b == 0),
                "transient storage should be zero after tx boundary (EIP-1153)"
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
