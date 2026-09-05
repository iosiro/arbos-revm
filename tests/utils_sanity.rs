// Copyright 2024, Offchain Labs, Inc.
// For license information, see https://github.com/OffchainLabs/nitro/blob/master/LICENSE.md

//! Sanity tests for test utilities - runs once instead of per-file.

mod test_utils;
use test_utils::*;

use alloy_sol_types::{SolCall, SolError, SolEvent, sol};
use arbos_revm::state::l2_pricing::GasConstraint;
use arbos_revm::{
    ArbitrumChainTr,
    constants::{
        ARBITRUM_INTERNAL_TX_TYPE, ARBITRUM_SUBMIT_RETRYABLE_TX_TYPE, ARBOS_ADDRESS,
        ARBOS_STATE_ADDRESS, FILTERED_TRANSACTIONS_STATE_ADDRESS, STYLUS_DISCRIMINANT,
    },
    state::{
        ArbState, ArbStateGetter,
        arbos_state::ArbosStateParams,
        types::{StorageBackedBytes, StorageBackedQueue, StorageBackedTr, map_address, substorage},
    },
    transaction::{arbitrum_retry_tx_hash, arbitrum_submit_retryable_tx_hash},
};

#[test]
fn arbos_initialization_installs_nitro_precompile_placeholder_code() {
    let mut context = setup_context_with_arbos_state();
    let addresses = [
        0x64u64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6b, 0x6c, 0x6d, 0x6e, 0x6f, 0x70, 0x71, 0x72,
        0x73, 0x74, 0xff, 0xa4b05,
    ];

    for raw_address in addresses {
        let address = Address::from_word(U256::from(raw_address).into());
        let code = context.journal_mut().code(address).unwrap();
        assert_eq!(code.data.as_ref(), &[0xfe], "placeholder at {address}");

        let account = context.journal_mut().load_account(address).unwrap();
        assert_eq!(account.data.info.code_hash, keccak256([0xfe]));
        // Geth's SetCode does not increment the account nonce.
        assert_eq!(account.data.info.nonce, 0);
    }
}

#[test]
fn arbos_upgrade_installs_placeholder_at_activation_version() {
    let mut context = setup_context();
    let params = ArbosStateParams {
        arbos_version: 59,
        ..Default::default()
    };
    context.arb_state(None, false).initialize(&params).unwrap();

    let filtered_manager = address!("0000000000000000000000000000000000000074");
    assert!(
        context
            .journal_mut()
            .code(filtered_manager)
            .unwrap()
            .is_empty()
    );

    context
        .arb_state(None, false)
        .upgrade_arbos_version(60)
        .unwrap();
    assert_eq!(
        context
            .journal_mut()
            .code(filtered_manager)
            .unwrap()
            .data
            .as_ref(),
        &[0xfe]
    );
    let account = context
        .journal_mut()
        .load_account(filtered_manager)
        .unwrap();
    assert_eq!(account.data.info.nonce, 0);
}

#[test]
fn storage_backed_queue_uses_nitro_cursor_slots() {
    let mut context = setup_context();
    context
        .journal_mut()
        .load_account(ARBOS_STATE_ADDRESS)
        .unwrap();
    let root = substorage(&B256::ZERO, b"nitro queue layout");
    let put_slot = map_address(&root, &B256::ZERO);
    let get_slot = map_address(&root, &B256::from(U256::ONE));
    let item_2_slot = map_address(&root, &B256::from(U256::from(2)));
    let item_3_slot = map_address(&root, &B256::from(U256::from(3)));

    // Raw state produced by Nitro: next-put at offset 0, next-get at offset 1.
    context
        .journal_mut()
        .sstore(ARBOS_STATE_ADDRESS, put_slot.into(), U256::from(4))
        .unwrap();
    context
        .journal_mut()
        .sstore(ARBOS_STATE_ADDRESS, get_slot.into(), U256::from(2))
        .unwrap();
    context
        .journal_mut()
        .sstore(ARBOS_STATE_ADDRESS, item_2_slot.into(), U256::from(0xaa))
        .unwrap();
    context
        .journal_mut()
        .sstore(ARBOS_STATE_ADDRESS, item_3_slot.into(), U256::from(0xbb))
        .unwrap();

    let mut queue = StorageBackedQueue::new(&mut context, None, false, root);
    assert_eq!(queue.size().unwrap(), 2);
    assert_eq!(queue.peek().unwrap(), Some(U256::from(0xaa)));
    assert_eq!(queue.pop().unwrap(), Some(U256::from(0xaa)));
    queue.push(U256::from(0xcc)).unwrap();

    assert_eq!(
        context
            .journal_mut()
            .sload(ARBOS_STATE_ADDRESS, put_slot.into())
            .unwrap()
            .data,
        U256::from(5)
    );
    assert_eq!(
        context
            .journal_mut()
            .sload(ARBOS_STATE_ADDRESS, get_slot.into())
            .unwrap()
            .data,
        U256::from(3)
    );
}

#[test]
fn storage_backed_bytes_matches_nitro_word_layout() {
    let mut context = setup_context();
    context
        .journal_mut()
        .load_account(ARBOS_STATE_ADDRESS)
        .unwrap();
    let root = substorage(&B256::ZERO, b"nitro bytes layout");
    let size_slot = map_address(&root, &B256::ZERO);
    let word_1_slot = map_address(&root, &B256::from(U256::ONE));
    let word_2_slot = map_address(&root, &B256::from(U256::from(2)));
    let first_word = [0xabu8; 32];
    let partial_word = U256::from_be_slice(&[0x11, 0x22, 0x33]);

    // Raw Nitro representation uses consecutive word offsets and right-aligns
    // the final partial word through common.BytesToHash.
    context
        .journal_mut()
        .sstore(ARBOS_STATE_ADDRESS, size_slot.into(), U256::from(35))
        .unwrap();
    context
        .journal_mut()
        .sstore(
            ARBOS_STATE_ADDRESS,
            word_1_slot.into(),
            U256::from_be_slice(&first_word),
        )
        .unwrap();
    context
        .journal_mut()
        .sstore(ARBOS_STATE_ADDRESS, word_2_slot.into(), partial_word)
        .unwrap();

    let mut expected = first_word.to_vec();
    expected.extend_from_slice(&[0x11, 0x22, 0x33]);
    assert_eq!(
        StorageBackedBytes::new(&mut context, None, false, root)
            .get()
            .unwrap(),
        expected
    );

    // An exact multiple writes Nitro's extra zero trailing word and clears the
    // prior partial word before replacing the value.
    StorageBackedBytes::new(&mut context, None, false, root)
        .set(&first_word)
        .unwrap();
    assert_eq!(
        context
            .journal_mut()
            .sload(ARBOS_STATE_ADDRESS, word_1_slot.into())
            .unwrap()
            .data,
        U256::from_be_slice(&first_word)
    );
    assert_eq!(
        context
            .journal_mut()
            .sload(ARBOS_STATE_ADDRESS, word_2_slot.into())
            .unwrap()
            .data,
        U256::ZERO
    );

    StorageBackedBytes::new(&mut context, None, false, root)
        .set(&[0x44, 0x55, 0x66])
        .unwrap();
    assert_eq!(
        context
            .journal_mut()
            .sload(ARBOS_STATE_ADDRESS, word_1_slot.into())
            .unwrap()
            .data,
        U256::from_be_slice(&[0x44, 0x55, 0x66])
    );
    assert_eq!(
        StorageBackedBytes::new(&mut context, None, true, root)
            .get()
            .unwrap(),
        vec![0x44, 0x55, 0x66]
    );
}

