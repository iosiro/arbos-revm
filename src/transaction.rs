use std::ops::{Deref, DerefMut};

use alloy_rlp::{Encodable, RlpDecodable, RlpEncodable};
use revm::{
    context::{
        Transaction, TxEnv,
        result::{EVMError, InvalidTransaction},
        transaction::TransactionError,
    },
    handler::SystemCallTx,
    primitives::{Address, B256, Bytes, TxKind, U256, keccak256},
};

use crate::constants::{
    ARBITRUM_DEPOSIT_TX_TYPE, ARBITRUM_INTERNAL_TX_TYPE, ARBOS_ADDRESS, ARBOS_STATE_ADDRESS,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ArbitrumTransactionError {
    Base(InvalidTransaction),
    L1FeeCalculationFailed,
}

impl TransactionError for ArbitrumTransactionError {}

impl std::fmt::Display for ArbitrumTransactionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Base(e) => e.fmt(f),
            Self::L1FeeCalculationFailed => write!(f, "L1 fee calculation failed"),
        }
    }
}

impl std::error::Error for ArbitrumTransactionError {}

impl From<InvalidTransaction> for ArbitrumTransactionError {
    fn from(value: InvalidTransaction) -> Self {
        Self::Base(value)
    }
}

impl<DBError> From<ArbitrumTransactionError> for EVMError<DBError, ArbitrumTransactionError> {
    fn from(value: ArbitrumTransactionError) -> Self {
        Self::Transaction(value)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ArbitrumTransaction {
    /// Base transaction environment
    pub base: TxEnv,
    /// The enveloped EIP-2718 transaction bytes for L1 cost calculation.
    /// This contains the full serialized transaction used to compute L1 data costs.
    pub enveloped_tx: Option<Bytes>,
    /// The poster address that submitted this transaction to L1 (batch poster)
    pub poster: Option<Address>,
}

impl ArbitrumTransaction {
    pub fn new(base: TxEnv) -> Self {
        Self {
            base,
            enveloped_tx: None,
            poster: None,
        }
    }

    /// Create a new transaction with enveloped bytes for L1 cost calculation
    pub fn new_with_enveloped(base: TxEnv, enveloped_tx: Bytes) -> Self {
        Self {
            base,
            enveloped_tx: Some(enveloped_tx),
            poster: None,
        }
    }

    /// Create a new transaction with enveloped bytes and poster address
    pub fn new_with_enveloped_and_poster(
        base: TxEnv,
        enveloped_tx: Bytes,
        poster: Address,
    ) -> Self {
        Self {
            base,
            enveloped_tx: Some(enveloped_tx),
            poster: Some(poster),
        }
    }
}

impl From<TxEnv> for ArbitrumTransaction {
    fn from(tx_env: TxEnv) -> Self {
        Self::new(tx_env)
    }
}

impl SystemCallTx for ArbitrumTransaction {
    fn new_system_tx_with_caller(
        caller: Address,
        system_contract_address: Address,
        data: Bytes,
    ) -> Self {
        Self::new(TxEnv::new_system_tx_with_caller(
            caller,
            system_contract_address,
            data,
        ))
    }

    fn new_system_tx(system_contract_address: Address, data: Bytes) -> Self {
        Self::new_system_tx_with_caller(
            revm::handler::SYSTEM_ADDRESS,
            system_contract_address,
            data,
        )
    }
}

impl Transaction for ArbitrumTransaction {
    type AccessListItem<'a> = <TxEnv as Transaction>::AccessListItem<'a>;
    type Authorization<'a> = <TxEnv as Transaction>::Authorization<'a>;

    fn tx_type(&self) -> u8 {
        self.base.tx_type()
    }

    fn caller(&self) -> Address {
        self.base.caller()
    }

    fn gas_limit(&self) -> u64 {
        self.base.gas_limit()
    }

    fn value(&self) -> U256 {
        self.base.value()
    }

    fn input(&self) -> &Bytes {
        self.base.input()
    }

    fn nonce(&self) -> u64 {
        self.base.nonce()
    }

    fn kind(&self) -> TxKind {
        self.base.kind()
    }

    fn chain_id(&self) -> Option<u64> {
        self.base.chain_id()
    }

    fn access_list(&self) -> Option<impl Iterator<Item = Self::AccessListItem<'_>>> {
        self.base.access_list()
    }

    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        self.base.max_priority_fee_per_gas()
    }

    fn max_fee_per_gas(&self) -> u128 {
        self.base.max_fee_per_gas()
    }

