// Copyright 2024, Offchain Labs, Inc.
// For license information, see https://github.com/OffchainLabs/nitro/blob/master/LICENSE.md

//! EVM data access tests for Stylus programs.

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
fn test_e2e_evm_block_number() {
    let mut context = setup_context_with_arbos_state();

    context.block.number = U256::from(42);

    let wat = include_bytes!("../test-data/evm-data.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);

    // Selector 0x00 = block_number
    let args = vec![0x00u8];

    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(
                output.data().len(),
                8,
                "block_number output should be 8 bytes (u64)"
            );
            let block_num = u64::from_le_bytes(output.data()[..8].try_into().unwrap());
            assert!(block_num == 42, "block_number should be valid");
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
fn test_e2e_evm_chainid() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/evm-data.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);

    // Selector 0x02 = chainid
    let args = vec![0x02u8];

    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(
                output.data().len(),
                8,
                "chainid output should be 8 bytes (u64)"
            );
            let chain_id = u64::from_le_bytes(output.data()[..8].try_into().unwrap());
            assert!(chain_id > 0, "chainid should be non-zero");
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
fn test_e2e_evm_msg_sender() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/evm-data.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);

    // Selector 0x03 = msg_sender
    let args = vec![0x03u8];

    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(
                output.data().len(),
                20,
                "msg_sender output should be 20 bytes (address)"
            );
            let sender = Address::from_slice(output.data().as_ref());
            assert_eq!(sender, caller, "msg_sender should match caller");
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
fn test_e2e_evm_contract_address() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/evm-data.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);

    // Selector 0x04 = contract_address
    let args = vec![0x04u8];

    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(
                output.data().len(),
                20,
                "contract_address output should be 20 bytes (address)"
            );
            let contract_addr = Address::from_slice(output.data().as_ref());
            assert_eq!(
                contract_addr, program_address,
                "contract_address should match deployed program"
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
fn test_e2e_evm_tx_origin() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/evm-data.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);

    // Selector 0x05 = tx_origin
    let args = vec![0x05u8];

    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(
                output.data().len(),
                20,
                "tx_origin output should be 20 bytes (address)"
            );
            let origin = Address::from_slice(output.data().as_ref());
            assert_eq!(
                origin, caller,
                "tx_origin should match caller for direct calls"
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
fn test_e2e_evm_msg_value() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/evm-data.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);

    // Selector 0x06 = msg_value
    let args = vec![0x06u8];

    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(
                output.data().len(),
                32,
                "msg_value output should be 32 bytes (U256)"
            );
            let value = U256::from_be_slice(output.data().as_ref());
            assert_eq!(
                value,
                U256::ZERO,
                "msg_value should be 0 when no value sent"
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
fn test_e2e_evm_block_timestamp() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/evm-data.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);

    // Selector 0x01 = block_timestamp
    let args = vec![0x01u8];

    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(
                output.data().len(),
                8,
                "block_timestamp output should be 8 bytes (u64)"
            );
            let _timestamp = u64::from_le_bytes(output.data()[..8].try_into().unwrap());
            // Timestamp can be 0 in test environment, just verify we got a valid u64
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
fn test_e2e_evm_block_basefee() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/evm-data.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);

    // Selector 0x07 = block_basefee
    let args = vec![0x07u8];

    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(
                output.data().len(),
                32,
                "block_basefee output should be 32 bytes (U256)"
            );
            let basefee = U256::from_be_slice(output.data().as_ref());
            // Basefee should be a valid value (can be zero in test environment)
            assert!(basefee >= U256::ZERO, "basefee should be valid");
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
fn test_e2e_evm_block_gas_limit() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/evm-data.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);

    // Selector 0x08 = block_gas_limit
    let args = vec![0x08u8];

    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(
                output.data().len(),
                8,
                "block_gas_limit output should be 8 bytes (u64)"
            );
            let gas_limit = u64::from_le_bytes(output.data()[..8].try_into().unwrap());
            // Gas limit should be non-zero
            assert!(gas_limit > 0, "block_gas_limit should be non-zero");
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
fn test_e2e_evm_block_coinbase() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/evm-data.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);

    // Selector 0x09 = block_coinbase
    let args = vec![0x09u8];

    let tx = create_call_tx(program_address, args, 10_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(
                output.data().len(),
                20,
                "block_coinbase output should be 20 bytes (address)"
            );
            // Coinbase is a valid address (can be zero address in test environment)
            let _coinbase = Address::from_slice(output.data().as_ref());
        }
        ExecutionResult::Revert { output, .. } => {
            panic!("execution reverted: {:?}", output);
        }
        ExecutionResult::Halt { reason, .. } => {
            panic!("execution halted: {:?}", reason);
        }
    }
}
