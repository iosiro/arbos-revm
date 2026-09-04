// Copyright 2024, Offchain Labs, Inc.
// For license information, see https://github.com/OffchainLabs/nitro/blob/master/LICENSE.md

//! Focused checks for Nitro's single-dimensional ArbOS fee routing.

#[allow(dead_code)]
mod test_utils;
use test_utils::{create_evm, execute_tx, fund_account, setup_context};

use arbos_revm::state::{
    ArbState, ArbStateGetter, arbos_state::ArbosStateParams, types::StorageBackedTr,
};
use arbos_revm::{constants::ARBOS_L1_PRICER_FUNDS_ADDRESS, transaction::ArbitrumTransaction};
use revm::{
    ExecuteEvm,
    context::{ContextTr, JournalTr, TxEnv, result::ExecutionResult},
    primitives::{Address, Bytes, TxKind, U256},
    state::Bytecode,
};

fn balance(evm: &mut test_utils::TestEvm, address: Address) -> U256 {
    evm.0
        .ctx
        .journal_mut()
        .load_account(address)
        .unwrap()
        .data
        .info
        .balance
}

#[test]
fn poster_fee_updates_recognized_l1_fees_and_pool_balance() {
    let caller = Address::repeat_byte(0x11);
    let recipient = Address::repeat_byte(0x22);
    let mut context = setup_context();
    context.cfg.arbos_version = 60;
    context.block.basefee = 100;
    context
        .arb_state(None, false)
        .initialize(&ArbosStateParams::for_arbos_version(60))
        .unwrap();
    context
        .arb_state(None, false)
        .l1_pricing()
        .price_per_unit()
        .set(U256::from(10))
        .unwrap();
    fund_account(&mut context, caller, U256::from(1_000_000_000_u64));

    let tx = TxEnv {
        tx_type: 2,
        caller,
        gas_limit: 30_000,
        gas_price: 100,
        kind: TxKind::Call(recipient),
        ..Default::default()
    };
    let mut evm = create_evm(context);
    let result = evm
        .transact_one(ArbitrumTransaction::new_with_enveloped(
            tx,
            vec![1, 2, 3, 4].into(),
        ))
        .expect("transaction execution failed");
    assert!(matches!(result, ExecutionResult::Success { .. }));

    let pool_balance = balance(&mut evm, ARBOS_L1_PRICER_FUNDS_ADDRESS);
    let recognized = evm
        .0
        .ctx
        .arb_state(None, true)
        .l1_pricing()
        .l1_fees_available()
        .get()
        .unwrap();
    let units = evm
        .0
        .ctx
        .arb_state(None, true)
        .l1_pricing()
        .units_since_update()
        .get()
        .unwrap();
    assert!(!pool_balance.is_zero());
    assert_eq!(recognized, pool_balance);
    assert_eq!(
        units,
        arbos_revm::l1_fee::compressed_data_units(&vec![1, 2, 3, 4].into(), 1).unwrap()
    );
}

#[test]
fn zero_poster_price_still_accumulates_units_without_collecting_fees() {
    let caller = Address::repeat_byte(0x11);
    let recipient = Address::repeat_byte(0x22);
    let envelope = Bytes::from_static(&[1, 2, 3, 4]);
    let mut context = setup_context();
    context.cfg.arbos_version = 60;
    context.block.basefee = 100;
    context
        .arb_state(None, false)
        .initialize(&ArbosStateParams::for_arbos_version(60))
        .unwrap();
    context
        .arb_state(None, false)
        .l1_pricing()
        .price_per_unit()
        .set(U256::ZERO)
        .unwrap();
    fund_account(&mut context, caller, U256::from(100_000_000_u64));
    let mut evm = create_evm(context);
    let result = evm
        .transact_one(ArbitrumTransaction::new_with_enveloped(
            TxEnv {
                tx_type: 2,
                caller,
                gas_limit: 30_000,
                gas_price: 100,
                kind: TxKind::Call(recipient),
                ..Default::default()
            },
            envelope.clone(),
        ))
        .unwrap();
    assert!(matches!(result, ExecutionResult::Success { .. }));
    let mut state = evm.0.ctx.arb_state(None, true);
    assert_eq!(
        state.l1_pricing().units_since_update().get().unwrap(),
        arbos_revm::l1_fee::compressed_data_units(&envelope, 1).unwrap()
    );
    assert_eq!(
        state.l1_pricing().l1_fees_available().get().unwrap(),
        U256::ZERO
    );
    assert_eq!(balance(&mut evm, ARBOS_L1_PRICER_FUNDS_ADDRESS), U256::ZERO);
}

