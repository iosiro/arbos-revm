// Copyright 2024, Offchain Labs, Inc.
// For license information, see https://github.com/OffchainLabs/nitro/blob/master/LICENSE.md

//! Receipt-level failure and retained-state checks for ArbOS system transactions.

#[allow(dead_code)]
mod test_utils;
use test_utils::{create_evm, execute_tx, setup_context};

use alloy_sol_types::{SolCall, sol};
use arbos_revm::{
    ArbitrumCommittedFailure, ArbitrumTransaction,
    constants::{ARBITRUM_INTERNAL_TX_TYPE, ARBITRUM_SUBMIT_RETRYABLE_TX_TYPE, ARBOS_ADDRESS},
    state::{ArbState, arbos_state::ArbosStateParams},
};
use revm::{
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
        ExecutionResult::Revert { ref gas, .. } if gas.tx_gas_used() == 0
    ));
    assert_eq!(balance(&mut evm, caller), U256::from(100));

    // A MaxSubmissionFee below the computed fee also retains the deposit mint.
    let result = execute_tx(
        &mut evm,
        submit_tx(caller, U256::from(1_000), U256::ZERO, U256::from(100)),
    );
    assert!(matches!(
        result,
        ExecutionResult::Revert { ref gas, .. } if gas.tx_gas_used() == 0
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
        ExecutionResult::Revert { ref gas, .. } if gas.tx_gas_used() == 0
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

    let outcome = evm
        .transact_one_arbitrum(ArbitrumTransaction::from(TxEnv {
            tx_type: ARBITRUM_INTERNAL_TX_TYPE,
            caller: ARBOS_ADDRESS,
            data: Bytes::from_static(&[0xff, 0xff, 0xff, 0xff]),
            ..Default::default()
        }))
        .unwrap();
    assert_eq!(
        outcome.committed_failure,
        Some(ArbitrumCommittedFailure::Internal)
    );
    let result = outcome.result;
    assert!(matches!(
        result,
        ExecutionResult::Revert { ref gas, .. } if gas.tx_gas_used() == 0
    ));

    let recipient = Address::repeat_byte(0x88);
    let result = execute_tx(
        &mut evm,
        TxEnv {
            tx_type: arbos_revm::constants::ARBITRUM_DEPOSIT_TX_TYPE,
            caller: caller_address(),
            kind: TxKind::Call(recipient),
            value: U256::from(7),
            ..Default::default()
        },
    );
    assert!(matches!(result, ExecutionResult::Success { .. }));
    assert_eq!(balance(&mut evm, recipient), U256::from(7));
}

fn caller_address() -> Address {
    Address::repeat_byte(0x99)
}
