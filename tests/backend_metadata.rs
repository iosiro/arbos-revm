// Copyright 2024, Offchain Labs, Inc.
// For license information, see https://github.com/OffchainLabs/nitro/blob/master/LICENSE.md

//! Functional coverage for metadata supplied by the Arbitrum execution family.

#[allow(dead_code)]
mod test_utils;

use arbos_revm::{
    ArbitrumChainTr, ArbitrumCommittedFailure, ArbitrumRetryTx, ArbitrumTransaction,
    state::{ArbState, ArbStateGetter, arbos_state::ArbosStateParams, types::StorageBackedTr},
};
use revm::{
    context::{ContextTr, JournalTr, TxEnv, result::ExecutionResult},
    primitives::{Address, Bytes, TxKind, U256, address, keccak256},
    state::Bytecode,
};
use test_utils::{create_evm, deploy_wat_program, execute_tx, fund_account, setup_context};

fn call_forwarder(target: Address, static_call: bool) -> Bytecode {
    // Copy calldata to memory, forward it, copy returndata, and return it.
    let mut code = vec![0x36, 0x5f, 0x5f, 0x37, 0x5f, 0x5f, 0x36, 0x5f];
    if !static_call {
        code.push(0x5f); // CALL value
    }
    code.push(0x73);
    code.extend_from_slice(target.as_slice());
    code.extend_from_slice(&[0x5a, if static_call { 0xfa } else { 0xf1 }, 0x50]);
    code.extend_from_slice(&[0x3d, 0x5f, 0x5f, 0x3e, 0x3d, 0x5f, 0xf3]);
    Bytecode::new_raw(Bytes::from(code))
}

fn reverting_call_forwarder(target: Address) -> Bytecode {
    let mut code = vec![0x36, 0x5f, 0x5f, 0x37, 0x5f, 0x5f, 0x36, 0x5f, 0x5f, 0x73];
    code.extend_from_slice(target.as_slice());
    code.extend_from_slice(&[0x5a, 0xf1, 0x50, 0x5f, 0x5f, 0xfd]);
    Bytecode::new_raw(Bytes::from(code))
}

fn propagating_call_forwarder(target: Address) -> Bytecode {
    let mut code = vec![0x36, 0x5f, 0x5f, 0x37, 0x5f, 0x5f, 0x36, 0x5f, 0x5f, 0x73];
    code.extend_from_slice(target.as_slice());
    code.extend_from_slice(&[0x5a, 0xf1, 0x15, 0x61, 0x00, 0x00, 0x57]);
    code.extend_from_slice(&[0x3d, 0x5f, 0x5f, 0x3e, 0x3d, 0x5f, 0xf3]);
    let revert_pc = code.len() as u16;
    code.extend_from_slice(&[0x5b, 0x5f, 0x5f, 0xfd]);
    // Immediate bytes following PUSH2.
    let push2 = code
        .iter()
        .position(|byte| *byte == 0x61)
        .expect("PUSH2 placeholder");
    code[push2 + 1..push2 + 3].copy_from_slice(&revert_pc.to_be_bytes());
    Bytecode::new_raw(Bytes::from(code))
}

fn install(context: &mut test_utils::TestContext, address: Address, code: Bytecode) {
    context.journal_mut().load_account(address).unwrap();
    context.journal_mut().set_code(address, code);
}

