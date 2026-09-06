// Copyright 2024, Offchain Labs, Inc.
// For license information, see https://github.com/OffchainLabs/nitro/blob/master/LICENSE.md

mod test_utils;

use alloy_sol_types::{SolCall, sol};
use arbos_revm::{
    constants::{
        ARBOS_GENESIS_TIMESTAMP, ARBOS_PROGRAMS_STATE_PARAMS_KEY,
        ARBOS_PROGRAMS_STATE_PROGRAM_DATA_KEY, ARBOS_STATE_ADDRESS, ARBOS_STATE_PROGRAMS_KEY,
    },
    state::{
        ArbState, ArbStateGetter,
        arbos_state::ArbosStateParams,
        program::ProgramInfo,
        types::{map_address, substorage},
    },
};
use revm::{
    context::{ContextTr, JournalTr},
    primitives::{Address, B256, U256, address},
};
use test_utils::{create_evm, execute_tx, fund_account, setup_context};

sol! {
    function setWasmMinInitGas(uint8 gas, uint16 cached);
}

fn program_data_slot(code_hash: B256) -> B256 {
    let programs = substorage(&B256::ZERO, ARBOS_STATE_PROGRAMS_KEY);
    let program_data = substorage(&programs, ARBOS_PROGRAMS_STATE_PROGRAM_DATA_KEY);
    map_address(&program_data, &code_hash)
}

fn stylus_params_slot() -> B256 {
    let programs = substorage(&B256::ZERO, ARBOS_STATE_PROGRAMS_KEY);
    let params = substorage(&programs, ARBOS_PROGRAMS_STATE_PARAMS_KEY);
    map_address(&params, &B256::ZERO)
}

#[test]
fn stylus_params_apply_nitro_versioned_tail_layout() {
    let mut context = setup_context();
    context.cfg.arbos_version = 31;
    context
        .arb_state(None, false)
        .initialize(&ArbosStateParams::for_arbos_version(31))
        .unwrap();

    let raw = context
        .journal_mut()
        .sload(ARBOS_STATE_ADDRESS, stylus_params_slot().into())
        .unwrap()
        .data
        .to_be_bytes::<32>();
    assert_eq!(&raw[25..], &[0; 7]);

    let params = context
        .arb_state(None, true)
        .programs()
        .stylus_params()
        .get()
        .unwrap();
    assert_eq!(params.max_wasm_size, 128 * 1024);
    assert_eq!(params.max_fragment_count, 0);
}

#[test]
fn program_record_matches_nitro_byte_layout_and_epoch() {
    let mut context = setup_context();
    context.cfg.arbos_version = 60;
    let activation_hour = 0x01_02_03u64;
    context.block.timestamp = U256::from(ARBOS_GENESIS_TIMESTAMP + activation_hour * 3600 + 3599);
    context
        .arb_state(None, false)
        .initialize(&ArbosStateParams::for_arbos_version(60))
        .unwrap();

    let code_hash = B256::repeat_byte(0x77);
    context
        .arb_state(None, false)
        .programs()
        .save_program_info(
            &code_hash,
            &ProgramInfo {
                version: 0x1234,
                init_cost: 0x2345,
                cached_cost: 0x3456,
                footprint: 0x4567,
                asm_estimated_kb: 0x0a_0b_0c,
                age: 0,
                cached: true,
            },
        )
        .unwrap();

    let raw = context
        .journal_mut()
        .sload(ARBOS_STATE_ADDRESS, program_data_slot(code_hash).into())
        .unwrap()
        .data;
    assert_eq!(
        raw,
        U256::from_be_slice(&[
            0x12, 0x34, 0x23, 0x45, 0x34, 0x56, 0x45, 0x67, 0x01, 0x02, 0x03, 0x0a, 0x0b, 0x0c,
            0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ])
    );

    let decoded = context
        .arb_state(None, true)
        .programs()
        .program_info(&code_hash)
        .unwrap()
        .unwrap();
    assert_eq!(decoded.asm_estimated_kb, 0x0a_0b_0c);
    assert_eq!(decoded.age, 3599);
}

