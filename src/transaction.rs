use std::ops::{Deref, DerefMut};

use alloy_rlp::{Encodable, Header, RlpDecodable, RlpEncodable};
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
    ARBITRUM_DEPOSIT_TX_TYPE, ARBITRUM_INTERNAL_TX_TYPE, ARBOS_ADDRESS, ARBOS_BATCH_POSTER_ADDRESS,
    ARBOS_STATE_ADDRESS,
};

/// Origin metadata that affects ArbOS fee charging and filtering.
///
/// This is deliberately explicit: absence of a poster address cannot safely
/// distinguish a batch transaction from a delayed-inbox or exempt message.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ArbitrumTxProvenance {
    /// Transaction data was posted by this batch poster.
    BatchPoster(Address),
    /// Transaction arrived through the delayed inbox and has no poster cost.
    DelayedInbox,
    /// System or otherwise explicitly L1-poster-cost-exempt transaction.
    #[default]
    Exempt,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ArbitrumTransactionError {
    Base(InvalidTransaction),
    L1FeeCalculationFailed,
    RetryPreparationFailed,
}

impl TransactionError for ArbitrumTransactionError {}

impl std::fmt::Display for ArbitrumTransactionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Base(e) => e.fmt(f),
            Self::L1FeeCalculationFailed => write!(f, "L1 fee calculation failed"),
            Self::RetryPreparationFailed => write!(f, "scheduled retry preparation failed"),
        }
    }
}

impl std::error::Error for ArbitrumTransactionError {}

#[cfg(feature = "alloy")]
impl alloy_evm::InvalidTxError for ArbitrumTransactionError {
    fn as_invalid_tx_err(&self) -> Option<&InvalidTransaction> {
        match self {
            Self::Base(error) => Some(error),
            Self::L1FeeCalculationFailed | Self::RetryPreparationFailed => None,
        }
    }
}

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
    /// Legacy poster projection retained for source compatibility.
    pub poster: Option<Address>,
    /// Canonical transaction identity used by ArbOS filtering.
    pub canonical_hash: Option<B256>,
    /// Explicit message provenance used by poster-fee rules.
    pub provenance: ArbitrumTxProvenance,
}

#[cfg(feature = "alloy")]
impl alloy_evm::IntoTxEnv<Self> for ArbitrumTransaction {
    fn into_tx_env(self) -> Self {
        self
    }
}

#[cfg(feature = "alloy")]
impl alloy_evm::FromRecoveredTx<alloy_consensus::TxEnvelope> for ArbitrumTransaction {
    fn from_recovered_tx(tx: &alloy_consensus::TxEnvelope, sender: Address) -> Self {
        Self::new_with_enveloped(
            TxEnv::from_recovered_tx(tx, sender),
            Bytes::from(alloy_eips::eip2718::Encodable2718::encoded_2718(tx)),
        )
    }
}

impl ArbitrumTransaction {
    pub fn new(base: TxEnv) -> Self {
        Self {
            base,
            enveloped_tx: None,
            poster: None,
            canonical_hash: None,
            provenance: ArbitrumTxProvenance::Exempt,
        }
    }

    /// Create a new transaction with enveloped bytes for L1 cost calculation
    pub fn new_with_enveloped(base: TxEnv, enveloped_tx: Bytes) -> Self {
        let canonical_hash = keccak256(&enveloped_tx);
        Self {
            base,
            enveloped_tx: Some(enveloped_tx),
            poster: Some(ARBOS_BATCH_POSTER_ADDRESS),
            canonical_hash: Some(canonical_hash),
            provenance: ArbitrumTxProvenance::BatchPoster(ARBOS_BATCH_POSTER_ADDRESS),
        }
    }

    /// Create a new transaction with enveloped bytes and poster address
    pub fn new_with_enveloped_and_poster(
        base: TxEnv,
        enveloped_tx: Bytes,
        poster: Address,
    ) -> Self {
        let canonical_hash = keccak256(&enveloped_tx);
        Self {
            base,
            enveloped_tx: Some(enveloped_tx),
            poster: Some(poster),
            canonical_hash: Some(canonical_hash),
            provenance: ArbitrumTxProvenance::BatchPoster(poster),
        }
    }

    /// Marks an enveloped transaction as delayed-inbox sourced.
    pub fn new_delayed(base: TxEnv, enveloped_tx: Bytes) -> Self {
        let canonical_hash = keccak256(&enveloped_tx);
        Self {
            base,
            canonical_hash: Some(canonical_hash),
            enveloped_tx: Some(enveloped_tx),
            poster: None,
            provenance: ArbitrumTxProvenance::DelayedInbox,
        }
    }

    /// Overrides the canonical identity when the backend already decoded it.
    pub fn with_canonical_hash(mut self, hash: B256) -> Self {
        self.canonical_hash = Some(hash);
        self
    }
}