#[test]
fn arb_sys_observes_actual_parent_caller_at_depth_three() {
    let origin = Address::repeat_byte(0x11);
    let outer = Address::repeat_byte(0x22);
    let inner = Address::repeat_byte(0x33);
    let arb_sys = address!("0000000000000000000000000000000000000064");
    let mut context = setup_context();
    context.cfg.arbos_version = 60;
    context
        .arb_state(None, false)
        .initialize(&ArbosStateParams::for_arbos_version(60))
        .unwrap();
    fund_account(&mut context, origin, U256::from(100_000_000_u64));
    install(&mut context, inner, call_forwarder(arb_sys, false));
    install(&mut context, outer, call_forwarder(inner, false));

    let selector = &keccak256("myCallersAddressWithoutAliasing()")[..4];
    let result = execute_tx(
        &mut create_evm(context),
        TxEnv {
            caller: origin,
            gas_limit: 1_000_000,
            gas_price: 0,
            kind: TxKind::Call(outer),
            data: Bytes::copy_from_slice(selector),
            ..Default::default()
        },
    );
    let ExecutionResult::Success { output, .. } = result else {
        panic!("nested ArbSys call failed: {result:?}");
    };
    assert_eq!(Address::from_slice(&output.data()[12..32]), outer);
}

#[test]
fn direct_static_scheme_is_distinct_from_inherited_static_state() {
    let origin = Address::repeat_byte(0x41);
    let outer = Address::repeat_byte(0x42);
    let inner = Address::repeat_byte(0x43);
    let address_table = address!("0000000000000000000000000000000000000066");
    let registered = Address::repeat_byte(0x44);
    let mut context = setup_context();
    context.cfg.arbos_version = 49;
    context
        .arb_state(None, false)
        .initialize(&ArbosStateParams::for_arbos_version(49))
        .unwrap();
    context
        .arb_state(None, false)
        .l2_pricing()
        .per_tx_gas_limit()
        .set(10_000)
        .unwrap();
    fund_account(&mut context, origin, U256::from(100_000_000_u64));
    install(&mut context, inner, call_forwarder(address_table, false));
    install(&mut context, outer, call_forwarder(inner, true));

    let mut calldata = keccak256("register(address)")[..4].to_vec();
    calldata.extend_from_slice(&[0; 12]);
    calldata.extend_from_slice(registered.as_slice());
    let mut evm = create_evm(context);
    let result = execute_tx(
        &mut evm,
        TxEnv {
            caller: origin,
            gas_limit: 100_000,
            gas_price: 0,
            kind: TxKind::Call(outer),
            data: Bytes::from(calldata),
            ..Default::default()
        },
    );
    assert!(
        matches!(result, ExecutionResult::Success { .. }),
        "nested write-precompile call failed: {result:?}"
    );
    assert_eq!(
        evm.0
            .ctx
            .arb_state(None, true)
            .address_table()
            .lookup(registered)
            .unwrap(),
        Some(0)
    );
}

#[test]
fn reverted_redeem_does_not_publish_scheduled_retry() {
    let origin = Address::repeat_byte(0x51);
    let helper = Address::repeat_byte(0x52);
    let retry_from = Address::repeat_byte(0x53);
    let retry_to = Address::repeat_byte(0x54);
    let ticket_id = revm::primitives::B256::repeat_byte(0x55);
    let retryable = address!("000000000000000000000000000000000000006e");
    let mut context = setup_context();
    context.cfg.arbos_version = 59;
    context.block.basefee = 1;
    context.block.timestamp = U256::from(100);
    context
        .arb_state(None, false)
        .initialize(&ArbosStateParams::for_arbos_version(59))
        .unwrap();
    context
        .arb_state(None, false)
        .retryable_state()
        .create_retryable(
            ticket_id,
            1_000,
            retry_from,
            Some(retry_to),
            U256::ZERO,
            origin,
            &Bytes::new(),
        )
        .unwrap();
    fund_account(&mut context, origin, U256::from(100_000_000_u64));
    install(&mut context, helper, reverting_call_forwarder(retryable));

    let mut calldata = keccak256("redeem(bytes32)")[..4].to_vec();
    calldata.extend_from_slice(ticket_id.as_slice());
    let mut evm = create_evm(context);
    let result = execute_tx(
        &mut evm,
        TxEnv {
            caller: origin,
            gas_limit: 200_000,
            gas_price: 1,
            kind: TxKind::Call(helper),
            data: Bytes::from(calldata),
            ..Default::default()
        },
    );
    assert!(matches!(result, ExecutionResult::Revert { .. }));
    assert!(evm.0.ctx.chain().scheduled_retries().is_empty());
    assert_eq!(
        evm.0
            .ctx
            .arb_state(None, true)
            .retryable(ticket_id)
            .num_tries()
            .get()
            .unwrap(),
        0
    );
}

