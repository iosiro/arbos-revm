// Copyright 2024, Offchain Labs, Inc.
// For license information, see https://github.com/OffchainLabs/nitro/blob/master/LICENSE.md

//! Basic execution tests for Stylus programs.

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
// Basic Execution Tests
// ============================================================================

#[test]
fn test_e2e_basic_execution() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/add.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);
    let tx = create_call_tx(program_address, vec![], 1_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success {
            output, gas_used, ..
        } => {
            assert!(gas_used > 0, "gas should have been consumed");
            assert!(output.data().is_empty() || output.data()[0] == 0);
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
fn test_e2e_write_args() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/write-args.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);
    let test_data = b"hello stylus world".to_vec();
    let tx = create_call_tx(program_address, test_data.clone(), 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(
                output.data().as_ref(),
                test_data.as_slice(),
                "output should match input"
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
fn test_e2e_write_args_empty() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/write-args.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);
    let tx = create_call_tx(program_address, vec![], 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert!(
                output.data().is_empty(),
                "empty input should produce empty output"
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
fn test_e2e_write_args_large() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/write-args.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);
    let large_data: Vec<u8> = (0..1024).map(|i| (i % 256) as u8).collect();
    let tx = create_call_tx(program_address, large_data.clone(), 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(output.data().len(), large_data.len());
            assert_eq!(output.data().as_ref(), large_data.as_slice());
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
// Memory Tests
// ============================================================================

#[test]
fn test_e2e_memory_growth() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/memory.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);
    let args = vec![64u8, 8u8];
    let tx = create_call_tx(program_address, args, 100_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert!(!output.data().is_empty(), "expected non-empty output");
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
// Exit Early Tests
// ============================================================================

#[test]
fn test_e2e_exit_early_revert() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/exit-early/exit-early.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);
    let args = vec![0x01u8; 32];
    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Revert { output, .. } => {
            assert!(!output.is_empty(), "revert should have output");
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
fn test_e2e_exit_early_success() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/exit-early/exit-early.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);
    let mut args = vec![0x00u8; 32];
    args[1..8].copy_from_slice(b"success");
    let tx = create_call_tx(program_address, args.clone(), 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(output.data().as_ref(), args.as_slice());
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
// Gas/Ink Tests
// ============================================================================

#[test]
fn test_e2e_out_of_gas() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/memory.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);
    let args = vec![128u8, 16u8];
    let tx = create_call_tx(program_address, args, 50_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Halt { .. } => {}
        ExecutionResult::Revert { .. } => {}
        ExecutionResult::Success { gas_used, .. } => {
            assert!(gas_used > 30_000, "if success, gas usage should be high");
        }
    }
}

#[test]
fn test_e2e_gas_tracking() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/add.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);
    let initial_gas = 10_000_000u64;
    let tx = create_call_tx(program_address, vec![], initial_gas);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { gas_used, .. } => {
            assert!(gas_used > 0, "gas should have been consumed");
            assert!(gas_used < initial_gas, "not all gas should be used");
        }
        ExecutionResult::Revert { gas_used, .. } => {
            assert!(gas_used > 0, "gas should have been consumed even on revert");
        }
        ExecutionResult::Halt { gas_used, .. } => {
            assert!(gas_used > 0, "gas should have been consumed even on halt");
        }
    }
}

/// Test execution with massive gas limit to verify no overflow in gas-to-ink conversion.
/// ink_price is typically 10000, so gas * ink_price could overflow u64 if gas is very large.
#[test]
fn test_e2e_massive_gas_limit() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/add.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    // Fund with a massive amount to cover any gas price
    fund_account(&mut context, caller, U256::MAX);

    let mut evm = create_evm(context);

    // Use a very large gas limit that could cause overflow when converted to ink
    // ink = gas * ink_price, where ink_price = 10000
    // u64::MAX / 10000 = 1844674407370955 is the max safe gas before overflow
    // Let's test with values near this boundary
    let massive_gas = u64::MAX / 10000 - 1000; // Just under the overflow threshold
    let tx = create_call_tx(program_address, vec![], massive_gas);
    let result = execute_tx(&mut evm, tx);

    // Should not panic due to overflow - either success or proper error handling
    match result {
        ExecutionResult::Success { gas_used, .. } => {
            assert!(gas_used > 0, "gas should have been consumed");
            assert!(
                gas_used < massive_gas,
                "not all gas should be used for simple program"
            );
        }
        ExecutionResult::Revert { .. } => {
            // Acceptable - might revert for other reasons
        }
        ExecutionResult::Halt { .. } => {
            // Acceptable - might halt for other reasons
        }
    }
}

/// Test execution with gas limit at the exact overflow boundary.
#[test]
fn test_e2e_gas_limit_at_ink_overflow_boundary() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/add.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::MAX);

    let mut evm = create_evm(context);

    // Test at exact boundary: u64::MAX / ink_price
    // This is the largest gas value that won't overflow when multiplied by ink_price
    let boundary_gas = u64::MAX / 10000;
    let tx = create_call_tx(program_address, vec![], boundary_gas);
    let result = execute_tx(&mut evm, tx);

    // Should handle gracefully without panic
    match result {
        ExecutionResult::Success { .. } => {}
        ExecutionResult::Revert { .. } => {}
        ExecutionResult::Halt { .. } => {}
    }
}