/// Computes Nitro's EIP-2718 hash for a scheduled `ArbitrumRetryTx` (type
/// `0x68`). The retry target is RLP empty when the attempt creates a contract.
#[allow(clippy::too_many_arguments)]
pub fn arbitrum_retry_tx_hash(
    chain_id: U256,
    nonce: u64,
    from: Address,
    gas_fee_cap: U256,
    gas: u64,
    to: Option<Address>,
    value: U256,
    data: &Bytes,
    ticket_id: B256,
    refund_to: Address,
    max_refund: U256,
    submission_fee_refund: U256,
) -> B256 {
    let mut payload = Vec::new();
    chain_id.encode(&mut payload);
    nonce.encode(&mut payload);
    from.encode(&mut payload);
    gas_fee_cap.encode(&mut payload);
    gas.encode(&mut payload);
    match to {
        Some(address) => address.encode(&mut payload),
        None => (&[] as &[u8]).encode(&mut payload),
    }
    value.encode(&mut payload);
    data.as_ref().encode(&mut payload);
    ticket_id.encode(&mut payload);
    refund_to.encode(&mut payload);
    max_refund.encode(&mut payload);
    submission_fee_refund.encode(&mut payload);

    let mut encoded = Vec::with_capacity(1 + payload.len() + 9);
    encoded.push(0x68);
    Header {
        list: true,
        payload_length: payload.len(),
    }
    .encode(&mut encoded);
    encoded.extend_from_slice(&payload);
    keccak256(encoded)
}

/// Backend-ready representation of Nitro's derived type-0x68 retry transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ArbitrumRetryTx {
    pub chain_id: U256,
    pub nonce: u64,
    pub from: Address,
    pub gas_fee_cap: U256,
    pub gas_limit: u64,
    pub to: Option<Address>,
    pub value: U256,
    pub data: Bytes,
    pub ticket_id: B256,
    pub refund_to: Address,
    pub max_refund: U256,
    pub submission_fee_refund: U256,
}

impl ArbitrumRetryTx {
    pub fn hash(&self) -> B256 {
        arbitrum_retry_tx_hash(
            self.chain_id,
            self.nonce,
            self.from,
            self.gas_fee_cap,
            self.gas_limit,
            self.to,
            self.value,
            &self.data,
            self.ticket_id,
            self.refund_to,
            self.max_refund,
            self.submission_fee_refund,
        )
    }

    pub fn into_transaction(self) -> ArbitrumTransaction {
        let hash = self.hash();
        let gas_price = self.gas_fee_cap.try_into().unwrap_or(u128::MAX);
        ArbitrumTransaction {
            base: TxEnv {
                tx_type: crate::constants::ARBITRUM_RETRY_TX_TYPE,
                caller: self.from,
                gas_limit: self.gas_limit,
                gas_price,
                gas_priority_fee: Some(0),
                kind: self.to.map_or(TxKind::Create, TxKind::Call),
                value: self.value,
                data: self.data,
                nonce: self.nonce,
                chain_id: self.chain_id.try_into().ok(),
                ..Default::default()
            },
            enveloped_tx: None,
            poster: None,
            canonical_hash: Some(hash),
            provenance: ArbitrumTxProvenance::Exempt,
        }
    }
}

/// Computes the canonical EIP-2718 hash for an `ArbitrumSubmitRetryableTx`
/// (type `0x69`). A zero retry target is encoded as RLP nil/contract creation.
#[allow(clippy::too_many_arguments)]
pub fn arbitrum_submit_retryable_tx_hash(
    chain_id: U256,
    request_id: B256,
    from: Address,
    l1_base_fee: U256,
    deposit_value: U256,
    gas_fee_cap: U256,
    gas: u64,
    retry_to: Option<Address>,
    retry_value: U256,
    beneficiary: Address,
    max_submission_fee: U256,
    fee_refund_address: Address,
    retry_data: &Bytes,
) -> B256 {
    let mut payload = Vec::new();
    chain_id.encode(&mut payload);
    request_id.encode(&mut payload);
    from.encode(&mut payload);
    l1_base_fee.encode(&mut payload);
    deposit_value.encode(&mut payload);
    gas_fee_cap.encode(&mut payload);
    gas.encode(&mut payload);
    match retry_to {
        Some(address) => address.encode(&mut payload),
        None => (&[] as &[u8]).encode(&mut payload),
    }
    retry_value.encode(&mut payload);
    beneficiary.encode(&mut payload);
    max_submission_fee.encode(&mut payload);
    fee_refund_address.encode(&mut payload);
    retry_data.as_ref().encode(&mut payload);

    let mut encoded = Vec::with_capacity(1 + payload.len() + 9);
    encoded.push(0x69);
    Header {
        list: true,
        payload_length: payload.len(),
    }
    .encode(&mut encoded);
    encoded.extend_from_slice(&payload);
    keccak256(encoded)
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

    /// Canonical transaction hash supplied or derived by the backend.
    fn canonical_hash(&self) -> Option<B256>;

    /// Provenance controlling delayed-inbox and poster-cost behavior.
    fn provenance(&self) -> ArbitrumTxProvenance;

    /// Drops the transaction tip before validation and execution, matching
    /// Nitro's mutation of the execution message when tips are not collected.
    fn drop_tip(&mut self, base_fee: u128);
}

