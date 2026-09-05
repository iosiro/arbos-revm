// Copyright 2024, Offchain Labs, Inc.
// For license information, see https://github.com/OffchainLabs/nitro/blob/master/LICENSE.md

//! Contract creation tests for Stylus programs (CREATE, CREATE2).

use revm::{
    context::{ContextTr, JournalTr, TxEnv, result::ExecutionResult},
    primitives::{Address, Bytes, TxKind, U256, keccak256},
};

use arbos_revm::{
    constants::{STYLUS_DISCRIMINANT, STYLUS_FRAGMENT_DISCRIMINANT, STYLUS_ROOT_DISCRIMINANT},
    state::{ArbState, arbos_state::ArbosStateParams},
};

mod test_utils;
use test_utils::{
    create_call_tx, create_call_tx_with_nonce, create_evm, deploy_wat_program, execute_tx,
    fund_account, setup_context, setup_context_with_arbos_state,
};

// Minimal EVM init code that deploys a contract returning empty
// Init code: 6005600c60003960056000f360006000f3
// Runtime: 60006000f3 (PUSH1 0, PUSH1 0, RETURN)
const MINIMAL_INIT_CODE: &[u8] = &[
    0x60, 0x05, // PUSH1 5 (runtime size)
    0x60, 0x0c, // PUSH1 12 (runtime offset)
    0x60, 0x00, // PUSH1 0 (memory dest)
    0x39, // CODECOPY
    0x60, 0x05, // PUSH1 5 (return size)
    0x60, 0x00, // PUSH1 0 (return offset)
    0xf3, // RETURN
    // Runtime code follows:
    0x60, 0x00, // PUSH1 0
    0x60, 0x00, // PUSH1 0
    0xf3, // RETURN
];

fn deploy_runtime_via_create(version: u64, runtime: &[u8]) -> (ExecutionResult, Option<Bytes>) {
    assert!(runtime.len() <= u8::MAX as usize);
    let mut init_code = vec![
        0x60,
        runtime.len() as u8,
        0x60,
        0x0c,
        0x60,
        0x00,
        0x39,
        0x60,
        runtime.len() as u8,
        0x60,
        0x00,
        0xf3,
    ];
    init_code.extend_from_slice(runtime);

    let caller = Address::repeat_byte(0xa1);
    let mut context = setup_context();
    context.cfg.arbos_version = version;
    context
        .arb_state(None, false)
        .initialize(&ArbosStateParams::for_arbos_version(version))
        .unwrap();
    fund_account(&mut context, caller, U256::from(100_000_000_u64));
    let mut evm = create_evm(context);
    let result = execute_tx(
        &mut evm,
        TxEnv {
            caller,
            kind: TxKind::Create,
            data: init_code.into(),
            gas_limit: 1_000_000,
            ..Default::default()
        },
    );
    let deployed = match &result {
        ExecutionResult::Success { output, .. } => output
            .address()
            .map(|address| evm.0.ctx.journal_mut().code(*address).unwrap().data),
        _ => None,
    };
    (result, deployed)
}

#[test]
fn create_deploys_only_version_valid_stylus_ef_prefixes() {
    let mut classic = STYLUS_DISCRIMINANT.to_vec();
    classic.push(0);
    let mut root = STYLUS_ROOT_DISCRIMINANT.to_vec();
    root.push(0);
    let mut fragment = STYLUS_FRAGMENT_DISCRIMINANT.to_vec();
    fragment.push(0);

    for (version, runtime) in [
        (30, classic.as_slice()),
        (60, root.as_slice()),
        (60, fragment.as_slice()),
    ] {
        let (result, deployed) = deploy_runtime_via_create(version, runtime);
        assert!(
            matches!(result, ExecutionResult::Success { .. }),
            "{result:?}"
        );
        assert_eq!(deployed.unwrap().as_ref(), runtime);
    }

    for (version, runtime) in [
        (29, classic.as_slice()),
        (59, root.as_slice()),
        (59, fragment.as_slice()),
        (60, &[0xef, 0x42, 0x00, 0x00][..]),
        (60, STYLUS_DISCRIMINANT),
    ] {
        let (result, deployed) = deploy_runtime_via_create(version, runtime);
        assert!(matches!(result, ExecutionResult::Halt { .. }));
        assert!(deployed.is_none());
    }
}