#[test]
fn program_record_decodes_nitro_layout_with_u64_age() {
    let mut context = setup_context();
    context.cfg.arbos_version = 60;
    let activation_hour = 0x02_03_04u64;
    context.block.timestamp =
        U256::from(ARBOS_GENESIS_TIMESTAMP + activation_hour * 3600 + u64::from(u32::MAX) + 7);
    context
        .arb_state(None, false)
        .initialize(&ArbosStateParams::for_arbos_version(60))
        .unwrap();
    let code_hash = B256::repeat_byte(0x88);
    let raw = U256::from_be_slice(&[
        0, 3, 0, 1, 0, 2, 0, 3, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0,
    ]);
    context
        .journal_mut()
        .sstore(
            ARBOS_STATE_ADDRESS,
            program_data_slot(code_hash).into(),
            raw,
        )
        .unwrap();

    let decoded = context
        .arb_state(None, true)
        .programs()
        .program_info(&code_hash)
        .unwrap()
        .unwrap();
    assert_eq!(decoded.asm_estimated_kb, 0x05_06_07);
    assert_eq!(decoded.age, u64::from(u32::MAX) + 7);
}

#[test]
fn set_wasm_min_init_gas_uses_nitro_units_and_saturation() {
    let mut context = setup_context();
    context.cfg.arbos_version = 60;
    context
        .arb_state(None, false)
        .initialize(&ArbosStateParams::for_arbos_version(60))
        .unwrap();
    let owner = Address::repeat_byte(0x42);
    fund_account(&mut context, owner, U256::from(1_000_000));
    context
        .arb_state(None, false)
        .chain_owners()
        .add(owner)
        .unwrap();
    let mut evm = create_evm(context);

    let result = execute_tx(
        &mut evm,
        revm::context::TxEnv {
            caller: owner,
            kind: revm::primitives::TxKind::Call(address!(
                "0000000000000000000000000000000000000070"
            )),
            data: setWasmMinInitGasCall {
                gas: 128,
                cached: u16::MAX,
            }
            .abi_encode()
            .into(),
            gas_limit: 100_000,
            gas_price: 1,
            ..Default::default()
        },
    );
    assert!(matches!(
        result,
        revm::context::result::ExecutionResult::Success { .. }
    ));
    let params = evm
        .0
        .ctx
        .arb_state(None, true)
        .programs()
        .stylus_params()
        .get()
        .unwrap();
    assert_eq!(params.min_init_gas, 1);
    assert_eq!(params.min_cached_init_gas, u8::MAX);

    let raw_params = evm
        .0
        .ctx
        .journal_mut()
        .sload(ARBOS_STATE_ADDRESS, stylus_params_slot().into())
        .unwrap()
        .data
        .to_be_bytes::<32>();
    assert_eq!(raw_params[15], 1);
    assert_eq!(raw_params[16], u8::MAX);
}

#[test]
fn upgrades_initialize_and_transition_stylus_parameters_at_nitro_versions() {
    let mut context = setup_context();
    context.cfg.arbos_version = 29;
    context
        .arb_state(None, false)
        .initialize(&ArbosStateParams::for_arbos_version(29))
        .unwrap();
    // Execution is configured for the post-upgrade ArbOS version while the
    // persisted version still drives which migrations run.
    context.cfg.arbos_version = 60;

    context
        .arb_state(None, false)
        .upgrade_arbos_version(40)
        .unwrap();
    let params = context
        .arb_state(None, true)
        .programs()
        .stylus_params()
        .get()
        .unwrap();
    assert_eq!(params.version, 2);
    assert_eq!(params.min_init_gas, 69);
    assert_eq!(params.max_wasm_size, 128 * 1024);

    context
        .arb_state(None, false)
        .upgrade_arbos_version(60)
        .unwrap();
    let params = context
        .arb_state(None, true)
        .programs()
        .stylus_params()
        .get()
        .unwrap();
    assert_eq!(params.version, 3);
    assert_eq!(params.max_wasm_size, 256 * 1024);
    assert_eq!(params.max_fragment_count, 4);
}