    fn gas_price(&self) -> u128 {
        self.base.gas_price()
    }

    fn blob_versioned_hashes(&self) -> &[B256] {
        self.base.blob_versioned_hashes()
    }

    fn max_fee_per_blob_gas(&self) -> u128 {
        self.base.max_fee_per_blob_gas()
    }

    fn effective_gas_price(&self, base_fee: u128) -> u128 {
        self.base.effective_gas_price(base_fee)
    }

    fn authorization_list_len(&self) -> usize {
        self.base.authorization_list_len()
    }

    fn authorization_list(&self) -> impl Iterator<Item = Self::Authorization<'_>> {
        self.base.authorization_list()
    }
}

/// Trait for Arbitrum-specific transaction operations
pub trait ArbitrumTxTr: Transaction {
    /// Returns the enveloped transaction bytes for L1 cost calculation.
    /// Returns None if not set (e.g., for internal/system transactions).
    fn enveloped_tx(&self) -> Option<&Bytes>;

    /// Returns the poster address that submitted this transaction.
    fn poster(&self) -> Option<Address>;
}

impl ArbitrumTxTr for ArbitrumTransaction {
    fn enveloped_tx(&self) -> Option<&Bytes> {
        self.enveloped_tx.as_ref()
    }

    fn poster(&self) -> Option<Address> {
        self.poster
    }
}

impl Deref for ArbitrumTransaction {
    type Target = TxEnv;

    fn deref(&self) -> &Self::Target {
        &self.base
    }
}

impl DerefMut for ArbitrumTransaction {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.base
    }
}

// Implement ArbitrumTxTr for TxEnv with default (no L1 data) behavior
impl ArbitrumTxTr for TxEnv {
    fn enveloped_tx(&self) -> Option<&Bytes> {
        None
    }

    fn poster(&self) -> Option<Address> {
        None
    }
}

/// Arbitrum Deposit Transaction (type 0x64)
///
/// Represents an L1 to L2 ETH deposit. These transactions:
/// - Have no gas cost (gas is 0)
/// - Have no signature (system-generated)
/// - Skip nonce checks
/// - Mint balance to `from` then transfer to `to`
#[derive(Clone, Debug, Default, PartialEq, Eq, RlpEncodable, RlpDecodable)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ArbitrumDepositTx {
    /// Chain ID
    pub chain_id: u64,
    /// L1 request ID for tracking and replay protection
    pub l1_request_id: B256,
    /// Source address (balance will be minted here first)
    pub from: Address,
    /// Destination address (receives the value)
    pub to: Address,
    /// Amount of ETH to deposit (in wei)
    pub value: U256,
}

impl ArbitrumDepositTx {
    /// Transaction type identifier
    pub const TX_TYPE: u8 = ARBITRUM_DEPOSIT_TX_TYPE;

    /// Create a new deposit transaction
    pub fn new(
        chain_id: u64,
        l1_request_id: B256,
        from: Address,
        to: Address,
        value: U256,
    ) -> Self {
        Self {
            chain_id,
            l1_request_id,
            from,
            to,
            value,
        }
    }

    /// Compute the hash of the deposit transaction
    pub fn hash(&self) -> B256 {
        let mut buf = Vec::new();
        buf.push(Self::TX_TYPE);
        self.encode(&mut buf);
        keccak256(&buf)
    }
}

/// Arbitrum Internal Transaction (type 0x6A)
///
/// System-level transactions for ArbOS state updates. These transactions:
/// - Have no gas cost (gas is 0)
/// - Have no signature (system-generated)
/// - Skip nonce checks
/// - Sender is always ARBOS_ADDRESS
/// - Target is always ARBOS_STATE_ADDRESS
#[derive(Clone, Debug, Default, PartialEq, Eq, RlpEncodable, RlpDecodable)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ArbitrumInternalTx {
    /// Chain ID
    pub chain_id: u64,
    /// Encoded instruction data (method selector + parameters)
    pub data: Bytes,
}

impl ArbitrumInternalTx {
    /// Transaction type identifier
    pub const TX_TYPE: u8 = ARBITRUM_INTERNAL_TX_TYPE;

    // Method selectors for internal transactions
    /// StartBlock method selector
    pub const START_BLOCK_METHOD: [u8; 4] = [0x00, 0x00, 0x00, 0x01];
    /// BatchPostingReport method selector
    pub const BATCH_POSTING_REPORT_METHOD: [u8; 4] = [0x00, 0x00, 0x00, 0x02];
    /// BatchPostingReportV2 method selector (ArbOS 50+)
    pub const BATCH_POSTING_REPORT_V2_METHOD: [u8; 4] = [0x00, 0x00, 0x00, 0x03];