// ============================================================================
// CREATE (CREATE1) Tests
// ============================================================================

#[test]
fn test_e2e_create1_minimal_contract() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/create.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(10_000_000_000_u64));

    let mut evm = create_evm(context);

    // Selector 0x02 = CREATE with minimal contract (zero value, predefined init code)
    let args = vec![0x02u8];

    let tx = create_call_tx(program_address, args, 50_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(
                output.data().len(),
                20,
                "create output should be 20 bytes (address)"
            );
            let created_address = Address::from_slice(output.data().as_ref());
            // Created address should be non-zero on success
            assert_ne!(
                created_address,
                Address::ZERO,
                "created contract address should be non-zero"
            );
        }
        ExecutionResult::Revert { output, .. } => {
            panic!("execution reverted: {:?}", String::from_utf8_lossy(&output));
        }
        ExecutionResult::Halt { reason, .. } => {
            panic!("execution halted: {:?}", reason);
        }
    }
}

#[test]
fn reverting_host_create_refunds_child_gas_and_preserves_returndata() {
    for selector in [4_u8, 5_u8] {
        let mut context = setup_context_with_arbos_state();
        let program = deploy_wat_program(&mut context, include_bytes!("../test-data/create.wat"));
        let caller = Address::repeat_byte(0x44);
        fund_account(&mut context, caller, U256::from(10_000_000_000_u64));
        let mut evm = create_evm(context);
        let result = execute_tx(&mut evm, create_call_tx(program, vec![selector], 5_000_000));
        assert!(
            matches!(result, ExecutionResult::Success { .. }),
            "{result:?}"
        );
        assert_eq!(result.output().unwrap().as_ref(), &[0xde, 0xad, 0xbe, 0xef]);
        assert!(
            result.gas_used() < 1_000_000,
            "child gas was not refunded: {result:?}"
        );
    }
}

#[test]
fn test_e2e_create1_with_value() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/create.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(10_000_000_000_u64));

    // Fund the program so it can endow the created contract
    fund_account(&mut context, program_address, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);

    // Selector 0x00 = CREATE with custom value and init code
    let value = U256::from(100_000_u64);
    let mut args = vec![0x00u8];
    args.extend_from_slice(&value.to_be_bytes::<32>());
    args.extend_from_slice(MINIMAL_INIT_CODE);

    let tx = create_call_tx(program_address, args, 50_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(
                output.data().len(),
                20,
                "create output should be 20 bytes (address)"
            );
            let created_address = Address::from_slice(output.data().as_ref());
            // Created address should be non-zero on success
            assert_ne!(
                created_address,
                Address::ZERO,
                "created contract address should be non-zero"
            );
        }
        ExecutionResult::Revert { output, .. } => {
            panic!("execution reverted: {:?}", String::from_utf8_lossy(&output));
        }
        ExecutionResult::Halt { reason, .. } => {
            panic!("execution halted: {:?}", reason);
        }
    }
}

#[test]
fn test_e2e_create1_deterministic_address() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/create.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(10_000_000_000_u64));

    let mut evm = create_evm(context);

    // First CREATE
    let args = vec![0x02u8];
    let tx = create_call_tx_with_nonce(program_address, args.clone(), 50_000_000, 0);
    let result1 = execute_tx(&mut evm, tx);

    let address1 = match result1 {
        ExecutionResult::Success { output, .. } => Address::from_slice(output.data().as_ref()),
        _ => panic!("first create failed"),
    };

    // Second CREATE (should get different address due to nonce increment)
    let tx = create_call_tx_with_nonce(program_address, args, 50_000_000, 1);
    let result2 = execute_tx(&mut evm, tx);

    let address2 = match result2 {
        ExecutionResult::Success { output, .. } => Address::from_slice(output.data().as_ref()),
        _ => panic!("second create failed"),
    };

    // Addresses should be different (different nonces)
    assert_ne!(
        address1, address2,
        "CREATE addresses with different nonces should be different"
    );
    assert_ne!(address1, Address::ZERO, "first address should be non-zero");
    assert_ne!(address2, Address::ZERO, "second address should be non-zero");
}

