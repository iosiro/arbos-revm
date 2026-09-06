// Copyright 2024, Offchain Labs, Inc.
// For license information, see https://github.com/OffchainLabs/nitro/blob/master/LICENSE.md

//! Functional regressions ported from Nitro's historical-block-hash behavior.

#[allow(dead_code)]
mod test_utils;

use alloy_sol_types::{SolCall, sol};
use arbos_revm::{
    constants::{ARBITRUM_INTERNAL_TX_TYPE, ARBOS_ADDRESS, HISTORY_STORAGE_ADDRESS},
    state::{ArbState, ArbStateGetter, arbos_state::ArbosStateParams, types::StorageBackedTr},
};
use revm::{
    context::{ContextTr, JournalTr, TxEnv, result::ExecutionResult},
    primitives::{Address, Bytes, KECCAK_EMPTY, TxKind, U256},
};
use test_utils::{create_evm, execute_tx, setup_context};

sol! {
    function startBlock(
        uint256 l1BaseFee,
        uint64 l1BlockNumber,
        uint64 l2BlockNumber,
        uint64 timeLastBlock
    );
}

fn assert_history_contract_returns(arbos_version: u64, initialize_by_upgrade: bool) {
    let mut context = setup_context();
    context.block.number = U256::from(5);

    if initialize_by_upgrade {
        context
            .arb_state(None, false)
            .initialize(&ArbosStateParams::for_arbos_version(39))
            .unwrap();
        context
            .arb_state(None, false)
            .upgrade_arbos_version(arbos_version)
            .unwrap();
    } else {
        context
            .arb_state(None, false)
            .initialize(&ArbosStateParams::for_arbos_version(arbos_version))
            .unwrap();
    }

    let history = context
        .journal_mut()
        .load_account(HISTORY_STORAGE_ADDRESS)
        .unwrap();
    assert_eq!(
        history.data.info.nonce, 1,
        "Nitro deploys this system contract with nonce 1"
    );
    assert_ne!(
        history.data.info.code_hash, KECCAK_EMPTY,
        "the history address must contain Nitro's callable EIP-2935 runtime"
    );

    let expected = U256::from_be_slice(&[0x5a; 32]);
    context
        .journal_mut()
        .sstore(HISTORY_STORAGE_ADDRESS, U256::from(4), expected)
        .unwrap();
    context.journal_mut().commit_tx();

    let mut evm = create_evm(context);
    let result = execute_tx(
        &mut evm,
        TxEnv {
            caller: Address::repeat_byte(0x11),
            kind: TxKind::Call(HISTORY_STORAGE_ADDRESS),
            data: Bytes::copy_from_slice(&U256::from(4).to_be_bytes::<32>()),
            gas_limit: 1_000_000,
            ..Default::default()
        },
    );

    let ExecutionResult::Success { output, .. } = result else {
        panic!("history-contract call did not succeed: {result:?}");
    };
    assert_eq!(output.data().as_ref(), expected.to_be_bytes::<32>());
}

#[test]
fn history_contract_is_callable_after_direct_arbos_40_plus_initialization() {
    assert_history_contract_returns(60, false);
}

#[test]
fn history_contract_is_callable_after_crossing_the_arbos_40_upgrade() {
    assert_history_contract_returns(40, true);
}

#[test]
fn legacy_start_block_uses_the_pre_arbos_8_l1_block_number_rule() {
    // This storage field is the next L1 block number. Before ArbOS 8 Nitro
    // increments the input first, so recording input 100 leaves next=101.
    for (arbos_version, expected_l1_block_number) in [(7, 101), (8, 100)] {
        let mut context = setup_context();
        context
            .arb_state(None, false)
            .initialize(&ArbosStateParams::for_arbos_version(arbos_version))
            .unwrap();
        let mut evm = create_evm(context);

        let result = execute_tx(
            &mut evm,
            TxEnv {
                tx_type: ARBITRUM_INTERNAL_TX_TYPE,
                caller: ARBOS_ADDRESS,
                data: startBlockCall {
                    l1BaseFee: U256::ZERO,
                    l1BlockNumber: 100,
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
                .blockhashes()
                .l1_block_number()
                .get()
                .unwrap(),
            expected_l1_block_number,
            "wrong startBlock compatibility behavior at ArbOS {arbos_version}"
        );
    }
}
