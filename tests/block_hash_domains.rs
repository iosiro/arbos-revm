//! L1 opcode history and L2 ArbSys history must remain distinct.

#[allow(dead_code)]
mod test_utils;

use arbos_revm::{
    ArbitrumEvm, ArbitrumInstructions,
    precompiles::ArbitrumPrecompileProvider,
    state::{ArbState, ArbStateGetter, ArbosStateParams},
};
use revm::{
    ExecuteEvm, InspectEvm,
    context::{ContextTr, JournalTr, TxEnv},
    database::InMemoryDB,
    handler::instructions::{EthInstructions, InstructionProvider},
    inspector::NoOpInspector,
    primitives::{Address, B256, Bytes, TxKind, U256, address, keccak256},
    state::Bytecode,
};

#[test]
fn blockhash_reads_l1_ring_while_arb_sys_reads_l2_database() {
    let caller = Address::repeat_byte(0x11);
    let contract = Address::repeat_byte(0x22);
    let l1_hash = B256::repeat_byte(0x42);
    let l2_hash = B256::repeat_byte(0x77);
    let mut context = test_utils::setup_context().with_db(InMemoryDB::default());
    context.block.number = U256::from(300);
    context.chain.set_rpc_block_number(Some(102));
    context
        .arb_state(None, false)
        .initialize(&ArbosStateParams::default())
        .unwrap();
    context
        .arb_state(None, false)
        .blockhashes()
        .record_new_l1_block(299, l1_hash, 61)
        .unwrap();
    context.journal_mut().load_account(caller).unwrap();
    context
        .journal_mut()
        .balance_incr(caller, U256::from(100_000_000u64))
        .unwrap();
    context.journal_mut().load_account(contract).unwrap();
    // BLOCKHASH(calldataload(0)), returned as a single word.
    context.journal_mut().set_code(
        contract,
        Bytecode::new_raw(Bytes::from_static(&[
            0x5f, 0x35, 0x40, 0x5f, 0x52, 0x60, 0x20, 0x5f, 0xf3,
        ])),
    );
    context
        .journaled_state
        .database
        .cache
        .block_hashes
        .insert(U256::from(299), B256::repeat_byte(0x99));
    context
        .journaled_state
        .database
        .cache
        .block_hashes
        .insert(U256::from(101), l2_hash);
    let spec = context.cfg.inner.spec;
    let instructions = ArbitrumInstructions::new(spec);
    let mut evm = ArbitrumEvm::new_with_inspector(
        context,
        NoOpInspector,
        instructions,
        ArbitrumPrecompileProvider::new(spec),
    );

    let mut fill_input = l1_hash.to_vec();
    fill_input.extend_from_slice(&44u64.to_le_bytes());
    let mut calls = vec![];
    for (number, expected) in [
        (U256::from(299), l1_hash),
        (U256::from(44), keccak256(fill_input)),
        (U256::from(43), B256::ZERO),
        (U256::from(300), B256::ZERO),
        (U256::MAX, B256::ZERO),
    ] {
        calls.push((
            contract,
            Bytes::copy_from_slice(&number.to_be_bytes::<32>()),
            expected,
        ));
    }
    let mut arb_sys_input = keccak256("arbBlockHash(uint256)")[..4].to_vec();
    arb_sys_input.extend_from_slice(&U256::from(101).to_be_bytes::<32>());
    calls.push((
        address!("0000000000000000000000000000000000000064"),
        arb_sys_input.into(),
        l2_hash,
    ));

    for (nonce, (target, data, expected)) in calls.into_iter().enumerate() {
        let tx = TxEnv {
            caller,
            kind: TxKind::Call(target),
            nonce: nonce as u64,
            gas_limit: 1_000_000,
            gas_price: 1,
            data,
            ..Default::default()
        };
        let mut inspected = ArbitrumEvm::new_with_inspector(
            evm.0.ctx.clone(),
            NoOpInspector,
            ArbitrumInstructions::new(spec),
            ArbitrumPrecompileProvider::new(spec),
        );
        let mut ethereum_instructions = ArbitrumEvm::new_with_inspector(
            evm.0.ctx.clone(),
            NoOpInspector,
            EthInstructions::new_mainnet_with_spec(spec),
            ArbitrumPrecompileProvider::new(spec),
        );
        assert_eq!(
            evm.0.instruction.gas_table(),
            ethereum_instructions.0.instruction.gas_table()
        );
        let original_gas = ethereum_instructions
            .transact_one(tx.clone().into())
            .unwrap()
            .tx_gas_used();
        let traced = inspected.inspect_one_tx(tx.clone().into()).unwrap();
        let result = evm.transact_one(tx.into()).unwrap();
        assert_eq!(
            result.tx_gas_used(),
            original_gas,
            "ArbOS history must not add storage gas"
        );
        assert_eq!(result, traced, "inspection must not change output or gas");
        assert!(result.is_success(), "{result:?}");
        assert_eq!(
            result.output().unwrap().as_ref(),
            expected.as_slice(),
            "call {nonce}"
        );
    }
}