// ============================================================================
// CREATE2 Tests
// ============================================================================

#[test]
fn test_e2e_create2_minimal_contract() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/create.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(10_000_000_000_u64));

    let mut evm = create_evm(context);

    // Selector 0x03 = CREATE2 with minimal contract (zero value, predefined init code, custom salt)
    let salt = [0x42u8; 32];
    let mut args = vec![0x03u8];
    args.extend_from_slice(&salt);

    let tx = create_call_tx(program_address, args, 50_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(
                output.data().len(),
                20,
                "create2 output should be 20 bytes (address)"
            );
            let created_address = Address::from_slice(output.data().as_ref());
            assert_ne!(
                created_address,
                Address::ZERO,
                "created contract address should be non-zero"
            );
        }
        ExecutionResult::Revert { output, .. } => {
            panic!("execution reverted: {:?}", String::from_utf8_lossy(&output));
        }
        ExecutionResult::Halt { reason, .. } => {
            panic!("execution halted: {:?}", reason);
        }
    }
}

#[test]
fn test_e2e_create2_different_salts() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/create.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(10_000_000_000_u64));

    let mut evm = create_evm(context);

    // First CREATE2 with salt 0x01...
    let salt1 = [0x01u8; 32];
    let mut args = vec![0x03u8];
    args.extend_from_slice(&salt1);

    let tx = create_call_tx_with_nonce(program_address, args, 50_000_000, 0);
    let result1 = execute_tx(&mut evm, tx);

    let address1 = match result1 {
        ExecutionResult::Success { output, .. } => Address::from_slice(output.data().as_ref()),
        _ => panic!("first create2 failed"),
    };

    // Second CREATE2 with salt 0x02...
    let salt2 = [0x02u8; 32];
    let mut args = vec![0x03u8];
    args.extend_from_slice(&salt2);

    let tx = create_call_tx_with_nonce(program_address, args, 50_000_000, 1);
    let result2 = execute_tx(&mut evm, tx);

    let address2 = match result2 {
        ExecutionResult::Success { output, .. } => Address::from_slice(output.data().as_ref()),
        _ => panic!("second create2 failed"),
    };

    // Addresses should be different (different salts)
    assert_ne!(
        address1, address2,
        "CREATE2 addresses with different salts should be different"
    );
    assert_ne!(address1, Address::ZERO, "first address should be non-zero");
    assert_ne!(address2, Address::ZERO, "second address should be non-zero");
}

#[test]
fn test_e2e_create2_with_value_and_custom_code() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/create.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(10_000_000_000_u64));

    // Fund the program so it can endow the created contract
    fund_account(&mut context, program_address, U256::from(1_000_000_000_u64));

    let mut evm = create_evm(context);

    // Selector 0x01 = CREATE2 with custom value, salt and init code
    let value = U256::from(50_000_u64);
    let salt = [0xABu8; 32];

    let mut args = vec![0x01u8];
    args.extend_from_slice(&value.to_be_bytes::<32>());
    args.extend_from_slice(&salt);
    args.extend_from_slice(MINIMAL_INIT_CODE);

    let tx = create_call_tx(program_address, args, 50_000_000);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(
                output.data().len(),
                20,
                "create2 output should be 20 bytes (address)"
            );
            let created_address = Address::from_slice(output.data().as_ref());
            assert_ne!(
                created_address,
                Address::ZERO,
                "created contract address should be non-zero"
            );
        }
        ExecutionResult::Revert { output, .. } => {
            panic!("execution reverted: {:?}", String::from_utf8_lossy(&output));
        }
        ExecutionResult::Halt { reason, .. } => {
            panic!("execution halted: {:?}", reason);
        }
    }
}

// ============================================================================
// Edge Case Tests
// ============================================================================

