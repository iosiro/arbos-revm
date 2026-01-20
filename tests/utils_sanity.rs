// Copyright 2024, Offchain Labs, Inc.
// For license information, see https://github.com/OffchainLabs/nitro/blob/master/LICENSE.md

//! Sanity tests for test utilities - runs once instead of per-file.

mod test_utils;
use test_utils::*;

use arbos_revm::{
    constants::STYLUS_DISCRIMINANT,
    state::{ArbState, ArbStateGetter},
};
use revm::{
    context::{ContextTr, JournalTr},
    primitives::{Address, U256},
};

#[test]
fn test_setup_context() {
    let context = setup_context();
    assert!(context.error.is_ok());
}

#[test]
fn test_setup_context_with_arbos_state() {
    let mut context = setup_context_with_arbos_state();

    // Verify stylus params are initialized
    let params = context
        .arb_state(None, true)
        .programs()
        .stylus_params()
        .get()
        .expect("failed to get stylus params");

    assert_eq!(params.version, 2); // INITIAL_STYLUS_VERSION
    assert!(params.ink_price > 0);
}

#[test]
fn test_deploy_wat_program() {
    let mut context = setup_context();
    let wat = include_bytes!("../test-data/add.wat");
    let address = deploy_wat_program(&mut context, wat);

    // Verify the code was deployed
    let code = context.journal_mut().code(address).unwrap();
    assert!(!code.data.is_empty());
    assert!(code.data.starts_with(STYLUS_DISCRIMINANT));
}

#[test]
fn test_fund_account() {
    let mut context = setup_context();
    let address = Address::repeat_byte(0xAB);
    let amount = U256::from(1_000_000);

    fund_account(&mut context, address, amount);

    let account = context.journal_mut().load_account(address).unwrap();
    assert_eq!(account.data.info.balance, amount);
}

#[test]
fn test_compile_wat() {
    let wasm = compile_wat("test-data/add.wat");
    assert!(!wasm.is_empty());
    // WASM magic number
    assert_eq!(&wasm[0..4], b"\0asm");
}
