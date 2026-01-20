// Copyright 2024, Offchain Labs, Inc.
// For license information, see https://github.com/OffchainLabs/nitro/blob/master/LICENSE.md

//! Account info tests for Stylus programs (balance, code hash).

use revm::{
    context::result::ExecutionResult,
    primitives::{Address, B256, U256, keccak256},
};

mod test_utils;
use test_utils::{
    create_call_tx, create_evm, deploy_wat_program, execute_tx, fund_account,
    setup_context_with_arbos_state,
};

// ============================================================================
// Account Balance Tests
// ============================================================================

#[test]
fn test_e2e_account_balance_zero() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/account-info.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);

    // Query balance of an unfunded address
    let query_address = Address::repeat_byte(0xAB);
    let mut args = vec![0x00u8]; // balance selector
    args.extend_from_slice(query_address.as_slice());

    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(
                output.data().len(),
                32,
                "balance output should be 32 bytes (U256)"
            );
            let balance = U256::from_be_slice(output.data().as_ref());
            assert_eq!(
                balance,
                U256::ZERO,
                "unfunded account should have zero balance"
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
fn test_e2e_account_balance_funded() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/account-info.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    // Fund a specific address with a known balance
    let funded_address = Address::repeat_byte(0xAB);
    let funded_balance = U256::from(123_456_789_u64);
    fund_account(&mut context, funded_address, funded_balance);

    let mut evm = create_evm(context);

    // Query balance of the funded address
    let mut args = vec![0x00u8]; // balance selector
    args.extend_from_slice(funded_address.as_slice());

    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(
                output.data().len(),
                32,
                "balance output should be 32 bytes (U256)"
            );
            let balance = U256::from_be_slice(output.data().as_ref());
            assert_eq!(
                balance, funded_balance,
                "balance should match funded amount"
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
fn test_e2e_account_balance_self() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/account-info.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    // Fund the program address itself
    let program_balance = U256::from(999_999_u64);
    fund_account(&mut context, program_address, program_balance);

    let mut evm = create_evm(context);

    // Query balance of the program's own address
    let mut args = vec![0x00u8]; // balance selector
    args.extend_from_slice(program_address.as_slice());

    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(
                output.data().len(),
                32,
                "balance output should be 32 bytes (U256)"
            );
            let balance = U256::from_be_slice(output.data().as_ref());
            assert_eq!(
                balance, program_balance,
                "self balance should match funded amount"
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
// Account Code Hash Tests
// ============================================================================

#[test]
fn test_e2e_account_codehash_eoa() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/account-info.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    // Fund an EOA (no code)
    let eoa = Address::repeat_byte(0xAB);
    fund_account(&mut context, eoa, U256::from(1_000_000_u64));

    let mut evm = create_evm(context);

    // Query code hash of EOA
    let mut args = vec![0x01u8]; // codehash selector
    args.extend_from_slice(eoa.as_slice());

    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(
                output.data().len(),
                32,
                "codehash output should be 32 bytes"
            );
            // EOA should have empty code hash (keccak256 of empty = c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470)
            let expected_empty_hash: B256 = keccak256([]);
            let code_hash = B256::from_slice(output.data().as_ref());
            // Note: Empty accounts may return zero hash, funded EOAs return empty code hash
            assert!(
                code_hash == expected_empty_hash || code_hash == B256::ZERO,
                "EOA code hash should be empty hash or zero, got {:?}",
                code_hash
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
fn test_e2e_account_codehash_contract() {
    let mut context = setup_context_with_arbos_state();

    // Deploy a contract to query
    let storage_wat = include_bytes!("../test-data/storage.wat");
    let storage_address = deploy_wat_program(&mut context, storage_wat);

    // Deploy the account-info program
    let wat = include_bytes!("../test-data/account-info.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);

    // Query code hash of deployed contract
    let mut args = vec![0x01u8]; // codehash selector
    args.extend_from_slice(storage_address.as_slice());

    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(
                output.data().len(),
                32,
                "codehash output should be 32 bytes"
            );
            let code_hash = B256::from_slice(output.data().as_ref());
            // Contract should have a non-zero, non-empty code hash
            let empty_hash: B256 = keccak256([]);
            assert_ne!(
                code_hash,
                B256::ZERO,
                "contract code hash should not be zero"
            );
            assert_ne!(
                code_hash, empty_hash,
                "contract code hash should not be empty hash"
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
fn test_e2e_account_codehash_nonexistent() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/account-info.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);

    // Query code hash of non-existent address
    let nonexistent = Address::repeat_byte(0xDE);
    let mut args = vec![0x01u8]; // codehash selector
    args.extend_from_slice(nonexistent.as_slice());

    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(
                output.data().len(),
                32,
                "codehash output should be 32 bytes"
            );
            let code_hash = B256::from_slice(output.data().as_ref());
            // Non-existent account should return zero hash per EIP-1052
            assert_eq!(
                code_hash,
                B256::ZERO,
                "non-existent account code hash should be zero"
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
// Cross-Contract Balance Query Tests
// ============================================================================

#[test]
fn test_e2e_balance_via_nested_call() {
    let mut context = setup_context_with_arbos_state();

    // Deploy account-info as target
    let account_info_wat = include_bytes!("../test-data/account-info.wat");
    let account_info_address = deploy_wat_program(&mut context, account_info_wat);

    // Deploy call.wat as caller
    let call_wat = include_bytes!("../test-data/call.wat");
    let caller_address = deploy_wat_program(&mut context, call_wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(10_000_000_000_u64));

    // Fund a specific address
    let query_address = Address::repeat_byte(0xAB);
    let query_balance = U256::from(555_555_555_u64);
    fund_account(&mut context, query_address, query_balance);

    let mut evm = create_evm(context);

    // Build args to call account_info with balance query
    let mut inner_args = vec![0x00u8]; // balance selector
    inner_args.extend_from_slice(query_address.as_slice());

    let mut args = vec![0x00u8]; // call_contract selector
    args.extend_from_slice(account_info_address.as_slice());
    args.extend_from_slice(&inner_args);

    let tx = create_call_tx(caller_address, args, 50_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(output.data().len(), 32, "balance output should be 32 bytes");
            let balance = U256::from_be_slice(output.data().as_ref());
            assert_eq!(
                balance, query_balance,
                "nested balance query should return correct value"
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