#[test]
fn scheduled_retry_cannot_cancel_its_own_ticket() {
    let retry_from = Address::repeat_byte(0x61);
    let helper = Address::repeat_byte(0x62);
    let beneficiary = Address::repeat_byte(0x63);
    let ticket_id = revm::primitives::B256::repeat_byte(0x64);
    let retryable_precompile = address!("000000000000000000000000000000000000006e");
    let mut context = setup_context();
    context.cfg.arbos_version = 60;
    context.cfg.chain_id = 42_161;
    context.block.basefee = 0;
    context.block.timestamp = U256::from(100);
    context
        .arb_state(None, false)
        .initialize(&ArbosStateParams::for_arbos_version(60))
        .unwrap();
    context
        .arb_state(None, false)
        .retryable_state()
        .create_retryable(
            ticket_id,
            1_000,
            retry_from,
            Some(helper),
            U256::ZERO,
            beneficiary,
            &Bytes::new(),
        )
        .unwrap();
    install(
        &mut context,
        helper,
        propagating_call_forwarder(retryable_precompile),
    );

    let mut calldata = keccak256("cancel(bytes32)")[..4].to_vec();
    calldata.extend_from_slice(ticket_id.as_slice());
    context.chain.schedule_retry(ArbitrumRetryTx {
        chain_id: U256::from(42_161),
        nonce: 0,
        from: retry_from,
        gas_fee_cap: U256::ZERO,
        gas_limit: 200_000,
        to: Some(helper),
        value: U256::ZERO,
        data: Bytes::from(calldata),
        ticket_id,
        refund_to: beneficiary,
        max_refund: U256::ZERO,
        submission_fee_refund: U256::ZERO,
    });

    let mut evm = create_evm(context);
    let result = evm
        .transact_next_scheduled_retry()
        .expect("retry execution errored")
        .expect("retry queue was empty");
    assert!(matches!(result, ExecutionResult::Revert { .. }));
    assert_eq!(
        evm.0
            .ctx
            .arb_state(None, true)
            .retryable(ticket_id)
            .timeout()
            .get()
            .unwrap(),
        1_000
    );
}

#[test]
fn memory_fill_overflow_surfaces_filter_outcome_and_versioned_cache_behavior() {
    let wat = include_bytes!("../test-data/memory-fill-value-overflow.wat");
    let envelope = Bytes::from_static(&[0x02, 0xaa, 0xbb]);
    let canonical_hash = keccak256(&envelope);

    for (version, expect_filtered) in [(42, true), (61, false)] {
        let caller = Address::repeat_byte(version as u8);
        let mut context = setup_context();
        context.cfg.arbos_version = version;
        context
            .arb_state(None, false)
            .initialize(&ArbosStateParams::for_arbos_version(version))
            .unwrap();
        let program = deploy_wat_program(&mut context, wat);
        fund_account(&mut context, caller, U256::from(100_000_000_u64));
        let mut evm = create_evm(context);
        let outcome = evm
            .transact_one_arbitrum(ArbitrumTransaction::new_with_enveloped(
                TxEnv {
                    caller,
                    gas_limit: 1_000_000,
                    gas_price: 0,
                    kind: TxKind::Call(program),
                    ..Default::default()
                },
                envelope.clone(),
            ))
            .unwrap();

        if expect_filtered {
            assert!(!outcome.result.is_success());
            assert_eq!(
                outcome.committed_failure,
                Some(ArbitrumCommittedFailure::FilteredTransaction {
                    hash: canonical_hash,
                })
            );
        } else {
            assert!(outcome.result.is_success());
            assert_eq!(outcome.committed_failure, None);
        }
    }
}
