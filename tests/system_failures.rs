// Copyright 2024, Offchain Labs, Inc.
// For license information, see https://github.com/OffchainLabs/nitro/blob/master/LICENSE.md

//! Receipt-level failure and retained-state checks for ArbOS system transactions.

#[allow(dead_code)]
mod test_utils;
use test_utils::{create_evm, execute_tx, setup_context};

use alloy_sol_types::{SolCall, sol};
use arbos_revm::{
    constants::{
        ARBITRUM_INTERNAL_TX_TYPE, ARBITRUM_SUBMIT_RETRYABLE_TX_TYPE, ARBOS_ADDRESS,
        ARBOS_L1_PRICER_FUNDS_ADDRESS, HISTORY_STORAGE_ADDRESS, HISTORY_STORAGE_CODE_ARBITRUM,
    },
    state::{ArbState, ArbStateGetter, arbos_state::ArbosStateParams, types::StorageBackedTr},
    transaction::ArbitrumTransaction,
};
use revm::{
    ExecuteEvm,
    context::{ContextTr, Host, JournalTr, TxEnv, result::ExecutionResult},
    primitives::{Address, B256, Bytes, TxKind, U256},
};

sol! {
    function startBlock(
        uint256 l1BaseFee,
        uint64 l1BlockNumber,
        uint64 l2BlockNumber,
        uint64 timeLastBlock
    );

    function submitRetryable(
        bytes32 requestId,
        uint256 l1BaseFee,
        uint256 deposit,
        uint256 callvalue,
        uint256 gasFeeCap,
        uint64 gasLimit,
        uint256 maxSubmissionFee,
        address feeRefundAddress,
        address beneficiary,
        address retryTo,
        bytes retryData
    );
}

#[test]
fn start_block_records_parent_hash_in_eip2935_history_ring() {
    let mut context = setup_context();
    context.block.number = U256::from(5);
    context
        .arb_state(None, false)
        .initialize(&ArbosStateParams::for_arbos_version(60))
        .unwrap();
    context
        .journal_mut()
        .load_account(arbos_revm::constants::HISTORY_STORAGE_ADDRESS)
        .unwrap();
    context
        .journal_mut()
        .sstore(
            arbos_revm::constants::HISTORY_STORAGE_ADDRESS,
            U256::from(4),
            U256::from(123),
        )
        .unwrap();
    context.journal_mut().commit_tx();
    let expected_parent_hash = context.block_hash(4).unwrap_or_default();
    let mut evm = create_evm(context);
    let result = execute_tx(
        &mut evm,
        TxEnv {
            tx_type: ARBITRUM_INTERNAL_TX_TYPE,
            caller: ARBOS_ADDRESS,
            data: startBlockCall {
                l1BaseFee: U256::ZERO,
                l1BlockNumber: 0,
                l2BlockNumber: 5,
                timeLastBlock: 0,
            }
            .abi_encode()
            .into(),
            ..Default::default()
        },
    );
    assert!(matches!(result, ExecutionResult::Success { .. }));
    assert_eq!(
        evm.0
            .ctx
            .journal_mut()
            .sload(
                arbos_revm::constants::HISTORY_STORAGE_ADDRESS,
                U256::from(4)
            )
            .unwrap()
            .data,
        U256::from_be_bytes(expected_parent_hash.0)
    );

    let result = execute_tx(
        &mut evm,
        TxEnv {
            caller: Address::repeat_byte(0x71),
            kind: TxKind::Call(HISTORY_STORAGE_ADDRESS),
            data: Bytes::copy_from_slice(&U256::from(4).to_be_bytes::<32>()),
            gas_limit: 100_000,
            ..Default::default()
        },
    );
    match result {
        ExecutionResult::Success { output, .. } => {
            assert_eq!(output.data().as_ref(), expected_parent_hash.as_slice());
        }
        other => panic!("history-contract call failed: {other:?}"),
    }
}