    /// Create a new internal transaction
    pub fn new(chain_id: u64, data: Bytes) -> Self {
        Self { chain_id, data }
    }

    /// Compute the hash of the internal transaction
    pub fn hash(&self) -> B256 {
        let mut buf = Vec::new();
        buf.push(Self::TX_TYPE);
        self.encode(&mut buf);
        keccak256(&buf)
    }

    /// Get the method selector from the data
    pub fn method_selector(&self) -> Option<[u8; 4]> {
        if self.data.len() >= 4 {
            let mut selector = [0u8; 4];
            selector.copy_from_slice(&self.data[..4]);
            Some(selector)
        } else {
            None
        }
    }

    /// Check if this is a StartBlock instruction
    pub fn is_start_block(&self) -> bool {
        self.method_selector() == Some(Self::START_BLOCK_METHOD)
    }

    /// Check if this is a BatchPostingReport instruction
    pub fn is_batch_posting_report(&self) -> bool {
        matches!(
            self.method_selector(),
            Some(Self::BATCH_POSTING_REPORT_METHOD) | Some(Self::BATCH_POSTING_REPORT_V2_METHOD)
        )
    }
}

/// Enum wrapper for Arbitrum-specific transaction types
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ArbitrumTypedTransaction {
    /// Standard transaction (delegates to base TxEnv)
    Standard(Box<ArbitrumTransaction>),
    /// L1 to L2 deposit transaction (type 0x64)
    Deposit(ArbitrumDepositTx),
    /// Internal ArbOS transaction (type 0x6A)
    Internal(ArbitrumInternalTx),
}

impl Default for ArbitrumTypedTransaction {
    fn default() -> Self {
        Self::Standard(Box::default())
    }
}

impl ArbitrumTypedTransaction {
    /// Returns true if this is a deposit transaction
    pub fn is_deposit(&self) -> bool {
        matches!(self, Self::Deposit(_))
    }

    /// Returns true if this is an internal transaction
    pub fn is_internal(&self) -> bool {
        matches!(self, Self::Internal(_))
    }

    /// Returns true if this is a system transaction (deposit or internal)
    pub fn is_system_tx(&self) -> bool {
        self.is_deposit() || self.is_internal()
    }

    /// Returns the transaction type
    pub fn tx_type(&self) -> u8 {
        match self {
            Self::Standard(tx) => tx.tx_type(),
            Self::Deposit(_) => ARBITRUM_DEPOSIT_TX_TYPE,
            Self::Internal(_) => ARBITRUM_INTERNAL_TX_TYPE,
        }
    }

    /// Get as deposit transaction if applicable
    pub fn as_deposit(&self) -> Option<&ArbitrumDepositTx> {
        match self {
            Self::Deposit(tx) => Some(tx),
            _ => None,
        }
    }

    /// Get as internal transaction if applicable
    pub fn as_internal(&self) -> Option<&ArbitrumInternalTx> {
        match self {
            Self::Internal(tx) => Some(tx),
            _ => None,
        }
    }

    /// Get as standard transaction if applicable
    pub fn as_standard(&self) -> Option<&ArbitrumTransaction> {
        match self {
            Self::Standard(tx) => Some(tx),
            _ => None,
        }
    }
}

impl From<ArbitrumTransaction> for ArbitrumTypedTransaction {
    fn from(tx: ArbitrumTransaction) -> Self {
        Self::Standard(Box::new(tx))
    }
}

impl From<ArbitrumDepositTx> for ArbitrumTypedTransaction {
    fn from(tx: ArbitrumDepositTx) -> Self {
        Self::Deposit(tx)
    }
}

impl From<ArbitrumInternalTx> for ArbitrumTypedTransaction {
    fn from(tx: ArbitrumInternalTx) -> Self {
        Self::Internal(tx)
    }
}

impl Transaction for ArbitrumTypedTransaction {
    type AccessListItem<'a> = <TxEnv as Transaction>::AccessListItem<'a>;
    type Authorization<'a> = <TxEnv as Transaction>::Authorization<'a>;

    fn tx_type(&self) -> u8 {
        match self {
            Self::Standard(tx) => tx.tx_type(),
            Self::Deposit(_) => ARBITRUM_DEPOSIT_TX_TYPE,
            Self::Internal(_) => ARBITRUM_INTERNAL_TX_TYPE,
        }
    }

