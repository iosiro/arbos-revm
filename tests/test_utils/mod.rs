// Copyright 2024, Offchain Labs, Inc.
// For license information, see https://github.com/OffchainLabs/nitro/blob/master/LICENSE.md

//! Test utilities for Stylus integration tests with ArbitrumEvm.

use std::convert::Infallible;

use arbos_revm::{
    ArbitrumContext, ArbitrumEvm, ArbitrumTransaction,
    config::ArbitrumConfig,
    constants::STYLUS_DISCRIMINANT,
    local_context::ArbitrumLocalContext,
    precompiles::ArbitrumPrecompileProvider,
    state::{ArbState, arbos_state::ArbosStateParams},
};
use revm::{
    ExecuteEvm, Journal,
    context::{BlockEnv, ContextTr, JournalTr, TxEnv, result::ExecutionResult},
    database::EmptyDBTyped,
    handler::instructions::EthInstructions,
    inspector::NoOpInspector,
    primitives::{Address, Bytes, TxKind, U256, keccak256},
    state::Bytecode,
};
use stylus::brotli;

/// Type alias for the test database
pub type TestDb = EmptyDBTyped<Infallible>;

/// Type alias for the test context
pub type TestContext = ArbitrumContext<TestDb>;

/// Type alias for the test EVM
#[allow(dead_code)]
pub type TestEvm = ArbitrumEvm<
    TestContext,
    NoOpInspector,
    ArbitrumPrecompileProvider<TestContext>,
    EthInstructions<revm::interpreter::interpreter::EthInterpreter, TestContext>,
>;

/// Setup a test context with an empty database
pub fn setup_context() -> TestContext {
    let db = TestDb::default();

    ArbitrumContext {
        journaled_state: Journal::new(db),
        block: BlockEnv::default(),
        cfg: ArbitrumConfig::default(),
        tx: ArbitrumTransaction::default(),
        chain: (),
        local: ArbitrumLocalContext::default(),
        error: Ok(()),
    }
}

/// Setup a test context with ArbOS state initialized
pub fn setup_context_with_arbos_state() -> TestContext {
    let mut context = setup_context();

    // Initialize ArbOS state with default parameters
    context
        .arb_state(None, false)
        .initialize(&ArbosStateParams::default())
        .expect("failed to initialize ArbOS state");

    context
}

/// Create a test ArbitrumEvm instance
#[allow(dead_code)]
pub fn create_evm(context: TestContext) -> TestEvm {
    ArbitrumEvm::new_with_inspector(
        context,
        NoOpInspector {},
        EthInstructions::default(),
        ArbitrumPrecompileProvider::default(),
    )
}

/// Deploy a WAT program to the test context and return its address
pub fn deploy_wat_program(context: &mut TestContext, wat: &[u8]) -> Address {
    let wasm_bytes = wasmer::wat2wasm(wat).expect("failed to compile WAT");
    deploy_wasm_program(context, &wasm_bytes)
}

/// Deploy a WASM program to the test context and return its address
pub fn deploy_wasm_program(context: &mut TestContext, wasm: &[u8]) -> Address {
    // Compress with brotli using empty dictionary
    let compressed = brotli::compress(wasm, 11, 22, brotli::Dictionary::Empty)
        .expect("brotli compression failed");

    // Build the Stylus bytecode format: discriminant + dictionary byte + compressed wasm
    let mut bytecode = Vec::with_capacity(STYLUS_DISCRIMINANT.len() + 1 + compressed.len());
    bytecode.extend_from_slice(STYLUS_DISCRIMINANT);
    bytecode.push(0x00); // Empty dictionary
    bytecode.extend_from_slice(&compressed);

    // Derive address from bytecode hash
    let code_address = Address::from_slice(&keccak256(&bytecode)[12..32]);

    // Load the account and set the code using ContextTr trait methods
    context.journal_mut().load_account(code_address).unwrap();
    context
        .journal_mut()
        .set_code(code_address, Bytecode::new_raw(Bytes::from(bytecode)));

    code_address
}

/// Fund an address with ETH
pub fn fund_account(context: &mut TestContext, address: Address, amount: U256) {
    context.journal_mut().load_account(address).unwrap();
    context.journal_mut().balance_incr(address, amount).unwrap();
}

/// Create a call transaction to a Stylus program
#[allow(dead_code)]
pub fn create_call_tx(to: Address, calldata: Vec<u8>, gas_limit: u64) -> TxEnv {
    create_call_tx_with_nonce(to, calldata, gas_limit, 0)
}

/// Create a call transaction to a Stylus program with a specific nonce
#[allow(dead_code)]
pub fn create_call_tx_with_nonce(
    to: Address,
    calldata: Vec<u8>,
    gas_limit: u64,
    nonce: u64,
) -> TxEnv {
    TxEnv {
        caller: Address::repeat_byte(0x01),
        kind: TxKind::Call(to),
        data: Bytes::from(calldata),
        gas_limit,
        gas_price: 1,
        value: U256::ZERO,
        nonce,
        ..Default::default()
    }
}

/// Create a call transaction with value
#[allow(dead_code)]
pub fn create_call_tx_with_value(
    to: Address,
    calldata: Vec<u8>,
    value: U256,
    gas_limit: u64,
) -> TxEnv {
    TxEnv {
        caller: Address::repeat_byte(0x01),
        kind: TxKind::Call(to),
        data: Bytes::from(calldata),
        gas_limit,
        gas_price: 1,
        value,
        nonce: 0,
        ..Default::default()
    }
}

/// Execute a transaction and return the result
#[allow(dead_code)]
pub fn execute_tx(evm: &mut TestEvm, tx: TxEnv) -> ExecutionResult {
    evm.transact_one(tx.into())
        .expect("transaction execution failed")
}

/// Compile a WAT file to WASM bytes
#[allow(dead_code)]
pub fn compile_wat(path: &str) -> Vec<u8> {
    let wat = std::fs::read(path).unwrap_or_else(|_| panic!("failed to read {}", path));
    wasmer::wat2wasm(&wat)
        .unwrap_or_else(|_| panic!("failed to compile WAT from {}", path))
        .to_vec()
}

/// Load WAT bytes from a file
#[allow(dead_code)]
pub fn load_wat(path: &str) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|_| panic!("failed to read {}", path))
}