#[test]
fn arbos_40_installs_callable_history_contract_on_initialization_and_upgrade() {
    for initial_version in [40, 60] {
        let mut context = setup_context();
        context
            .arb_state(None, false)
            .initialize(&ArbosStateParams::for_arbos_version(initial_version))
            .unwrap();
        let account = context
            .journal_mut()
            .load_account(HISTORY_STORAGE_ADDRESS)
            .unwrap();
        assert_eq!(account.data.info.nonce, 1);
        assert_eq!(
            account.data.info.code.as_ref().unwrap().original_bytes(),
            Bytes::from_static(HISTORY_STORAGE_CODE_ARBITRUM)
        );
    }

    let mut context = setup_context();
    context
        .arb_state(None, false)
        .initialize(&ArbosStateParams::for_arbos_version(39))
        .unwrap();
    context
        .arb_state(None, false)
        .upgrade_arbos_version(40)
        .unwrap();
    let account = context
        .journal_mut()
        .load_account(HISTORY_STORAGE_ADDRESS)
        .unwrap();
    assert_eq!(account.data.info.nonce, 1);
    assert_eq!(
        account.data.info.code.as_ref().unwrap().original_bytes(),
        Bytes::from_static(HISTORY_STORAGE_CODE_ARBITRUM)
    );
}

#[test]
fn scheduled_upgrades_apply_nitro_pricing_owner_and_brotli_migrations() {
    let owner = Address::repeat_byte(0x42);
    let mut context = setup_context();
    let mut params = ArbosStateParams::for_arbos_version(9);
    params.initial_chain_owner = owner;
    context.arb_state(None, false).initialize(&params).unwrap();
    context
        .journal_mut()
        .load_account(ARBOS_L1_PRICER_FUNDS_ADDRESS)
        .unwrap();
    context
        .journal_mut()
        .balance_incr(ARBOS_L1_PRICER_FUNDS_ADDRESS, U256::from(1234))
        .unwrap();
    context
        .arb_state(None, false)
        .upgrade_arbos_version(11)
        .unwrap();
    let mut state = context.arb_state(None, false);
    assert_eq!(
        state.l1_pricing().l1_fees_available().get().unwrap(),
        U256::from(1234)
    );
    assert_eq!(
        state.l1_pricing().per_batch_gas_cost().get().unwrap(),
        210_000
    );
    assert_eq!(
        state.l1_pricing().amortized_cost_cap_bips().get().unwrap(),
        0
    );
    assert!(state.chain_owners().all().unwrap().is_empty());

    let mut context = setup_context();
    context
        .arb_state(None, false)
        .initialize(&ArbosStateParams::for_arbos_version(19))
        .unwrap();
    context
        .arb_state(None, false)
        .upgrade_arbos_version(20)
        .unwrap();
    assert_eq!(
        context
            .arb_state(None, false)
            .brotli_compression_level()
            .get()
            .unwrap(),
        1
    );
}

#[test]
fn historical_start_block_version_gates_match_nitro() {
    let mut context = setup_context();
    context.cfg.arbos_version = 2;
    context
        .arb_state(None, false)
        .initialize(&ArbosStateParams::for_arbos_version(2))
        .unwrap();
    context
        .arb_state(None, false)
        .l2_pricing()
        .speed_limit_per_second()
        .set(1)
        .unwrap();
    context
        .arb_state(None, false)
        .l2_pricing()
        .gas_backlog()
        .set(100)
        .unwrap();
    let mut evm = create_evm(context);
    let result = execute_tx(
        &mut evm,
        TxEnv {
            tx_type: ARBITRUM_INTERNAL_TX_TYPE,
            caller: ARBOS_ADDRESS,
            data: startBlockCall {
                l1BaseFee: U256::ZERO,
                l1BlockNumber: 5,
                l2BlockNumber: 10,
                timeLastBlock: 1,
            }
            .abi_encode()
            .into(),
            ..Default::default()
        },
    );
    assert!(matches!(result, ExecutionResult::Success { .. }));
    assert_eq!(
        evm.0
            .ctx
            .arb_state(None, false)
            .l2_pricing()
            .gas_backlog()
            .get()
            .unwrap(),
        90
    );
    assert_eq!(
        evm.0
            .ctx
            .arb_state(None, false)
            .blockhashes()
            .l1_block_number()
            .get()
            .unwrap(),
        6
    );
}

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

fn submit_tx(caller: Address, deposit: U256, callvalue: U256, max_submission_fee: U256) -> TxEnv {
    TxEnv {
        tx_type: ARBITRUM_SUBMIT_RETRYABLE_TX_TYPE,
        caller,
        kind: TxKind::Call(Address::from_word(U256::from(0x6e).into())),
        data: submitRetryableCall {
            requestId: B256::repeat_byte(0xaa),
            l1BaseFee: U256::from(100),
            deposit,
            callvalue,
            gasFeeCap: U256::from(100),
            gasLimit: 54_321,
            maxSubmissionFee: max_submission_fee,
            feeRefundAddress: Address::repeat_byte(0x44),
            beneficiary: Address::repeat_byte(0x33),
            retryTo: Address::repeat_byte(0x55),
            retryData: Bytes::from_static(&[0xde, 0xad, 0xbe, 0xef]),
        }
        .abi_encode()
        .into(),
        gas_limit: 54_321,
        ..Default::default()
    }
}