/// Test that Stylus program properly runs out of ink during heavy computation.
#[test]
fn test_e2e_stylus_out_of_ink_during_execution() {
    let mut context = setup_context_with_arbos_state();

    // memory.wat does memory growth which consumes significant ink
    let wat = include_bytes!("../test-data/memory.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);

    // Request massive memory growth (256 pages, stepping by 32) with minimal gas
    // This should exhaust ink during execution
    let args = vec![255u8, 32u8]; // target_pages=255, step_size=32
    let tx = create_call_tx(program_address, args, 100_000); // Limited gas
    let result = execute_tx(&mut evm, tx);

    // Should run out of gas/ink, not succeed
    match result {
        ExecutionResult::Halt {
            reason, gas_used, ..
        } => {
            // Expected - ran out of gas
            assert!(gas_used > 0, "should have used some gas before halting");
            // OutOfGas is the expected halt reason
            assert!(
                format!("{:?}", reason).contains("OutOfGas")
                    || format!("{:?}", reason).contains("Gas"),
                "should halt due to gas/ink exhaustion, got: {:?}",
                reason
            );
        }
        ExecutionResult::Revert { gas_used, .. } => {
            // Also acceptable - program might revert when it detects low gas
            assert!(gas_used > 0, "should have used some gas");
        }
        ExecutionResult::Success { gas_used, .. } => {
            // If it succeeds, it should have used nearly all the gas
            assert!(
                gas_used > 90_000,
                "if success with limited gas, should use most of it"
            );
        }
    }
}

/// Test minimal gas - just enough to start but not complete.
#[test]
fn test_e2e_minimal_gas_for_stylus() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/write-args.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);

    // 21000 is base tx cost, add just a little more
    // This might not be enough for Stylus execution overhead
    let minimal_gas = 25_000u64;
    let tx = create_call_tx(program_address, vec![1, 2, 3, 4], minimal_gas);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Halt { gas_used, .. } => {
            assert!(
                gas_used <= minimal_gas,
                "shouldn't use more gas than provided"
            );
        }
        ExecutionResult::Revert { gas_used, .. } => {
            assert!(
                gas_used <= minimal_gas,
                "shouldn't use more gas than provided"
            );
        }
        ExecutionResult::Success { gas_used, .. } => {
            assert!(
                gas_used <= minimal_gas,
                "shouldn't use more gas than provided"
            );
        }
    }
}

// ============================================================================
// Multiple Execution Tests
// ============================================================================

#[test]
fn test_e2e_multiple_executions() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/write-args.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(10_000_000_000_u64));

    let mut evm = create_evm(context);

    for i in 0..5u64 {
        let data = format!("test message {}", i);
        let tx =
            create_call_tx_with_nonce(program_address, data.as_bytes().to_vec(), 10_000_000, i);
        let result = execute_tx(&mut evm, tx);

        match result {
            ExecutionResult::Success { output, .. } => {
                assert_eq!(output.data().as_ref(), data.as_bytes());
            }
            ExecutionResult::Revert { output, .. } => {
                panic!("execution {} reverted: {:?}", i, output);
            }
            ExecutionResult::Halt { reason, .. } => {
                panic!("execution {} halted: {:?}", i, reason);
            }
        }
    }
}

#[test]
fn test_e2e_various_input_sizes() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/write-args.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(10_000_000_000_u64));

    let mut evm = create_evm(context);
    let sizes = [0usize, 1, 32, 64, 256, 1024];

    for (nonce, &size) in sizes.iter().enumerate() {
        let data: Vec<u8> = (0..size).map(|i| (i % 256) as u8).collect();
        let tx = create_call_tx_with_nonce(program_address, data.clone(), 10_000_000, nonce as u64);
        let result = execute_tx(&mut evm, tx);

        match result {
            ExecutionResult::Success { output, .. } => {
                assert_eq!(
                    output.data().len(),
                    size,
                    "output size mismatch for input size {}",
                    size
                );
                assert_eq!(
                    output.data().as_ref(),
                    data.as_slice(),
                    "output data mismatch for input size {}",
                    size
                );
            }
            ExecutionResult::Revert { output, .. } => {
                panic!("execution for size {} reverted: {:?}", size, output);
            }
            ExecutionResult::Halt { reason, .. } => {
                panic!("execution for size {} halted: {:?}", size, reason);
            }
        }
    }
}

// ============================================================================
// Miscellaneous Tests
// ============================================================================

#[test]
fn test_e2e_bulk_memory() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/bulk-memory-oob.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);
    let tx = create_call_tx(program_address, vec![], 10_000_000);
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
}

#[test]
fn test_e2e_depth() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/depth.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);
    let tx = create_call_tx(program_address, vec![], 10_000_000);
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
}

#[test]
fn test_e2e_return_size() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/return-size.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);
    let size: u32 = 256;
    let mut args = vec![0u8; 32];
    args[28..32].copy_from_slice(&size.to_be_bytes());
    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(output.data().len(), 256, "output should be 256 bytes");
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
fn test_e2e_write_result_len() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/write-result-len.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);
    let output_len: u32 = 10;
    let args = [output_len.to_le_bytes().as_slice(), &[0u8; 28]].concat();
    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(output.data().len(), 10, "output length should be 10");
        }
        ExecutionResult::Revert { output, .. } => {
            panic!("execution reverted: {:?}", output);
        }
        ExecutionResult::Halt { reason, .. } => {
            panic!("execution halted: {:?}", reason);
        }
    }
}
