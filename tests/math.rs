// Copyright 2024, Offchain Labs, Inc.
// For license information, see https://github.com/OffchainLabs/nitro/blob/master/LICENSE.md

//! Math operation tests for Stylus programs.

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
fn test_e2e_math_div() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/math.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);

    // math_div (selector 0x00): a / b
    // 100 / 7 = 14
    let mut args = vec![0x00u8]; // div selector
    let a = U256::from(100);
    let b = U256::from(7);
    args.extend_from_slice(&a.to_be_bytes::<32>());
    args.extend_from_slice(&b.to_be_bytes::<32>());

    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(output.data().len(), 32, "output should be 32 bytes");
            let result_value = U256::from_be_slice(output.data().as_ref());
            assert_eq!(result_value, U256::from(14), "100 / 7 should be 14");
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
fn test_e2e_math_mod() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/math.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);

    // math_mod (selector 0x01): a % b
    // 100 % 7 = 2
    let mut args = vec![0x01u8]; // mod selector
    let a = U256::from(100);
    let b = U256::from(7);
    args.extend_from_slice(&a.to_be_bytes::<32>());
    args.extend_from_slice(&b.to_be_bytes::<32>());

    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(output.data().len(), 32, "output should be 32 bytes");
            let result_value = U256::from_be_slice(output.data().as_ref());
            assert_eq!(result_value, U256::from(2), "100 % 7 should be 2");
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
fn test_e2e_math_pow() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/math.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);

    // math_pow (selector 0x02): a ** b
    // 2 ** 10 = 1024
    let mut args = vec![0x02u8]; // pow selector
    let a = U256::from(2);
    let b = U256::from(10);
    args.extend_from_slice(&a.to_be_bytes::<32>());
    args.extend_from_slice(&b.to_be_bytes::<32>());

    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(output.data().len(), 32, "output should be 32 bytes");
            let result_value = U256::from_be_slice(output.data().as_ref());
            assert_eq!(result_value, U256::from(1024), "2 ** 10 should be 1024");
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
fn test_e2e_math_add_mod() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/math.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);

    // math_add_mod (selector 0x03): (a + b) % c
    // (10 + 5) % 7 = 1
    let mut args = vec![0x03u8]; // add_mod selector
    let a = U256::from(10);
    let b = U256::from(5);
    let c = U256::from(7);
    args.extend_from_slice(&a.to_be_bytes::<32>());
    args.extend_from_slice(&b.to_be_bytes::<32>());
    args.extend_from_slice(&c.to_be_bytes::<32>());

    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(output.data().len(), 32, "output should be 32 bytes");
            let result_value = U256::from_be_slice(output.data().as_ref());
            assert_eq!(result_value, U256::from(1), "(10 + 5) % 7 should be 1");
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
fn test_e2e_math_mul_mod() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/math.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);

    // math_mul_mod (selector 0x04): (a * b) % c
    // (10 * 5) % 7 = 50 % 7 = 1
    let mut args = vec![0x04u8]; // mul_mod selector
    let a = U256::from(10);
    let b = U256::from(5);
    let c = U256::from(7);
    args.extend_from_slice(&a.to_be_bytes::<32>());
    args.extend_from_slice(&b.to_be_bytes::<32>());
    args.extend_from_slice(&c.to_be_bytes::<32>());

    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(output.data().len(), 32, "output should be 32 bytes");
            let result_value = U256::from_be_slice(output.data().as_ref());
            assert_eq!(result_value, U256::from(1), "(10 * 5) % 7 should be 1");
        }
        ExecutionResult::Revert { output, .. } => {
            panic!("execution reverted: {:?}", output);
        }
        ExecutionResult::Halt { reason, .. } => {
            panic!("execution halted: {:?}", reason);
        }
    }
}