sol! {
    function startBlock(
        uint256 l1BaseFee,
        uint64 l1BlockNumber,
        uint64 l2BlockNumber,
        uint64 timeLastBlock
    );
    function batchPostingReport(
        uint256 batchTimestamp,
        address batchPosterAddress,
        uint64 batchNumber,
        uint64 batchDataGas,
        uint256 l1BaseFeeWei
    );
    function batchPostingReportV2(
        uint256 batchTimestamp,
        address batchPosterAddress,
        uint64 batchNumber,
        uint64 batchCalldataLength,
        uint64 batchCalldataNonZeros,
        uint64 batchExtraGas,
        uint256 l1BaseFeeWei
    );
    function overwriteContractCode(
        address target,
        bytes newCode
    ) returns (bytes oldCode);
    function size(address addr) view returns (uint256);
    function get(address addr, uint256 index) view returns (uint256, bool, uint256);
    function sendTxToL1(address destination, bytes data) payable returns (uint256);
    function burnArbGas(uint256 gasAmount);
    function setChainConfig(string chainConfig);
    function eventsView() view;
    function customRevert(uint64 number) pure;
    error Custom(uint64 number, string message, bool flag);
    function cancel(bytes32 ticketId);
    function redeem(bytes32 ticketId) returns (bytes32);
    event RedeemScheduled(
        bytes32 indexed ticketId,
        bytes32 indexed retryTxHash,
        uint64 indexed sequenceNum,
        uint64 donatedGas,
        address gasDonor,
        uint256 maxRefund,
        uint256 submissionFeeRefund
    );
    function getInfraFeeAccount() view returns (address);
    function getPricesInWei() view returns (
        uint256, uint256, uint256, uint256, uint256, uint256
    );
    function setCalldataPriceIncrease(bool enable);
    function isCalldataPriceIncreaseEnabled() view returns (bool);
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
use revm::{
    context::{ContextTr, JournalTr},
    interpreter::Gas,
    primitives::{Address, B256, Bytes, U256, address, b256, keccak256},
    state::Bytecode,
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

    assert_eq!(params.version, 3);
    assert!(params.ink_price > 0);
    assert_eq!(params.max_stack_depth, 22_000);
    assert_eq!(params.max_wasm_size, 256 * 1024);
    assert_eq!(params.max_fragment_count, 4);

    let mut state = context.arb_state(None, true);
    let mut l1 = state.l1_pricing();
    assert_eq!(
        l1.equilibration_units().get().unwrap(),
        U256::from(160_000_000)
    );
    assert_eq!(l1.inertia().get().unwrap(), 10);
    assert_eq!(l1.per_unit_reward().get().unwrap(), 10);
    assert_eq!(l1.per_batch_gas_cost().get().unwrap(), 210_000);
    assert!(
        l1.batch_poster_table()
            .contains(arbos_revm::constants::ARBOS_BATCH_POSTER_ADDRESS)
            .unwrap()
    );
    let mut l2 = state.l2_pricing();
    assert_eq!(l2.speed_limit_per_second().get().unwrap(), 7_000_000);
    assert_eq!(l2.per_block_gas_limit().get().unwrap(), 32_000_000);
    assert_eq!(l2.base_fee_wei().get().unwrap(), U256::from(100_000_000u64));
    assert_eq!(
        l2.min_base_fee_wei().get().unwrap(),
        U256::from(100_000_000u64)
    );
    assert_eq!(l2.pricing_inertia().get().unwrap(), 102);
    assert_eq!(l2.backlog_tolerance().get().unwrap(), 10);
    assert_eq!(l2.per_tx_gas_limit().get().unwrap(), 32_000_000);
}

#[test]
fn test_address_table_uses_nitro_rlp_and_reports_bytes_consumed() {
    let mut context = setup_context_with_arbos_state();
    let first = address!("1111111111111111111111111111111111111111");
    let second = address!("2222222222222222222222222222222222222222");

    let unknown = context
        .arb_state(None, false)
        .address_table()
        .compress(first)
        .unwrap();
    assert_eq!(unknown, Bytes::from([&[0x94], first.as_slice()].concat()));

    assert_eq!(
        context
            .arb_state(None, false)
            .address_table()
            .register(first)
            .unwrap(),
        0
    );
    assert_eq!(
        context
            .arb_state(None, false)
            .address_table()
            .register(second)
            .unwrap(),
        1
    );
    assert_eq!(
        context
            .arb_state(None, false)
            .address_table()
            .compress(first)
            .unwrap(),
        Bytes::from_static(&[0x80])
    );
    assert_eq!(
        context
            .arb_state(None, false)
            .address_table()
            .compress(second)
            .unwrap(),
        Bytes::from_static(&[0x01])
    );

    let encoded_with_tail = [&[0x94][..], first.as_slice(), &[0xaa, 0xbb]].concat();
    assert_eq!(
        context
            .arb_state(None, false)
            .address_table()
            .decompress(&encoded_with_tail)
            .unwrap(),
        (first, 21)
    );
}

#[test]
fn test_precompile_activation_follows_arbos_version() {
    let caller = Address::repeat_byte(0x42);
    let arb_wasm = address!("0000000000000000000000000000000000000071");

    let execute_at = |version| {
        let mut context = setup_context();
        context.cfg.arbos_version = version;
        context
            .arb_state(None, false)
            .initialize(&ArbosStateParams::for_arbos_version(version))
            .unwrap();
        fund_account(&mut context, caller, U256::from(1_000_000));
        let mut evm = create_evm(context);
        execute_tx(
            &mut evm,
            revm::context::TxEnv {
                caller,
                kind: revm::primitives::TxKind::Call(arb_wasm),
                data: Bytes::new(),
                gas_limit: 100_000,
                gas_price: 1,
                ..Default::default()
            },
        )
    };

    assert!(matches!(
        execute_at(29),
        revm::context::result::ExecutionResult::Success { .. }
    ));
    assert!(matches!(
        execute_at(30),
        revm::context::result::ExecutionResult::Revert { .. }
    ));
}

#[test]
fn test_infra_fee_getter_uses_network_account_before_arbos_6() {
    let network = Address::repeat_byte(0x11);
    let infra = Address::repeat_byte(0x22);
    let caller = Address::repeat_byte(0x42);
    let execute_at = |version| {
        let mut context = setup_context();
        context.cfg.arbos_version = version;
        let mut params = ArbosStateParams::for_arbos_version(version);
        params.network_fee_account = network;
        params.infra_fee_account = infra;
        context.arb_state(None, false).initialize(&params).unwrap();
        fund_account(&mut context, caller, U256::from(1_000_000));
        let mut evm = create_evm(context);
        let result = execute_tx(
            &mut evm,
            revm::context::TxEnv {
                caller,
                kind: revm::primitives::TxKind::Call(address!(
                    "000000000000000000000000000000000000006b"
                )),
                data: getInfraFeeAccountCall {}.abi_encode().into(),
                gas_limit: 100_000,
                gas_price: 1,
                ..Default::default()
            },
        );
        let revm::context::result::ExecutionResult::Success { output, .. } = result else {
            panic!("infra getter failed at ArbOS {version}: {result:?}");
        };
        getInfraFeeAccountCall::abi_decode_returns(output.data()).unwrap()
    };
    assert_eq!(execute_at(5), network);
    assert_eq!(execute_at(6), infra);
}

#[test]
fn test_pre_arbos_4_gas_info_reports_no_congestion_component() {
    let mut context = setup_context();
    context.cfg.arbos_version = 3;
    context.block.basefee = 100;
    context
        .arb_state(None, false)
        .initialize(&ArbosStateParams::for_arbos_version(3))
        .unwrap();
    let caller = Address::repeat_byte(0x42);
    fund_account(&mut context, caller, U256::from(10_000_000));
    let mut evm = create_evm(context);
    let result = execute_tx(
        &mut evm,
        revm::context::TxEnv {
            caller,
            kind: revm::primitives::TxKind::Call(address!(
                "000000000000000000000000000000000000006c"
            )),
            data: getPricesInWeiCall {}.abi_encode().into(),
            gas_limit: 100_000,
            gas_price: 100,
            ..Default::default()
        },
    );
    let revm::context::result::ExecutionResult::Success { output, .. } = result else {
        panic!("gas info failed: {result:?}");
    };
    let words = output.data();
    assert_eq!(U256::from_be_slice(&words[96..128]), U256::from(100));
    assert_eq!(U256::from_be_slice(&words[128..160]), U256::ZERO);
    assert_eq!(U256::from_be_slice(&words[160..192]), U256::from(100));
}

#[test]
fn test_set_calldata_price_increase_updates_feature_without_changing_gas_floor() {
    let mut context = setup_context();
    context.cfg.arbos_version = 40;
    context
        .arb_state(None, false)
        .initialize(&ArbosStateParams::for_arbos_version(40))
        .unwrap();
    let owner = Address::repeat_byte(0x42);
    fund_account(&mut context, owner, U256::from(10_000_000));
    {
        let mut state = context.arb_state(None, false);
        state.chain_owners().add(owner).unwrap();
        state.features().set(U256::from(2)).unwrap();
        state.l1_pricing().gas_floor_per_token().set(37).unwrap();
    }
    let mut evm = create_evm(context);

    let set_enabled = execute_tx(
        &mut evm,
        revm::context::TxEnv {
            caller: owner,
            kind: revm::primitives::TxKind::Call(address!(
                "0000000000000000000000000000000000000070"
            )),
            data: setCalldataPriceIncreaseCall { enable: true }
                .abi_encode()
                .into(),
            gas_limit: 100_000,
            gas_price: 1,
            ..Default::default()
        },
    );
    assert!(matches!(
        set_enabled,
        revm::context::result::ExecutionResult::Success { .. }
    ));

    let getter = execute_tx(
        &mut evm,
        revm::context::TxEnv {
            caller: owner,
            nonce: 1,
            kind: revm::primitives::TxKind::Call(address!(
                "000000000000000000000000000000000000006b"
            )),
            data: isCalldataPriceIncreaseEnabledCall {}.abi_encode().into(),
            gas_limit: 100_000,
            gas_price: 1,
            ..Default::default()
        },
    );
    let revm::context::result::ExecutionResult::Success { output, .. } = getter else {
        panic!("calldata-price-increase getter failed: {getter:?}");
    };
    assert!(isCalldataPriceIncreaseEnabledCall::abi_decode_returns(output.data()).unwrap());
    {
        let mut state = evm.0.ctx.arb_state(None, true);
        assert_eq!(state.features().get().unwrap(), U256::from(3));
        assert_eq!(state.l1_pricing().gas_floor_per_token().get().unwrap(), 37);
    }

    let set_disabled = execute_tx(
        &mut evm,
        revm::context::TxEnv {
            caller: owner,
            nonce: 2,
            kind: revm::primitives::TxKind::Call(address!(
                "0000000000000000000000000000000000000070"
            )),
            data: setCalldataPriceIncreaseCall { enable: false }
                .abi_encode()
                .into(),
            gas_limit: 100_000,
            gas_price: 1,
            ..Default::default()
        },
    );
    assert!(matches!(
        set_disabled,
        revm::context::result::ExecutionResult::Success { .. }
    ));
    let mut state = evm.0.ctx.arb_state(None, true);
    assert_eq!(state.features().get().unwrap(), U256::from(2));
    assert_eq!(state.l1_pricing().gas_floor_per_token().get().unwrap(), 37);
}

#[test]
fn test_debug_overwrite_contract_code_matches_nitro() {
    let mut context = setup_context_with_arbos_state();
    context.cfg.debug_mode = true;
    let caller = Address::repeat_byte(0x42);
    let target = Address::repeat_byte(0x77);
    let old_code = Bytes::from_static(&[0x60, 0x01]);
    let new_code = Bytes::from_static(&[0x60, 0x02, 0x00]);
    fund_account(&mut context, caller, U256::from(2_000_000));
    context.journal_mut().load_account(target).unwrap();
    context
        .journal_mut()
        .set_code(target, Bytecode::new_raw(old_code.clone()));

    let data = overwriteContractCodeCall {
        target,
        newCode: new_code.clone(),
    }
    .abi_encode();
    let mut evm = create_evm(context);
    let result = execute_tx(
        &mut evm,
        revm::context::TxEnv {
            caller,
            kind: revm::primitives::TxKind::Call(address!(
                "00000000000000000000000000000000000000ff"
            )),
            data: data.into(),
            gas_limit: 100_000,
            gas_price: 1,
            ..Default::default()
        },
    );
    let revm::context::result::ExecutionResult::Success { output, .. } = result else {
        panic!("overwriteContractCode did not succeed: {result:?}");
    };
    let returned = overwriteContractCodeCall::abi_decode_returns(output.data()).unwrap();
    assert_eq!(returned, old_code);
    assert_eq!(evm.0.ctx.journal_mut().code(target).unwrap().data, new_code);
}

#[test]
fn test_legacy_precompile_registration_matches_nitro() {
    let provider = arbos_revm::precompiles::ArbitrumPrecompileProvider::<TestContext>::new(
        revm::primitives::hardfork::SpecId::default(),
    );
    let arb_bls = address!("0000000000000000000000000000000000000067");
    let function_table = address!("0000000000000000000000000000000000000068");
    assert!(provider.contains(&arb_bls));
    assert!(provider.contains(&function_table));

    let caller = Address::repeat_byte(0x42);
    let execute = |to, data: Vec<u8>| {
        let mut context = setup_context_with_arbos_state();
        fund_account(&mut context, caller, U256::from(1_000_000));
        let mut evm = create_evm(context);
        execute_tx(
            &mut evm,
            revm::context::TxEnv {
                caller,
                kind: revm::primitives::TxKind::Call(to),
                data: data.into(),
                gas_limit: 100_000,
                gas_price: 1,
                ..Default::default()
            },
        )
    };

    assert!(matches!(
        execute(arb_bls, vec![0, 0, 0, 0]),
        revm::context::result::ExecutionResult::Revert { .. }
    ));

    let result = execute(
        function_table,
        sizeCall {
            addr: Address::ZERO,
        }
        .abi_encode(),
    );
    let revm::context::result::ExecutionResult::Success { output, .. } = result else {
        panic!("function-table size did not succeed: {result:?}");
    };
    assert_eq!(
        sizeCall::abi_decode_returns(output.data()).unwrap(),
        U256::ZERO
    );

    assert!(matches!(
        execute(
            function_table,
            getCall {
                addr: Address::ZERO,
                index: U256::ZERO,
            }
            .abi_encode(),
        ),
        revm::context::result::ExecutionResult::Revert { .. }
    ));
}

#[test]
fn test_spec_change_preserves_arbitrum_precompiles() {
    use revm::handler::PrecompileProvider;

    let arb_sys = address!("0000000000000000000000000000000000000064");
    let mut provider = arbos_revm::precompiles::ArbitrumPrecompileProvider::<TestContext>::new(
        revm::primitives::hardfork::SpecId::CANCUN,
    );
    assert!(provider.contains(&arb_sys));
    assert!(provider.set_spec(revm::primitives::hardfork::SpecId::PRAGUE));
    assert!(provider.contains(&arb_sys));
}

#[test]
fn test_send_merkle_accumulator_matches_nitro_vectors() {
    let mut context = setup_context_with_arbos_state();
    let expected = [
        b256!("b10e2d527612073b26eecdfd717e6a320cf44b4afac2b0732d9fcbe2b7fa0cf6"),
        b256!("50387073e2d4f7060a3c02c3c5268d8a72700a28b5cbd7e23314ae0e1ebda895"),
        b256!("68d4880e8b3b97fc57678c4dce33b2e0d84a06024476eceba62bdc2e07a1e279"),
    ];
    for (index, expected_root) in expected.into_iter().enumerate() {
        let mut item = B256::ZERO;
        item[31] = index as u8 + 1;
        let mut state = context.arb_state(None, false);
        let mut accumulator = state.send_merkle();
        accumulator.append(item).unwrap();
        assert_eq!(accumulator.size().unwrap(), index as u64 + 1);
        assert_eq!(accumulator.root().unwrap(), expected_root);
    }
}

#[test]
fn test_arb_sys_send_to_l1_burns_value_and_appends_send() {
    let mut context = setup_context_with_arbos_state();
    context.block.number = U256::from(12);
    context.block.timestamp = U256::from(34);
    context
        .arb_state(None, false)
        .blockhashes()
        .l1_block_number()
        .set(56)
        .unwrap();
    let caller = Address::repeat_byte(0x42);
    let arb_sys = address!("0000000000000000000000000000000000000064");
    fund_account(&mut context, caller, U256::from(2_000_000));
    let data = sendTxToL1Call {
        destination: Address::repeat_byte(0x99),
        data: Bytes::from_static(b"hello"),
    }
    .abi_encode();
    let mut evm = create_evm(context);
    let result = execute_tx(
        &mut evm,
        revm::context::TxEnv {
            caller,
            kind: revm::primitives::TxKind::Call(arb_sys),
            data: data.into(),
            value: U256::from(123),
            gas_limit: 1_000_000,
            gas_price: 1,
            ..Default::default()
        },
    );
    let revm::context::result::ExecutionResult::Success { output, logs, .. } = result else {
        panic!("sendTxToL1 did not succeed: {result:?}");
    };
    assert_eq!(
        sendTxToL1Call::abi_decode_returns(output.data()).unwrap(),
        U256::ZERO
    );
    assert_eq!(logs.len(), 1);
    assert_eq!(
        evm.0
            .ctx
            .journal_mut()
            .load_account(arb_sys)
            .unwrap()
            .data
            .info
            .balance,
        U256::ZERO
    );
    let mut state = evm.0.ctx.arb_state(None, true);
    assert_eq!(state.send_merkle().size().unwrap(), 1);
}

#[test]
fn arbos_test_burns_requested_gas_and_rejects_non_u64() {
    let caller = Address::repeat_byte(0x42);
    let arbos_test = address!("0000000000000000000000000000000000000069");
    let execute_burn = |amount: U256| {
        let mut context = setup_context_with_arbos_state();
        fund_account(&mut context, caller, U256::from(10_000_000));
        let mut evm = create_evm(context);
        execute_tx(
            &mut evm,
            revm::context::TxEnv {
                caller,
                kind: revm::primitives::TxKind::Call(arbos_test),
                data: burnArbGasCall { gasAmount: amount }.abi_encode().into(),
                gas_limit: 100_000,
                gas_price: 1,
                ..Default::default()
            },
        )
    };

    // Keep ABI calldata byte composition equal so transaction-data pricing
    // cancels out and the delta isolates BurnArbGas itself.
    let baseline = execute_burn(U256::from(40_000));
    let burned = execute_burn(U256::from(50_000));
    assert!(matches!(
        baseline,
        revm::context::result::ExecutionResult::Success { .. }
    ));
    assert!(matches!(
        burned,
        revm::context::result::ExecutionResult::Success { .. }
    ));
    assert_eq!(burned.tx_gas_used() - baseline.tx_gas_used(), 10_000);

    let oversized = execute_burn(U256::from(u64::MAX) + U256::ONE);
    assert!(matches!(
        oversized,
        revm::context::result::ExecutionResult::Halt { ref gas, .. }
            if gas.tx_gas_used() == 100_000
    ));
}

#[test]
fn set_chain_config_dispatch_persists_nitro_bytes_layout() {
    let mut context = setup_context_with_arbos_state();
    let owner = Address::repeat_byte(0x42);
    fund_account(&mut context, owner, U256::from(1_000_000));
    context
        .arb_state(None, false)
        .chain_owners()
        .add(owner)
        .unwrap();
    let config = "x".repeat(100);
    let mut evm = create_evm(context);
    let result = execute_tx(
        &mut evm,
        revm::context::TxEnv {
            caller: owner,
            kind: revm::primitives::TxKind::Call(address!(
                "0000000000000000000000000000000000000070"
            )),
            data: setChainConfigCall {
                chainConfig: config.clone(),
            }
            .abi_encode()
            .into(),
            gas_limit: 500_000,
            gas_price: 1,
            ..Default::default()
        },
    );
    assert!(matches!(
        result,
        revm::context::result::ExecutionResult::Success { .. }
    ));
    assert_eq!(
        evm.0
            .ctx
            .arb_state(None, true)
            .chain_config()
            .get()
            .unwrap(),
        config.as_bytes()
    );
}

#[test]
fn arb_debug_matches_nitro_revert_and_view_semantics() {
    let caller = Address::repeat_byte(0x42);
    let arb_debug = address!("00000000000000000000000000000000000000ff");
    let mut context = setup_context_with_arbos_state();
    context.cfg.debug_mode = true;
    fund_account(&mut context, caller, U256::from(1_000_000));
    let mut evm = create_evm(context);

    let result = execute_tx(
        &mut evm,
        revm::context::TxEnv {
            caller,
            kind: revm::primitives::TxKind::Call(arb_debug),
            data: customRevertCall { number: 7 }.abi_encode().into(),
            gas_limit: 100_000,
            gas_price: 1,
            ..Default::default()
        },
    );
    let revm::context::result::ExecutionResult::Revert { output, .. } = result else {
        panic!("customRevert did not revert: {result:?}");
    };
    let decoded = Custom::abi_decode(&output).unwrap();
    assert_eq!(decoded.number, 7);
    assert_eq!(
        decoded.message,
        "This spider family wards off bugs: /\\oo/\\ //\\(oo)//\\ /\\oo/\\"
    );
    assert!(decoded.flag);

    let events_view = execute_tx(
        &mut evm,
        revm::context::TxEnv {
            caller,
            kind: revm::primitives::TxKind::Call(arb_debug),
            data: eventsViewCall {}.abi_encode().into(),
            gas_limit: 100_000,
            gas_price: 1,
            nonce: 1,
            ..Default::default()
        },
    );
    assert!(!matches!(
        events_view,
        revm::context::result::ExecutionResult::Success { .. }
    ));
}

#[test]
fn test_stylus_params_follow_nitro_version_boundaries() {
    use arbos_revm::state::program::StylusParams;

    let v42 = StylusParams::for_arbos_version(42);
    assert_eq!(v42.version, 2);
    assert_eq!(v42.min_init_gas, 69);
    assert_eq!(v42.max_stack_depth, 4 * 65_536);
    assert_eq!(v42.max_wasm_size, 128 * 1024);
    assert_eq!(v42.max_fragment_count, 0);

    let v50 = StylusParams::for_arbos_version(50);
    assert_eq!(v50.max_stack_depth, 22_000);
    assert_eq!(v50.version, 2);

    let v59 = StylusParams::for_arbos_version(59);
    assert_eq!(v59.version, 3);
    assert_eq!(v59.max_fragment_count, 0);

    let v60 = StylusParams::for_arbos_version(60);
    assert_eq!(v60.max_wasm_size, 256 * 1024);
    assert_eq!(v60.max_fragment_count, 4);
}

#[test]
fn test_activation_gas_uses_nitro_programs_slot_and_version_gate() {
    let mut context = setup_context();
    context.cfg.arbos_version = 58;
    context
        .arb_state(None, false)
        .initialize(&ArbosStateParams::for_arbos_version(58))
        .unwrap();

    context
        .arb_state(None, false)
        .programs()
        .set_activation_gas(5_000_000)
        .unwrap();

    // Nitro masks the persisted value until ArbOS 59 activates.
    assert_eq!(
        context
            .arb_state(None, true)
            .programs()
            .activation_gas()
            .unwrap(),
        0
    );

    context.cfg.arbos_version = 59;
    assert_eq!(
        context
            .arb_state(None, true)
            .programs()
            .activation_gas()
            .unwrap(),
        5_000_000
    );
}

#[test]
fn test_collect_tips_uses_nitro_offset_and_version_gate() {
    let mut context = setup_context();
    context.cfg.arbos_version = 59;
    context
        .arb_state(None, false)
        .initialize(&ArbosStateParams::for_arbos_version(59))
        .unwrap();
    context
        .arb_state(None, false)
        .set_collect_tips(true)
        .unwrap();

    assert!(!context.arb_state(None, true).collect_tips().unwrap());
    context.cfg.arbos_version = 60;
    assert!(context.arb_state(None, true).collect_tips().unwrap());
}

#[test]
fn test_transaction_filtering_uses_nitro_state_layout() {
    let mut context = setup_context();
    context.cfg.arbos_version = 60;
    let network = Address::repeat_byte(0x44);
    let filterer = Address::repeat_byte(0x55);
    let recipient = Address::repeat_byte(0x66);
    let tx_hash = B256::repeat_byte(0x77);
    let mut params = ArbosStateParams::for_arbos_version(60);
    params.network_fee_account = network;
    params.transaction_filtering_enabled_time = 1234;
    context.arb_state(None, false).initialize(&params).unwrap();

    {
        let mut state = context.arb_state(None, false);
        assert_eq!(
            state.transaction_filtering_enabled_time().get().unwrap(),
            1234
        );
        assert_eq!(
            state.filtered_funds_recipient_or_default().unwrap(),
            network
        );
        state.transaction_filterers().add(filterer).unwrap();
        state.filtered_transactions().add(tx_hash).unwrap();
        state.filtered_funds_recipient().set(recipient).unwrap();
    }

    {
        let mut state = context.arb_state(None, true);
        assert!(state.transaction_filterers().contains(filterer).unwrap());
        assert!(state.filtered_transactions().is_filtered(tx_hash).unwrap());
        assert_eq!(
            state.filtered_funds_recipient_or_default().unwrap(),
            recipient
        );
    }

    // Nitro's dedicated KVStorage account still applies storage.mapAddress to
    // the transaction hash.
    let filtered_slot = map_address(&B256::ZERO, &tx_hash);
    assert_eq!(
        context
            .journal_mut()
            .sload(FILTERED_TRANSACTIONS_STATE_ADDRESS, filtered_slot.into())
            .unwrap()
            .data,
        U256::ONE
    );
    assert_eq!(
        context
            .journal_mut()
            .sload(FILTERED_TRANSACTIONS_STATE_ADDRESS, tx_hash.into())
            .unwrap()
            .data,
        U256::ZERO
    );
    assert_eq!(
        context
            .journal_mut()
            .sload(ARBOS_STATE_ADDRESS, tx_hash.into())
            .unwrap()
            .data,
        U256::ZERO
    );

    context
        .arb_state(None, false)
        .filtered_transactions()
        .delete(tx_hash)
        .unwrap();
    assert!(
        !context
            .arb_state(None, true)
            .filtered_transactions()
            .is_filtered(tx_hash)
            .unwrap()
    );
}

#[test]
fn test_legacy_l2_pricing_update_matches_nitro_quartic_model() {
    let mut context = setup_context_with_arbos_state();
    {
        let mut state = context.arb_state(None, false);
        let mut pricing = state.l2_pricing();
        pricing.speed_limit_per_second().set(100).unwrap();
        pricing.gas_backlog().set(1_000).unwrap();
        pricing.backlog_tolerance().set(2).unwrap();
        pricing.pricing_inertia().set(10).unwrap();
        pricing.min_base_fee_wei().set(U256::from(100)).unwrap();
        pricing.update_legacy_pricing_model(3).unwrap();
    }

    let mut state = context.arb_state(None, true);
    let mut pricing = state.l2_pricing();
    assert_eq!(pricing.gas_backlog().get().unwrap(), 700);
    assert_eq!(pricing.base_fee_wei().get().unwrap(), U256::from(164));
}

#[test]
fn test_single_gas_constraints_use_nitro_layout_and_model() {
    let mut context = setup_context();
    context.cfg.arbos_version = 50;
    context
        .arb_state(None, false)
        .initialize(&ArbosStateParams::for_arbos_version(50))
        .unwrap();

    {
        let mut state = context.arb_state(None, false);
        let mut pricing = state.l2_pricing();
        pricing.min_base_fee_wei().set(U256::from(100)).unwrap();
        pricing
            .replace_gas_constraints(&[
                GasConstraint {
                    target: 100,
                    adjustment_window: 10,
                    backlog: 1_000,
                },
                GasConstraint {
                    target: 200,
                    adjustment_window: 20,
                    backlog: 100,
                },
            ])
            .unwrap();
        pricing.grow_backlog(50, 50).unwrap();
        pricing.update_pricing_model(3, 50).unwrap();
    }

    let mut state = context.arb_state(None, true);
    let mut pricing = state.l2_pricing();
    assert_eq!(pricing.gas_constraints_len().unwrap(), 2);
    assert_eq!(
        pricing.gas_constraint(0).unwrap(),
        GasConstraint {
            target: 100,
            adjustment_window: 10,
            backlog: 750,
        }
    );
    assert_eq!(pricing.gas_constraint(1).unwrap().backlog, 0);
    assert_eq!(pricing.base_fee_wei().get().unwrap(), U256::from(211));
}

#[test]
fn test_empty_constraints_fall_back_to_legacy_pricer() {
    let mut context = setup_context();
    context.cfg.arbos_version = 50;
    context
        .arb_state(None, false)
        .initialize(&ArbosStateParams::for_arbos_version(50))
        .unwrap();
    {
        let mut state = context.arb_state(None, false);
        let mut pricing = state.l2_pricing();
        pricing.speed_limit_per_second().set(100).unwrap();
        pricing.gas_backlog().set(1_000).unwrap();
        pricing.backlog_tolerance().set(2).unwrap();
        pricing.pricing_inertia().set(10).unwrap();
        pricing.min_base_fee_wei().set(U256::from(100)).unwrap();
        pricing.replace_gas_constraints(&[]).unwrap();
        pricing.update_pricing_model(3, 50).unwrap();
    }
    assert_eq!(
        context
            .arb_state(None, true)
            .l2_pricing()
            .base_fee_wei()
            .get()
            .unwrap(),
        U256::from(164)
    );
}

#[test]
fn constraint_backlog_update_touches_only_nitro_backlog_word() {
    let mut context = setup_context();
    context.cfg.arbos_version = 55;
    context
        .arb_state(None, false)
        .initialize(&ArbosStateParams::for_arbos_version(55))
        .unwrap();
    let original = GasConstraint {
        target: 123,
        adjustment_window: 456,
        backlog: 10_000,
    };
    context
        .arb_state(None, false)
        .l2_pricing()
        .replace_gas_constraints(&[original])
        .unwrap();

    let mut gas = Gas::new(100_000);
    context
        .arb_state(Some(&mut gas), false)
        .l2_pricing()
        .shrink_backlog(700, 55)
        .unwrap();

    // One length read plus one backlog read/write, matching BacklogUpdateCost.
    assert_eq!(gas.total_gas_spent(), 800 + 800 + 20_000);
    assert_eq!(
        context
            .arb_state(None, true)
            .l2_pricing()
            .gas_constraint(0)
            .unwrap(),
        GasConstraint {
            backlog: 9_300,
            ..original
        }
    );
}

#[test]
fn test_retryable_reaping_consumes_windows_before_deletion() {
    const LIFETIME: u64 = 7 * 24 * 60 * 60;
    let mut context = setup_context_with_arbos_state();
    let id = B256::repeat_byte(0x42);
    {
        let mut state = context.arb_state(None, false);
        let mut retryable = state.retryable(id);
        retryable.timeout().set(1_000).unwrap();
        retryable.timeout_windows_left().set(1).unwrap();
        retryable
            .beneficiary()
            .set(Address::repeat_byte(0x77))
            .unwrap();
        state
            .timeout_queue()
            .push(U256::from_be_slice(id.as_slice()))
            .unwrap();
        state
            .timeout_queue()
            .push(U256::from_be_slice(id.as_slice()))
            .unwrap();
    }

    context
        .arb_state(None, false)
        .retryable_state()
        .try_reap_one(1_001)
        .unwrap();
    {
        let mut state = context.arb_state(None, true);
        assert_eq!(
            state.retryable(id).timeout().get().unwrap(),
            1_000 + LIFETIME
        );
        assert_eq!(state.retryable(id).timeout_windows_left().get().unwrap(), 0);
        assert_eq!(state.timeout_queue().size().unwrap(), 1);
    }

    context
        .arb_state(None, false)
        .retryable_state()
        .try_reap_one(1_001 + LIFETIME)
        .unwrap();
    let mut state = context.arb_state(None, true);
    assert_eq!(state.retryable(id).timeout().get().unwrap(), 0);
    assert_eq!(state.timeout_queue().size().unwrap(), 0);
}

#[test]
fn test_retryable_cancel_emits_canceled_event() {
    let mut context = setup_context_with_arbos_state();
    context.block.timestamp = U256::from(100);
    let caller = Address::repeat_byte(0x42);
    let id = B256::repeat_byte(0x33);
    fund_account(&mut context, caller, U256::from(1_000_000));
    {
        let mut state = context.arb_state(None, false);
        let mut retryable = state.retryable(id);
        retryable.timeout().set(1_000).unwrap();
        retryable.beneficiary().set(caller).unwrap();
    }
    let mut evm = create_evm(context);
    let result = execute_tx(
        &mut evm,
        revm::context::TxEnv {
            caller,
            kind: revm::primitives::TxKind::Call(address!(
                "000000000000000000000000000000000000006e"
            )),
            data: cancelCall { ticketId: id }.abi_encode().into(),
            gas_limit: 100_000,
            gas_price: 1,
            ..Default::default()
        },
    );
    let revm::context::result::ExecutionResult::Success { logs, .. } = result else {
        panic!("retryable cancel did not succeed: {result:?}");
    };
    assert_eq!(logs.len(), 1);
    assert_eq!(
        evm.0
            .ctx
            .arb_state(None, true)
            .retryable(id)
            .timeout()
            .get()
            .unwrap(),
        0
    );
}

#[test]
fn retryable_clear_matches_nitro_raw_state_and_gas() {
    let mut context = setup_context_with_arbos_state();
    let id = B256::repeat_byte(0x91);
    let calldata = Bytes::from(vec![0xabu8; 96]);
    context
        .arb_state(None, false)
        .retryable_state()
        .create_retryable(
            id,
            1_000,
            Address::repeat_byte(0x11),
            None,
            U256::from(12),
            Address::repeat_byte(0x22),
            &calldata,
        )
        .unwrap();

    let ticket_root = substorage(&substorage(&B256::ZERO, &[2]), id.as_slice());
    let target_slot = map_address(&ticket_root, &B256::from(U256::from(2)));
    let calldata_root = substorage(&ticket_root, &[1]);
    let calldata_slots = (0..=3)
        .map(|offset| map_address(&calldata_root, &B256::from(U256::from(offset))))
        .collect::<Vec<_>>();

    assert_eq!(
        context
            .journal_mut()
            .sload(ARBOS_STATE_ADDRESS, target_slot.into())
            .unwrap()
            .data,
        U256::ONE << 255
    );

    let mut gas = Gas::new(100_000);
    context
        .arb_state(Some(&mut gas), false)
        .retryable(id)
        .clear()
        .unwrap();

    // Seven fixed fields, calldata length plus three 32-byte chunks, and the
    // calldata-length SLOAD: 11 resets * 5000 + 800.
    assert_eq!(gas.total_gas_spent(), 55_800);
    assert_eq!(
        context
            .journal_mut()
            .sload(ARBOS_STATE_ADDRESS, target_slot.into())
            .unwrap()
            .data,
        U256::ZERO
    );
    for slot in calldata_slots {
        assert_eq!(
            context
                .journal_mut()
                .sload(ARBOS_STATE_ADDRESS, slot.into())
                .unwrap()
                .data,
            U256::ZERO
        );
    }
}

#[test]
fn retryable_redeem_donates_nitro_gas_and_hashes_it() {
    let mut context = setup_context();
    context.cfg.arbos_version = 59;
    context.block.timestamp = U256::from(100);
    context.block.basefee = 7;
    context
        .arb_state(None, false)
        .initialize(&ArbosStateParams::for_arbos_version(59))
        .unwrap();
    let caller = Address::repeat_byte(0x42);
    let from = Address::repeat_byte(0x11);
    let to = Some(Address::repeat_byte(0x22));
    let id = B256::repeat_byte(0x33);
    fund_account(&mut context, caller, U256::from(10_000_000));
    context
        .arb_state(None, false)
        .retryable_state()
        .create_retryable(id, 1_000, from, to, U256::from(99), caller, &Bytes::new())
        .unwrap();
    let gas_limit = 150_000;
    let mut evm = create_evm(context);
    let result = execute_tx(
        &mut evm,
        revm::context::TxEnv {
            caller,
            kind: revm::primitives::TxKind::Call(address!(
                "000000000000000000000000000000000000006e"
            )),
            data: redeemCall { ticketId: id }.abi_encode().into(),
            gas_limit,
            gas_price: 7,
            ..Default::default()
        },
    );
    let revm::context::result::ExecutionResult::Success { output, logs, .. } = result else {
        panic!("retryable redeem did not succeed: {result:?}");
    };
    let event = RedeemScheduled::decode_log(&logs[0]).unwrap().data;

    // Nitro vector for this transaction. This includes transaction/precompile
    // entry accounting, both OpenRetryable calls, RetryableSizeBytes, MakeTx,
    // the 50-gas-per-word copy charge, event prepayment, and backlog prepayment.
    let expected_donated = 75_569;
    assert_eq!(event.donatedGas, expected_donated);

    let expected_hash = arbitrum_retry_tx_hash(
        U256::from(evm.0.ctx.cfg.chain_id),
        0,
        from,
        U256::from(7),
        expected_donated,
        to,
        U256::from(99),
        &Bytes::new(),
        id,
        caller,
        U256::MAX,
        U256::ZERO,
    );
    assert_eq!(event.retryTxHash, expected_hash);
    assert_eq!(
        redeemCall::abi_decode_returns(output.data()).unwrap(),
        expected_hash
    );
}

#[test]
fn test_submit_retryable_creates_ticket_and_schedules_redeem() {
    let mut context = setup_context();
    let caller = Address::repeat_byte(0x11);
    let network = Address::repeat_byte(0x22);
    let beneficiary = Address::repeat_byte(0x33);
    let refund = Address::repeat_byte(0x44);
    let retry_to = Address::repeat_byte(0x55);
    let params = ArbosStateParams {
        network_fee_account: network,
        chain_id: U256::from(42_161),
        ..Default::default()
    };
    context.arb_state(None, false).initialize(&params).unwrap();
    let data = submitRetryableCall {
        requestId: B256::repeat_byte(0xaa),
        l1BaseFee: U256::from(100),
        deposit: U256::from(1_000_000),
        callvalue: U256::from(99),
        gasFeeCap: U256::from(1),
        gasLimit: 54_321,
        maxSubmissionFee: U256::from(150_000),
        feeRefundAddress: refund,
        beneficiary,
        retryTo: retry_to,
        retryData: Bytes::from_static(&[0xde, 0xad, 0xbe, 0xef]),
    }
    .abi_encode();
    let mut evm = create_evm(context);
    let result = execute_tx(
        &mut evm,
        revm::context::TxEnv {
            tx_type: ARBITRUM_SUBMIT_RETRYABLE_TX_TYPE,
            caller,
            kind: revm::primitives::TxKind::Call(address!(
                "000000000000000000000000000000000000006e"
            )),
            data: data.into(),
            gas_limit: 54_321,
            gas_price: 0,
            ..Default::default()
        },
    );
    let revm::context::result::ExecutionResult::Success { output, logs, .. } = result else {
        panic!("submit retryable did not succeed: {result:?}");
    };
    assert_eq!(logs.len(), 2);
    let ticket_id = B256::from_slice(output.data());
    {
        let mut state = evm.0.ctx.arb_state(None, true);
        let mut retryable = state.retryable(ticket_id);
        assert_eq!(retryable.from().get().unwrap(), caller);
        assert_eq!(retryable.to().get().unwrap(), Some(retry_to));
        assert_eq!(retryable.callvalue().get().unwrap(), U256::from(99));
        assert_eq!(retryable.beneficiary().get().unwrap(), beneficiary);
        assert_eq!(retryable.num_tries().get().unwrap(), 1);
        assert_eq!(state.timeout_queue().size().unwrap(), 1);
    }

    let scheduled = evm.0.ctx.chain().scheduled_retries();
    assert_eq!(scheduled.len(), 1);
    assert_eq!(scheduled[0].ticket_id, ticket_id);
    assert_eq!(
        scheduled[0].hash(),
        B256::from_slice(logs[1].topics()[2].as_slice())
    );

    let retry_result = evm
        .transact_next_scheduled_retry()
        .expect("scheduled retry execution failed")
        .expect("scheduled retry queue was empty");
    assert!(matches!(
        retry_result,
        revm::context::result::ExecutionResult::Success { .. }
    ));
    assert!(evm.0.ctx.chain().scheduled_retries().is_empty());
    assert_eq!(
        evm.0
            .ctx
            .journal_mut()
            .load_account(retry_to)
            .unwrap()
            .data
            .info
            .balance,
        U256::from(99)
    );
    assert_eq!(
        evm.0
            .ctx
            .arb_state(None, true)
            .retryable(ticket_id)
            .timeout()
            .get()
            .unwrap(),
        0
    );
}

#[test]
fn test_filtered_submit_retryable_redirects_refunds_and_skips_redeem() {
    let mut context = setup_context();
    let caller = Address::repeat_byte(0x11);
    let network = Address::repeat_byte(0x22);
    let beneficiary = Address::repeat_byte(0x33);
    let refund = Address::repeat_byte(0x44);
    let recipient = Address::repeat_byte(0x66);
    let retry_to = Address::repeat_byte(0x55);
    let chain_id = U256::from(42_161);
    let mut params = ArbosStateParams::for_arbos_version(60);
    params.network_fee_account = network;
    params.chain_id = chain_id;
    context.cfg.chain_id = chain_id.to::<u64>();
    context.block.basefee = 100;
    context.arb_state(None, false).initialize(&params).unwrap();
    let call = submitRetryableCall {
        requestId: B256::repeat_byte(0xaa),
        l1BaseFee: U256::from(100),
        deposit: U256::from(10_000_000),
        callvalue: U256::from(99),
        gasFeeCap: U256::from(120),
        gasLimit: 54_321,
        maxSubmissionFee: U256::from(150_000),
        feeRefundAddress: refund,
        beneficiary,
        retryTo: retry_to,
        retryData: Bytes::from_static(&[0xde, 0xad, 0xbe, 0xef]),
    };
    let ticket_id = arbitrum_submit_retryable_tx_hash(
        chain_id,
        call.requestId,
        caller,
        call.l1BaseFee,
        call.deposit,
        call.gasFeeCap,
        call.gasLimit,
        Some(call.retryTo),
        call.callvalue,
        call.beneficiary,
        call.maxSubmissionFee,
        call.feeRefundAddress,
        &call.retryData,
    );
    {
        let mut state = context.arb_state(None, false);
        state.filtered_funds_recipient().set(recipient).unwrap();
        state.filtered_transactions().add(ticket_id).unwrap();
    }

    let mut evm = create_evm(context);
    let result = execute_tx(
        &mut evm,
        revm::context::TxEnv {
            tx_type: ARBITRUM_SUBMIT_RETRYABLE_TX_TYPE,
            caller,
            kind: revm::primitives::TxKind::Call(address!(
                "000000000000000000000000000000000000006e"
            )),
            data: call.abi_encode().into(),
            gas_limit: 54_321,
            gas_price: 0,
            ..Default::default()
        },
    );
    let revm::context::result::ExecutionResult::Success {
        output, logs, gas, ..
    } = result
    else {
        panic!("filtered submit retryable did not succeed: {result:?}");
    };
    assert_eq!(B256::from_slice(output.data()), ticket_id);
    assert_eq!(logs.len(), 1);
    assert_eq!(gas.tx_gas_used(), call.gasLimit);
    {
        let mut state = evm.0.ctx.arb_state(None, true);
        let mut retryable = state.retryable(ticket_id);
        assert_eq!(retryable.beneficiary().get().unwrap(), recipient);
        assert_eq!(retryable.num_tries().get().unwrap(), 0);
    }
    assert_eq!(
        evm.0
            .ctx
            .journal_mut()
            .load_account(recipient)
            .unwrap()
            .data
            .info
            .balance,
        U256::from(1_094_020)
    );
    assert_eq!(
        evm.0
            .ctx
            .journal_mut()
            .load_account(network)
            .unwrap()
            .data
            .info
            .balance,
        U256::from(5_574_500)
    );
    assert_eq!(
        evm.0
            .ctx
            .journal_mut()
            .load_account(caller)
            .unwrap()
            .data
            .info
            .balance,
        U256::from(3_331_381)
    );
    assert_eq!(
        evm.0
            .ctx
            .journal_mut()
            .load_account(refund)
            .unwrap()
            .data
            .info
            .balance,
        U256::ZERO
    );
}

#[test]
fn test_scheduled_upgrade_applies_nitro_state_transitions() {
    let mut context = setup_context();
    context
        .arb_state(None, false)
        .initialize(&ArbosStateParams::for_arbos_version(42))
        .unwrap();

    {
        let mut state = context.arb_state(None, false);
        state.upgrade_version().set(60).unwrap();
        state.upgrade_timestamp().set(1_000).unwrap();
        assert!(!state.upgrade_arbos_version_if_necessary(999).unwrap());
        assert!(state.upgrade_arbos_version_if_necessary(1_000).unwrap());
    }

    let mut state = context.arb_state(None, true);
    assert_eq!(state.arbos_version().get().unwrap(), 60);
    assert_eq!(
        state.l2_pricing().per_tx_gas_limit().get().unwrap(),
        32_000_000
    );
    let stylus = state.programs().stylus_params().get().unwrap();
    assert_eq!(stylus.version, 3);
    assert_eq!(stylus.max_stack_depth, 22_000);
    assert_eq!(stylus.max_wasm_size, 256 * 1024);
    assert_eq!(stylus.max_fragment_count, 4);
}

#[test]
fn test_nitro_genesis_owner_and_pricing_defaults() {
    let mut context = setup_context();
    let owner = Address::repeat_byte(0xa1);
    let mut params = ArbosStateParams::for_arbos_version(61);
    params.initial_chain_owner = owner;
    context.arb_state(None, false).initialize(&params).unwrap();

    let mut state = context.arb_state(None, true);
    assert!(state.is_chain_owner(owner).unwrap());
    assert_eq!(state.network_fee_account().get().unwrap(), owner);
    assert_eq!(state.l1_pricing().reward_recipient().get().unwrap(), owner);
    assert_eq!(
        state.l1_pricing().price_per_unit().get().unwrap(),
        U256::from(50_000_000_000u64)
    );
    assert_eq!(state.brotli_compression_level().get().unwrap(), 1);
}

#[test]
fn test_signed_per_batch_gas_cost_round_trips_nitro_encoding() {
    let mut context = setup_context();
    context
        .arb_state(None, false)
        .initialize(&ArbosStateParams::for_arbos_version(61))
        .unwrap();
    let mut state = context.arb_state(None, false);
    state.l1_pricing().per_batch_gas_cost().set(-1_000).unwrap();
    assert_eq!(
        state.l1_pricing().per_batch_gas_cost().get().unwrap(),
        -1_000
    );
}

#[test]
fn test_start_block_internal_tx_applies_scheduled_upgrade() {
    let mut context = setup_context();
    context
        .arb_state(None, false)
        .initialize(&ArbosStateParams::for_arbos_version(42))
        .unwrap();
    context.block.number = U256::from(10);
    context.block.timestamp = U256::from(1_000);
    {
        let mut state = context.arb_state(None, false);
        state.upgrade_version().set(50).unwrap();
        state.upgrade_timestamp().set(1_000).unwrap();
    }

    let data = startBlockCall {
        l1BaseFee: U256::ZERO,
        l1BlockNumber: 100,
        l2BlockNumber: 10,
        timeLastBlock: 1,
    }
    .abi_encode();
    let tx = revm::context::TxEnv {
        tx_type: ARBITRUM_INTERNAL_TX_TYPE,
        caller: ARBOS_ADDRESS,
        kind: revm::primitives::TxKind::Call(ARBOS_STATE_ADDRESS),
        data: data.into(),
        gas_limit: 0,
        ..Default::default()
    };
    let mut evm = create_evm(context);
    assert!(matches!(
        execute_tx(&mut evm, tx),
        revm::context::result::ExecutionResult::Success { .. }
    ));

    {
        let mut state = evm.0.ctx.arb_state(None, true);
        assert_eq!(state.arbos_version().get().unwrap(), 50);
        assert_eq!(state.blockhashes().l1_block_number().get().unwrap(), 100);
    }
    assert_eq!(evm.0.ctx.cfg.arbos_version, 50);
}

fn execute_internal_call(context: test_utils::TestContext, data: Vec<u8>) -> test_utils::TestEvm {
    let tx = revm::context::TxEnv {
        tx_type: ARBITRUM_INTERNAL_TX_TYPE,
        caller: ARBOS_ADDRESS,
        kind: revm::primitives::TxKind::Call(ARBOS_STATE_ADDRESS),
        data: data.into(),
        gas_limit: 0,
        ..Default::default()
    };
    let mut evm = create_evm(context);
    assert!(matches!(
        execute_tx(&mut evm, tx),
        revm::context::result::ExecutionResult::Success { .. }
    ));
    evm
}

#[test]
fn test_batch_posting_report_updates_nitro_l1_ledger() {
    let mut context = setup_context();
    context.cfg.arbos_version = 60;
    context.block.timestamp = U256::from(200);
    context
        .arb_state(None, false)
        .initialize(&ArbosStateParams::for_arbos_version(60))
        .unwrap();
    {
        let mut state = context.arb_state(None, false);
        let mut pricing = state.l1_pricing();
        pricing.last_update_time().set(100).unwrap();
        pricing.units_since_update().set(100).unwrap();
        pricing.per_unit_reward().set(10).unwrap();
        pricing.per_batch_gas_cost().set(100).unwrap();
        pricing
            .equilibration_units()
            .set(U256::from(1_000))
            .unwrap();
        pricing.inertia().set(10).unwrap();
        pricing.price_per_unit().set(U256::from(100)).unwrap();
    }
    let poster = Address::repeat_byte(0x88);
    let data = batchPostingReportCall {
        batchTimestamp: U256::from(150),
        batchPosterAddress: poster,
        batchNumber: 7,
        batchDataGas: 200,
        l1BaseFeeWei: U256::from(2),
    }
    .abi_encode();
    let mut evm = execute_internal_call(context, data);
    {
        let mut state = evm.0.ctx.arb_state(None, true);
        let mut pricing = state.l1_pricing();
        assert_eq!(pricing.last_update_time().get().unwrap(), 150);
        assert_eq!(pricing.units_since_update().get().unwrap(), 50);
        assert_eq!(
            pricing
                .batch_poster_table()
                .get(poster)
                .funds_due()
                .get()
                .unwrap(),
            U256::from(600)
        );
        assert_eq!(
            pricing.funds_due_for_rewards().get().unwrap(),
            revm::primitives::I256::try_from(500).unwrap()
        );
        assert_eq!(
            pricing.last_surplus().get().unwrap(),
            revm::primitives::I256::try_from(-1_100).unwrap()
        );
        assert_eq!(pricing.price_per_unit().get().unwrap(), U256::from(107));
    }

    // A second report in the same backend instance must consume the first
    // report's controller state rather than reinitializing transaction-local
    // pricing data.
    let second = batchPostingReportCall {
        batchTimestamp: U256::from(175),
        batchPosterAddress: poster,
        batchNumber: 8,
        batchDataGas: 300,
        l1BaseFeeWei: U256::from(3),
    }
    .abi_encode();
    let second_result = execute_tx(
        &mut evm,
        revm::context::TxEnv {
            tx_type: ARBITRUM_INTERNAL_TX_TYPE,
            caller: ARBOS_ADDRESS,
            kind: revm::primitives::TxKind::Call(ARBOS_STATE_ADDRESS),
            data: second.into(),
            gas_limit: 0,
            ..Default::default()
        },
    );
    assert!(matches!(
        second_result,
        revm::context::result::ExecutionResult::Success { .. }
    ));
    let mut state = evm.0.ctx.arb_state(None, true);
    let mut pricing = state.l1_pricing();
    assert_eq!(pricing.last_update_time().get().unwrap(), 175);
    assert_eq!(
        pricing
            .batch_poster_table()
            .get(poster)
            .funds_due()
            .get()
            .unwrap(),
        U256::from(1_800)
    );
    assert_eq!(pricing.units_since_update().get().unwrap(), 25);
}

#[test]
fn test_batch_posting_report_v2_applies_arbos50_floor() {
    let mut context = setup_context();
    context.cfg.arbos_version = 60;
    context.block.timestamp = U256::from(200);
    context
        .arb_state(None, false)
        .initialize(&ArbosStateParams::for_arbos_version(60))
        .unwrap();
    {
        let mut state = context.arb_state(None, false);
        let mut pricing = state.l1_pricing();
        pricing.last_update_time().set(100).unwrap();
        pricing.per_batch_gas_cost().set(100).unwrap();
        pricing.gas_floor_per_token().set(2).unwrap();
    }
    let poster = Address::repeat_byte(0x99);
    let data = batchPostingReportV2Call {
        batchTimestamp: U256::from(150),
        batchPosterAddress: poster,
        batchNumber: 8,
        batchCalldataLength: 10,
        batchCalldataNonZeros: 2,
        batchExtraGas: 3,
        l1BaseFeeWei: U256::from(3),
    }
    .abi_encode();
    let mut evm = execute_internal_call(context, data);
    let due = evm
        .0
        .ctx
        .arb_state(None, true)
        .l1_pricing()
        .batch_poster_table()
        .get(poster)
        .funds_due()
        .get()
        .unwrap();
    assert_eq!(due, U256::from(120_609));
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