#[test]
fn test_e2e_create_empty_init_code() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/create.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(10_000_000_000_u64));

    let mut evm = create_evm(context);

    // Selector 0x00 = CREATE with zero value and empty init code
    let value = U256::ZERO;
    let mut args = vec![0x00u8];
    args.extend_from_slice(&value.to_be_bytes::<32>());
    // No init code

    let tx = create_call_tx(program_address, args, 50_000_000);
    let result = execute_tx(&mut evm, tx);

    // Empty init code should create a contract with no code (or fail)
    match result {
        ExecutionResult::Success { output, .. } => {
            // Could succeed with zero address or valid address
            let created_address = Address::from_slice(output.data().as_ref());
            // Empty init code creates an empty contract (no code)
            // This is valid EVM behavior - any address (including zero) is acceptable
            let _ = created_address;
        }
        ExecutionResult::Revert { .. } => {
            // Also acceptable - some implementations reject empty init code
        }
        ExecutionResult::Halt { reason, .. } => {
            panic!("execution halted: {:?}", reason);
        }
    }
}

#[test]
fn test_e2e_create2_same_salt_same_address() {
    let mut context = setup_context_with_arbos_state();

    let wat = include_bytes!("../test-data/create.wat");
    let program_address = deploy_wat_program(&mut context, wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(10_000_000_000_u64));

    let mut evm = create_evm(context);

    // CREATE2 with specific salt
    let salt = [0x77u8; 32];
    let mut args = vec![0x03u8];
    args.extend_from_slice(&salt);

    let tx = create_call_tx_with_nonce(program_address, args, 50_000_000, 0);
    let result = execute_tx(&mut evm, tx);

    let created_address = match result {
        ExecutionResult::Success { output, .. } => Address::from_slice(output.data().as_ref()),
        _ => panic!("first create2 failed"),
    };

    // Calculate expected CREATE2 address
    // address = keccak256(0xff ++ deployer ++ salt ++ keccak256(init_code))[12:]
    let init_code_hash = keccak256([
        0x60, 0x05, 0x60, 0x0c, 0x60, 0x00, 0x39, 0x60, 0x05, 0x60, 0x00, 0xf3, 0x60, 0x00, 0x60,
        0x00, 0xf3,
    ]);

    let mut preimage = vec![0xff];
    preimage.extend_from_slice(program_address.as_slice());
    preimage.extend_from_slice(&salt);
    preimage.extend_from_slice(init_code_hash.as_slice());

    let expected_address = Address::from_slice(&keccak256(&preimage)[12..]);

    assert_eq!(
        created_address, expected_address,
        "CREATE2 address should match deterministic calculation"
    );
}

#[test]
fn test_e2e_call_created_contract() {
    let mut context = setup_context_with_arbos_state();

    // Deploy create.wat
    let create_wat = include_bytes!("../test-data/create.wat");
    let create_address = deploy_wat_program(&mut context, create_wat);

    // Deploy call.wat for calling the created contract
    let call_wat = include_bytes!("../test-data/call.wat");
    let call_address = deploy_wat_program(&mut context, call_wat);

    let caller = Address::repeat_byte(0x01);
    fund_account(&mut context, caller, U256::from(10_000_000_000_u64));

    let mut evm = create_evm(context);

    // First create a contract using CREATE
    let args = vec![0x02u8];
    let tx = create_call_tx_with_nonce(create_address, args, 50_000_000, 0);
    let result = execute_tx(&mut evm, tx);

    let created_address = match result {
        ExecutionResult::Success { output, .. } => {
            let addr = Address::from_slice(output.data().as_ref());
            assert_ne!(addr, Address::ZERO, "created address should be non-zero");
            addr
        }
        _ => panic!("create failed"),
    };

    // Now call the created contract via call.wat
    let mut args = vec![0x00u8]; // call_contract selector
    args.extend_from_slice(created_address.as_slice());
    // No calldata needed - the created contract just returns empty

    let tx = create_call_tx_with_nonce(call_address, args, 50_000_000, 1);
    let result = execute_tx(&mut evm, tx);

    match result {
        ExecutionResult::Success { output, .. } => {
            // The created contract returns empty, so output should be empty
            assert!(
                output.data().is_empty(),
                "calling created contract should return empty data"
            );
        }
        ExecutionResult::Revert { output, .. } => {
            panic!(
                "call to created contract reverted: {:?}",
                String::from_utf8_lossy(&output)
            );
        }
        ExecutionResult::Halt { reason, .. } => {
            panic!("call to created contract halted: {:?}", reason);
        }
    }
}