    fn caller(&self) -> Address {
        match self {
            Self::Standard(tx) => tx.caller(),
            Self::Deposit(tx) => tx.from,
            Self::Internal(_) => ARBOS_ADDRESS,
        }
    }

    fn gas_limit(&self) -> u64 {
        match self {
            Self::Standard(tx) => tx.gas_limit(),
            // Deposit and Internal transactions have 0 gas
            Self::Deposit(_) | Self::Internal(_) => 0,
        }
    }

    fn value(&self) -> U256 {
        match self {
            Self::Standard(tx) => tx.value(),
            Self::Deposit(tx) => tx.value,
            Self::Internal(_) => U256::ZERO,
        }
    }

    fn input(&self) -> &Bytes {
        match self {
            Self::Standard(tx) => tx.input(),
            Self::Deposit(_) => {
                // Deposit transactions have no input data
                static EMPTY: Bytes = Bytes::new();
                &EMPTY
            }
            Self::Internal(tx) => &tx.data,
        }
    }

    fn nonce(&self) -> u64 {
        match self {
            Self::Standard(tx) => tx.nonce(),
            // System transactions have nonce 0
            Self::Deposit(_) | Self::Internal(_) => 0,
        }
    }

    fn kind(&self) -> TxKind {
        match self {
            Self::Standard(tx) => tx.kind(),
            Self::Deposit(tx) => TxKind::Call(tx.to),
            Self::Internal(_) => TxKind::Call(ARBOS_STATE_ADDRESS),
        }
    }

    fn chain_id(&self) -> Option<u64> {
        match self {
            Self::Standard(tx) => tx.chain_id(),
            Self::Deposit(tx) => Some(tx.chain_id),
            Self::Internal(tx) => Some(tx.chain_id),
        }
    }

    fn access_list(&self) -> Option<impl Iterator<Item = Self::AccessListItem<'_>>> {
        match self {
            Self::Standard(tx) => tx.access_list(),
            // System transactions have no access list
            Self::Deposit(_) | Self::Internal(_) => None,
        }
    }

    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        match self {
            Self::Standard(tx) => tx.max_priority_fee_per_gas(),
            // System transactions have 0 fees
            Self::Deposit(_) | Self::Internal(_) => Some(0),
        }
    }

    fn max_fee_per_gas(&self) -> u128 {
        match self {
            Self::Standard(tx) => tx.max_fee_per_gas(),
            // System transactions have 0 fees
            Self::Deposit(_) | Self::Internal(_) => 0,
        }
    }

    fn gas_price(&self) -> u128 {
        match self {
            Self::Standard(tx) => tx.gas_price(),
            // System transactions have 0 gas price
            Self::Deposit(_) | Self::Internal(_) => 0,
        }
    }

    fn blob_versioned_hashes(&self) -> &[B256] {
        match self {
            Self::Standard(tx) => tx.blob_versioned_hashes(),
            Self::Deposit(_) | Self::Internal(_) => &[],
        }
    }

    fn max_fee_per_blob_gas(&self) -> u128 {
        match self {
            Self::Standard(tx) => tx.max_fee_per_blob_gas(),
            Self::Deposit(_) | Self::Internal(_) => 0,
        }
    }

    fn effective_gas_price(&self, base_fee: u128) -> u128 {
        match self {
            Self::Standard(tx) => tx.effective_gas_price(base_fee),
            // System transactions have 0 effective gas price
            Self::Deposit(_) | Self::Internal(_) => 0,
        }
    }

    fn authorization_list_len(&self) -> usize {
        match self {
            Self::Standard(tx) => tx.authorization_list_len(),
            Self::Deposit(_) | Self::Internal(_) => 0,
        }
    }

    fn authorization_list(&self) -> impl Iterator<Item = Self::Authorization<'_>> {
        // Only standard transactions can have authorization lists
        // For system transactions (deposit/internal), return empty iterator
        self.as_standard()
            .into_iter()
            .flat_map(|tx| tx.authorization_list())
    }
}

impl ArbitrumTxTr for ArbitrumTypedTransaction {
    fn enveloped_tx(&self) -> Option<&Bytes> {
        match self {
            Self::Standard(tx) => tx.enveloped_tx(),
            // System transactions don't have L1 data costs
            Self::Deposit(_) | Self::Internal(_) => None,
        }
    }

    fn poster(&self) -> Option<Address> {
        match self {
            Self::Standard(tx) => tx.poster(),
            // System transactions don't have a poster
            Self::Deposit(_) | Self::Internal(_) => None,
        }
    }
}