#[test]
fn submit_retryable_early_failures_retain_nitro_state_and_isolate_next_tx() {
    let caller = Address::repeat_byte(0x11);
    let network = Address::repeat_byte(0x22);
    let recipient = Address::repeat_byte(0x77);
    let mut context = setup_context();
    let mut params = ArbosStateParams::for_arbos_version(60);
    params.network_fee_account = network;
    context.arb_state(None, false).initialize(&params).unwrap();
    let mut evm = create_evm(context);

    // Nitro mints the deposit before checking whether MaxSubmissionFee can be paid.
    let result = execute_tx(
        &mut evm,
        submit_tx(caller, U256::from(100), U256::ZERO, U256::from(150)),
    );
    assert!(matches!(
        result,
        ExecutionResult::Revert { gas_used: 0, .. }
    ));
    assert_eq!(balance(&mut evm, caller), U256::from(100));

    // A MaxSubmissionFee below the computed fee also retains the deposit mint.
    let result = execute_tx(
        &mut evm,
        submit_tx(caller, U256::from(1_000), U256::ZERO, U256::from(100)),
    );
    assert!(matches!(
        result,
        ExecutionResult::Revert { gas_used: 0, .. }
    ));
    assert_eq!(balance(&mut evm, caller), U256::from(1_100));

    // A subsequent ordinary transaction must not inherit an open system journal.
    let result = execute_tx(
        &mut evm,
        TxEnv {
            caller,
            kind: TxKind::Call(recipient),
            value: U256::from(1),
            gas_limit: 21_000,
            ..Default::default()
        },
    );
    assert!(matches!(result, ExecutionResult::Success { .. }));
    assert_eq!(balance(&mut evm, recipient), U256::from(1));
}

#[test]
fn submit_retryable_callvalue_failure_compensates_before_failed_receipt() {
    let caller = Address::repeat_byte(0x11);
    let network = Address::repeat_byte(0x22);
    let refund = Address::repeat_byte(0x44);
    let mut context = setup_context();
    let mut params = ArbosStateParams::for_arbos_version(60);
    params.network_fee_account = network;
    context.arb_state(None, false).initialize(&params).unwrap();
    let mut evm = create_evm(context);

    let result = execute_tx(
        &mut evm,
        submit_tx(
            caller,
            U256::from(150_000),
            U256::from(200_000),
            U256::from(150_000),
        ),
    );
    assert!(matches!(
        result,
        ExecutionResult::Revert { gas_used: 0, .. }
    ));
    assert_eq!(balance(&mut evm, network), U256::ZERO);
    assert_eq!(balance(&mut evm, caller), U256::from(150_000));
    assert_eq!(balance(&mut evm, refund), U256::ZERO);
}

#[test]
fn malformed_internal_failure_is_receipt_level_and_does_not_dirty_next_tx() {
    let mut context = setup_context();
    context
        .arb_state(None, false)
        .initialize(&ArbosStateParams::for_arbos_version(60))
        .unwrap();
    let mut evm = create_evm(context);

    let result = execute_tx(
        &mut evm,
        TxEnv {
            tx_type: ARBITRUM_INTERNAL_TX_TYPE,
            caller: ARBOS_ADDRESS,
            data: Bytes::from_static(&[0xff, 0xff, 0xff, 0xff]),
            ..Default::default()
        },
    );
    assert!(matches!(
        result,
        ExecutionResult::Revert { gas_used: 0, .. }
    ));

    let recipient = Address::repeat_byte(0x88);
    let result = evm
        .transact_one(
            ArbitrumTransaction::from(TxEnv {
                tx_type: arbos_revm::constants::ARBITRUM_DEPOSIT_TX_TYPE,
                caller: caller_address(),
                kind: TxKind::Call(recipient),
                value: U256::from(7),
                ..Default::default()
            })
            .with_tx_hash(B256::repeat_byte(0x77)),
        )
        .expect("deposit execution failed");
    assert!(matches!(result, ExecutionResult::Success { .. }));
    assert_eq!(balance(&mut evm, recipient), U256::from(7));
}

fn caller_address() -> Address {
    Address::repeat_byte(0x99)
}