#[test]
fn zero_basefee_and_retry_transactions_do_not_accrue_poster_units() {
    for (basefee, tx_type) in [(0, 2), (100, arbos_revm::constants::ARBITRUM_RETRY_TX_TYPE)] {
        let caller = Address::repeat_byte(0x11);
        let mut context = setup_context();
        context.cfg.arbos_version = 60;
        context.block.basefee = basefee;
        context
            .arb_state(None, false)
            .initialize(&ArbosStateParams::for_arbos_version(60))
            .unwrap();
        fund_account(&mut context, caller, U256::from(100_000_000_u64));
        let mut evm = create_evm(context);
        let result = evm
            .transact_one(ArbitrumTransaction::new_with_enveloped(
                TxEnv {
                    tx_type,
                    caller,
                    gas_limit: 30_000,
                    gas_price: basefee as u128,
                    kind: TxKind::Call(Address::repeat_byte(0x22)),
                    ..Default::default()
                },
                Bytes::from_static(&[1, 2, 3, 4]),
            ))
            .unwrap();
        assert!(matches!(result, ExecutionResult::Success { .. }));
        assert_eq!(
            evm.0
                .ctx
                .arb_state(None, true)
                .l1_pricing()
                .units_since_update()
                .get()
                .unwrap(),
            0
        );
        assert_eq!(balance(&mut evm, ARBOS_L1_PRICER_FUNDS_ADDRESS), U256::ZERO);
    }
}

fn run_fee_routing(version: u64, collect_tips: bool) -> (u64, U256, U256, U256, U256) {
    let caller = Address::repeat_byte(0x11);
    let recipient = Address::repeat_byte(0x22);
    let network = Address::repeat_byte(0x33);
    let infra = Address::repeat_byte(0x44);
    let coinbase = Address::repeat_byte(0x55);

    let mut context = setup_context();
    context.cfg.arbos_version = version;
    context.block.basefee = 100;
    context.block.beneficiary = coinbase;
    let mut params = ArbosStateParams::for_arbos_version(version);
    params.network_fee_account = network;
    params.infra_fee_account = infra;
    context.arb_state(None, false).initialize(&params).unwrap();
    context
        .arb_state(None, false)
        .l2_pricing()
        .min_base_fee_wei()
        .set(U256::from(40))
        .unwrap();
    context
        .arb_state(None, false)
        .set_collect_tips(collect_tips)
        .unwrap();
    let initial_balance = U256::from(100_000_000_u64);
    fund_account(&mut context, caller, initial_balance);

    let tx = TxEnv {
        tx_type: 2,
        caller,
        gas_limit: 30_000,
        gas_price: 120,
        gas_priority_fee: Some(20),
        kind: TxKind::Call(recipient),
        ..Default::default()
    };
    let mut evm = create_evm(context);
    let result = execute_tx(&mut evm, tx);
    let gas_used = match result {
        ExecutionResult::Success { gas_used, .. } => gas_used,
        other => panic!("transaction failed: {other:?}"),
    };
    (
        gas_used,
        balance(&mut evm, network),
        balance(&mut evm, infra),
        balance(&mut evm, coinbase),
        initial_balance - balance(&mut evm, caller),
    )
}

#[test]
fn arbos_60_routes_enabled_tips_to_network_account() {
    let (gas_used, network, infra, coinbase, charged) = run_fee_routing(60, true);
    assert_eq!(infra, U256::from(40 * gas_used));
    assert_eq!(network, U256::from(80 * gas_used));
    assert_eq!(coinbase, U256::ZERO);
    assert_eq!(charged, U256::from(120 * gas_used));
}

#[test]
fn pre_arbos_60_drops_tips_even_if_storage_flag_is_set() {
    let (gas_used, network, infra, coinbase, charged) = run_fee_routing(59, true);
    assert_eq!(infra, U256::from(40 * gas_used));
    assert_eq!(network, U256::from(60 * gas_used));
    assert_eq!(coinbase, U256::ZERO);
    assert_eq!(charged, U256::from(100 * gas_used));
}

#[test]
fn arbos_60_drops_tips_and_refunds_caller_when_disabled() {
    let (gas_used, network, infra, coinbase, charged) = run_fee_routing(60, false);
    assert_eq!(infra, U256::from(40 * gas_used));
    assert_eq!(network, U256::from(60 * gas_used));
    assert_eq!(coinbase, U256::ZERO);
    assert_eq!(charged, U256::from(100 * gas_used));
}

#[test]
fn arbos_9_collects_tips_regardless_of_storage_flag() {
    let (gas_used, network, infra, coinbase, charged) = run_fee_routing(9, false);
    assert_eq!(infra, U256::ZERO);
    assert_eq!(network, U256::from(120 * gas_used));
    assert_eq!(coinbase, U256::ZERO);
    assert_eq!(charged, U256::from(120 * gas_used));
}