impl ArbitrumTxTr for ArbitrumTransaction {
    fn enveloped_tx(&self) -> Option<&Bytes> {
        self.enveloped_tx.as_ref()
    }

    fn poster(&self) -> Option<Address> {
        self.poster
    }

    fn canonical_hash(&self) -> Option<B256> {
        self.canonical_hash
    }

    fn provenance(&self) -> ArbitrumTxProvenance {
        self.provenance
    }

    fn drop_tip(&mut self, base_fee: u128) {
        if self.base.effective_gas_price(base_fee) > base_fee {
            self.base.gas_price = base_fee;
            self.base.gas_priority_fee = Some(0);
        }
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

    fn canonical_hash(&self) -> Option<B256> {
        None
    }

    fn provenance(&self) -> ArbitrumTxProvenance {
        ArbitrumTxProvenance::Exempt
    }

    fn drop_tip(&mut self, base_fee: u128) {
        if self.effective_gas_price(base_fee) > base_fee {
            self.gas_price = base_fee;
            self.gas_priority_fee = Some(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use revm::primitives::{address, b256};

    fn retry_hash(to: Option<Address>) -> B256 {
        arbitrum_retry_tx_hash(
            U256::from(42_161),
            7,
            address!("1111111111111111111111111111111111111111"),
            U256::from(100_000_000),
            54_321,
            to,
            U256::from(99),
            &Bytes::from_static(&[0xde, 0xad, 0xbe, 0xef]),
            b256!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            address!("2222222222222222222222222222222222222222"),
            U256::MAX,
            U256::from(1234),
        )
    }

    #[test]
    fn retry_tx_hash_matches_nitro_for_call_and_create() {
        // Independently generated by Nitro v3.11.0's types.NewTx(...).Hash().
        assert_eq!(
            retry_hash(Some(address!("3333333333333333333333333333333333333333"))),
            b256!("ea536d40099896924c0d6fd6dcab82929bb442a8e6ed3c8f1bf7552f3dc3c998")
        );
        assert_eq!(
            retry_hash(None),
            b256!("211e14bec006a19e5bfccc7f1177cbc257b1114e5000c35807dd388c08945032")
        );
    }

    #[test]
    fn submit_retryable_hash_matches_nitro() {
        assert_eq!(
            arbitrum_submit_retryable_tx_hash(
                U256::from(42_161),
                b256!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
                address!("1111111111111111111111111111111111111111"),
                U256::from(100),
                U256::from(1_000_000),
                U256::from(200),
                54_321,
                Some(address!("3333333333333333333333333333333333333333")),
                U256::from(99),
                address!("4444444444444444444444444444444444444444"),
                U256::from(12_345),
                address!("2222222222222222222222222222222222222222"),
                &Bytes::from_static(&[0xde, 0xad, 0xbe, 0xef]),
            ),
            b256!("d9dd9c321f46fab07a28e73626ee09fc32d70bd5d831246c89c5cf03955d57c0")
        );
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
    pub const START_BLOCK_METHOD: [u8; 4] = [0x6b, 0xf6, 0xa4, 0x2d];
    /// BatchPostingReport method selector
    pub const BATCH_POSTING_REPORT_METHOD: [u8; 4] = [0xb6, 0x69, 0x37, 0x71];
    /// BatchPostingReportV2 method selector (ArbOS 50+)
    pub const BATCH_POSTING_REPORT_V2_METHOD: [u8; 4] = [0x99, 0x98, 0x26, 0x9e];

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

    fn canonical_hash(&self) -> Option<B256> {
        match self {
            Self::Standard(tx) => tx.canonical_hash(),
            Self::Deposit(tx) => Some(tx.hash()),
            Self::Internal(tx) => Some(tx.hash()),
        }
    }

    fn provenance(&self) -> ArbitrumTxProvenance {
        match self {
            Self::Standard(tx) => tx.provenance(),
            Self::Deposit(_) | Self::Internal(_) => ArbitrumTxProvenance::Exempt,
        }
    }

    fn drop_tip(&mut self, base_fee: u128) {
        if let Self::Standard(tx) = self {
            tx.drop_tip(base_fee);
        }
    }
}
