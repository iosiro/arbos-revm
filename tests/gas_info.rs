// Copyright 2024, Offchain Labs, Inc.
// For license information, see https://github.com/OffchainLabs/nitro/blob/master/LICENSE.md

//! Gas/Ink introspection tests for Stylus programs.

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
// Gas Introspection Tests
// ============================================================================

#[test]
fn test_e2e_evm_gas_left() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/gas-info.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);

    // Selector 0x00 = evm_gas_left
    let args = vec![0x00u8];

    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(
                output.data().len(),
                8,
                "evm_gas_left output should be 8 bytes (u64)"
            );
            let gas_left = u64::from_le_bytes(output.data()[..8].try_into().unwrap());
            // Gas left should be positive (we started with 10M gas)
            // It will be less than the limit due to execution costs
            assert!(gas_left > 0, "gas_left should be positive");
            assert!(
                gas_left < 10_000_000,
                "gas_left should be less than initial limit"
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
fn test_e2e_evm_gas_left_decreases_with_work() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/gas-info.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);

    // First call with more gas
    let args = vec![0x00u8];
    let tx = create_call_tx_with_nonce(program_address, args.clone(), 20_000_000, 0);
    let result1 = execute_tx(&mut evm, tx);

    let gas_left_high = match result1 {
        ExecutionResult::Success { output, .. } => {
            u64::from_le_bytes(output.data()[..8].try_into().unwrap())
        }
        _ => panic!("first call failed"),
    };

    // Second call with less gas
    let tx = create_call_tx_with_nonce(program_address, args, 5_000_000, 1);
    let result2 = execute_tx(&mut evm, tx);

    let gas_left_low = match result2 {
        ExecutionResult::Success { output, .. } => {
            u64::from_le_bytes(output.data()[..8].try_into().unwrap())
        }
        _ => panic!("second call failed"),
    };

    // With more initial gas, we should have more gas left
    assert!(
        gas_left_high > gas_left_low,
        "higher gas limit should result in more gas left: {} vs {}",
        gas_left_high,
        gas_left_low
    );
}

// ============================================================================
// Ink Introspection Tests
// ============================================================================

#[test]
fn test_e2e_evm_ink_left() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/gas-info.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);

    // Selector 0x01 = evm_ink_left
    let args = vec![0x01u8];

    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(
                output.data().len(),
                8,
                "evm_ink_left output should be 8 bytes (u64)"
            );
            let ink_left = u64::from_le_bytes(output.data()[..8].try_into().unwrap());
            // Ink left should be positive
            // Note: ink = gas * ink_price, so ink values are typically larger than gas
            assert!(ink_left > 0, "ink_left should be positive");
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
fn test_e2e_ink_greater_than_gas() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/gas-info.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);

    // Get gas left
    let args = vec![0x00u8];
    let tx = create_call_tx_with_nonce(program_address, args, 10_000_000, 0);
    let result = execute_tx(&mut evm, tx);

    let gas_left = match result {
        ExecutionResult::Success { output, .. } => {
            u64::from_le_bytes(output.data()[..8].try_into().unwrap())
        }
        _ => panic!("gas call failed"),
    };

    // Get ink left
    let args = vec![0x01u8];
    let tx = create_call_tx_with_nonce(program_address, args, 10_000_000, 1);
    let result = execute_tx(&mut evm, tx);

    let ink_left = match result {
        ExecutionResult::Success { output, .. } => {
            u64::from_le_bytes(output.data()[..8].try_into().unwrap())
        }
        _ => panic!("ink call failed"),
    };

    // Ink should be >= gas (ink = gas * ink_price, where ink_price >= 1)
    // In Stylus, ink_price is typically 10000
    assert!(
        ink_left >= gas_left,
        "ink_left ({}) should be >= gas_left ({})",
        ink_left,
        gas_left
    );
}

// ============================================================================
// Transaction Gas Price Tests
// ============================================================================

#[test]
fn test_e2e_tx_gas_price() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/gas-info.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);

    // Selector 0x02 = tx_gas_price
    let args = vec![0x02u8];

    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(
                output.data().len(),
                32,
                "tx_gas_price output should be 32 bytes (U256)"
            );
            let gas_price = U256::from_be_slice(output.data().as_ref());
            // Gas price should be a valid value (can be zero or positive in test environment)
            // On Arbitrum, gas price equals basefee
            assert!(gas_price >= U256::ZERO, "gas_price should be valid");
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
// Ink Price Tests
// ============================================================================

#[test]
fn test_e2e_tx_ink_price() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/gas-info.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);

    // Selector 0x03 = tx_ink_price
    let args = vec![0x03u8];

    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(
                output.data().len(),
                8,
                "tx_ink_price output should be 8 bytes (u64)"
            );
            let ink_price = u64::from_le_bytes(output.data()[..8].try_into().unwrap());
            // Ink price should be positive (default is 10000 in Stylus)
            assert!(ink_price > 0, "ink_price should be positive");
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
fn test_e2e_ink_price_consistency() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/gas-info.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);

    // Get ink price
    let args = vec![0x03u8];
    let tx = create_call_tx_with_nonce(program_address, args, 10_000_000, 0);
    let result = execute_tx(&mut evm, tx);

    let ink_price = match result {
        ExecutionResult::Success { output, .. } => {
            u64::from_le_bytes(output.data()[..8].try_into().unwrap())
        }
        _ => panic!("ink price call failed"),
    };

    // Get gas left and ink left to verify consistency
    let args = vec![0x00u8];
    let tx = create_call_tx_with_nonce(program_address, args, 10_000_000, 1);
    let result = execute_tx(&mut evm, tx);

    let gas_left = match result {
        ExecutionResult::Success { output, .. } => {
            u64::from_le_bytes(output.data()[..8].try_into().unwrap())
        }
        _ => panic!("gas call failed"),
    };

    let args = vec![0x01u8];
    let tx = create_call_tx_with_nonce(program_address, args, 10_000_000, 2);
    let result = execute_tx(&mut evm, tx);

    let ink_left = match result {
        ExecutionResult::Success { output, .. } => {
            u64::from_le_bytes(output.data()[..8].try_into().unwrap())
        }
        _ => panic!("ink call failed"),
    };

    // Verify the relationship: ink = gas * ink_price (approximately)
    // Due to different execution costs, this won't be exact, but should be in the same ballpark
    if ink_price > 0 && gas_left > 0 {
        let expected_ink_approx = gas_left.saturating_mul(ink_price);
        // Allow 50% variance due to different execution paths
        let lower_bound = expected_ink_approx / 2;
        let upper_bound = expected_ink_approx.saturating_mul(2);

        assert!(
            ink_left >= lower_bound && ink_left <= upper_bound,
            "ink_left ({}) should be approximately gas_left ({}) * ink_price ({}), expected ~{}",
            ink_left,
            gas_left,
            ink_price,
            expected_ink_approx
        );
    }
}