fn observed_gas_price(version: u64, collect_tips: bool) -> U256 {
    let caller = Address::repeat_byte(0x61);
    let contract = Address::repeat_byte(0x62);
    let mut context = setup_context();
    context.cfg.arbos_version = version;
    context.block.basefee = 100;
    context
        .arb_state(None, false)
        .initialize(&ArbosStateParams::for_arbos_version(version))
        .unwrap();
    context
        .arb_state(None, false)
        .set_collect_tips(collect_tips)
        .unwrap();
    fund_account(&mut context, caller, U256::from(100_000_000_u64));
    context.journal_mut().load_account(contract).unwrap();
    // GASPRICE; MSTORE(0); RETURN(0, 32)
    context.journal_mut().set_code(
        contract,
        Bytecode::new_raw(Bytes::from_static(&[
            0x3a, 0x60, 0x00, 0x52, 0x60, 0x20, 0x60, 0x00, 0xf3,
        ])),
    );

    let result = execute_tx(
        &mut create_evm(context),
        TxEnv {
            tx_type: 2,
            caller,
            gas_limit: 30_000,
            gas_price: 120,
            gas_priority_fee: Some(20),
            kind: TxKind::Call(contract),
            ..Default::default()
        },
    );
    match result {
        ExecutionResult::Success { output, .. } => U256::from_be_slice(output.data().as_ref()),
        other => panic!("transaction failed: {other:?}"),
    }
}

#[test]
fn gasprice_observes_nitro_tip_collection_policy() {
    assert_eq!(observed_gas_price(59, true), U256::from(100));
    assert_eq!(observed_gas_price(60, false), U256::from(100));
    assert_eq!(observed_gas_price(60, true), U256::from(120));
    assert_eq!(observed_gas_price(9, false), U256::from(120));
}

#[test]
fn zero_gas_price_does_not_grow_l2_backlog() {
    for gas_price in [0_u128, 1] {
        let caller = Address::repeat_byte(0x71);
        let mut context = setup_context();
        context.cfg.arbos_version = 60;
        context.block.basefee = gas_price as u64;
        context
            .arb_state(None, false)
            .initialize(&ArbosStateParams::for_arbos_version(60))
            .unwrap();
        fund_account(&mut context, caller, U256::from(1_000_000_u64));
        let mut evm = create_evm(context);
        let result = execute_tx(
            &mut evm,
            TxEnv {
                caller,
                gas_limit: 21_000,
                gas_price,
                kind: TxKind::Call(Address::repeat_byte(0x72)),
                ..Default::default()
            },
        );
        let gas_used = result.gas_used();
        let backlog = evm
            .0
            .ctx
            .arb_state(None, true)
            .l2_pricing()
            .gas_backlog()
            .get()
            .unwrap();
        assert_eq!(backlog, if gas_price == 0 { 0 } else { gas_used });
    }
}

fn run_gas_cap(version: u64) -> (ExecutionResult, U256) {
    let caller = Address::repeat_byte(0x81);
    let contract = Address::repeat_byte(0x82);
    let initial_balance = U256::from(100_000);
    let mut context = setup_context();
    context.cfg.arbos_version = version;
    context.block.basefee = 1;
    context
        .arb_state(None, false)
        .initialize(&ArbosStateParams::for_arbos_version(version))
        .unwrap();
    if version >= 50 {
        context
            .arb_state(None, false)
            .l2_pricing()
            .per_tx_gas_limit()
            .set(22_000)
            .unwrap();
    } else {
        context
            .arb_state(None, false)
            .l2_pricing()
            .per_block_gas_limit()
            .set(1_000)
            .unwrap();
    }
    fund_account(&mut context, caller, initial_balance);
    context.journal_mut().load_account(contract).unwrap();
    // JUMPDEST PUSH1(0) JUMP: consume gas until the capped frame allowance is exhausted.
    context.journal_mut().set_code(
        contract,
        Bytecode::new_raw(Bytes::from_static(&[0x5b, 0x60, 0x00, 0x56])),
    );
    let mut evm = create_evm(context);
    let result = execute_tx(
        &mut evm,
        TxEnv {
            caller,
            gas_limit: 30_000,
            gas_price: 1,
            kind: TxKind::Call(contract),
            ..Default::default()
        },
    );
    (result, initial_balance - balance(&mut evm, caller))
}

#[test]
fn arbos_gas_caps_limit_execution_but_refund_held_gas() {
    for version in [49, 60] {
        let (result, charged) = run_gas_cap(version);
        assert!(matches!(
            result,
            ExecutionResult::Halt {
                gas_used: 22_000,
                ..
            }
        ));
        assert_eq!(charged, U256::from(22_000));
    }
}
