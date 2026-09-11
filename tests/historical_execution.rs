//! Persisted ArbOS versions select opcode and transaction gas semantics.

#[allow(dead_code)]
mod test_utils;

use arbos_revm::state::{ArbState, ArbStateGetter, ArbosStateParams, types::StorageBackedTr};
use revm::{
    ExecuteEvm, InspectEvm,
    context::{ContextTr, JournalTr, TxEnv},
    primitives::{Address, Bytes, TxKind, U256, hardfork::SpecId},
    state::Bytecode,
};
use test_utils::{create_evm, execute_tx, fund_account, setup_context};

#[test]
fn ethereum_frame_sync_does_not_load_arbos_state() {
    let mut evm = create_evm(setup_context());
    assert!(evm.0.ctx.journal_mut().state().is_empty());
    evm.sync_execution_spec().unwrap();
    assert!(evm.0.ctx.journal_mut().state().is_empty());
    assert!(!evm.0.ctx.chain().arbos_initialized);
}

#[test]
fn persisted_arbos_version_selects_calldata_floor_and_osaka_instructions() {
    let caller = Address::repeat_byte(0x11);
    let stop_contract = Address::repeat_byte(0x22);
    let clz_contract = Address::repeat_byte(0x33);
    let mut evm = create_evm(setup_context());
    for (version, expected_gas, clz_enabled) in [
        (10, 21_000 + 16 * 1024, false),
        (11, 21_000 + 16 * 1024, false),
        (20, 21_000 + 16 * 1024, false),
        (40, 21_000 + 40 * 1024, false),
        (50, 21_000 + 40 * 1024, true),
        (61, 21_000 + 40 * 1024, true),
        (20, 21_000 + 16 * 1024, false),
    ] {
        let mut context = setup_context();
        // A stale newer spec must not enable opcode or gas rules before their ArbOS version.
        context.cfg.inner.spec = SpecId::OSAKA;
        context
            .arb_state(None, false)
            .initialize(&ArbosStateParams::for_arbos_version(version))
            .unwrap();
        fund_account(&mut context, caller, U256::from(100_000_000u64));
        context.journal_mut().load_account(stop_contract).unwrap();
        context.journal_mut().set_code(
            stop_contract,
            Bytecode::new_raw(Bytes::from_static(&[0x00])),
        );
        context.journal_mut().load_account(clz_contract).unwrap();
        // CLZ(0) returns 256; the opcode is unavailable before Osaka.
        context.journal_mut().set_code(
            clz_contract,
            Bytecode::new_raw(Bytes::from_static(&[
                0x60, 0x00, 0x1e, 0x60, 0x00, 0x52, 0x60, 0x20, 0x60, 0x00, 0xf3,
            ])),
        );
        // Retain the instruction/precompile providers across state replacement, as in a reused EVM.
        context
            .journal_mut()
            .warm_precompiles(evm.0.ctx.journal().precompile_addresses().clone());
        evm.0.ctx = context;
        let result = execute_tx(
            &mut evm,
            TxEnv {
                caller,
                kind: TxKind::Call(stop_contract),
                gas_limit: 1_000_000,
                gas_price: 1,
                data: Bytes::from(vec![1; 1024]),
                ..Default::default()
            },
        );
        assert!(result.is_success(), "ArbOS {version}: {result:?}");
        assert_eq!(
            result.gas_used(),
            expected_gas,
            "ArbOS {version} calldata pricing"
        );
        assert_eq!(
            evm.0
                .ctx
                .journal()
                .precompile_addresses()
                .contains(&Address::with_last_byte(0x0b)),
            version >= 40,
            "ArbOS {version} BLS precompile warmth"
        );
        let result = execute_tx(
            &mut evm,
            TxEnv {
                caller,
                kind: TxKind::Call(clz_contract),
                nonce: 1,
                gas_limit: 1_000_000,
                gas_price: 1,
                ..Default::default()
            },
        );
        assert_eq!(
            result.is_success(),
            clz_enabled,
            "ArbOS {version}: {result:?}"
        );
        if clz_enabled {
            assert_eq!(
                result.output().unwrap().as_ref(),
                &U256::from(256).to_be_bytes::<32>()
            );
        }
    }
}

#[test]
fn unknown_persisted_arbos_version_is_rejected_before_execution() {
    let mut context = setup_context();
    context
        .arb_state(None, false)
        .initialize(&ArbosStateParams::default())
        .unwrap();
    context
        .arb_state(None, false)
        .arbos_version()
        .set(62)
        .unwrap();
    let mut plain = create_evm(context.clone());
    let mut inspected = create_evm(context);
    assert_eq!(
        plain
            .transact_one(TxEnv::default().into())
            .unwrap_err()
            .to_string(),
        "unsupported ArbOS version 62",
    );
    assert_eq!(
        inspected
            .inspect_one_tx(TxEnv::default().into())
            .unwrap_err()
            .to_string(),
        "unsupported ArbOS version 62",
    );
}

#[test]
fn blob_basefee_halts_even_when_the_ethereum_spec_supports_it() {
    let caller = Address::repeat_byte(0x11);
    let contract = Address::repeat_byte(0x22);
    for version in [19, 20, 40, 50, 61] {
        let mut context = setup_context();
        context
            .arb_state(None, false)
            .initialize(&ArbosStateParams::for_arbos_version(version))
            .unwrap();
        fund_account(&mut context, caller, U256::from(100_000_000u64));
        context.journal_mut().load_account(contract).unwrap();
        // Return BLOBBASEFEE. Nitro rejects this opcode even after Cancun activation.
        context.journal_mut().set_code(
            contract,
            Bytecode::new_raw(Bytes::from_static(&[
                0x4a, 0x60, 0x00, 0x52, 0x60, 0x20, 0x60, 0x00, 0xf3,
            ])),
        );
        let result = execute_tx(
            &mut create_evm(context),
            TxEnv {
                caller,
                kind: TxKind::Call(contract),
                gas_limit: 100_000,
                gas_price: 1,
                ..Default::default()
            },
        );
        assert!(result.is_halt(), "ArbOS {version}: {result:?}");
        assert_eq!(
            result.gas_used(),
            100_000,
            "the exceptional halt burns all gas"
        );
    }
}
