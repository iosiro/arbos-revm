use revm::{
    context_interface::result::{ExecutionResult, HaltReason},
    interpreter::InstructionResult,
    primitives::B256,
};

/// A receipt-level failure whose state changes were intentionally committed by
/// ArbOS. This remains distinct from an EVM/DB error and from ordinary revert
/// data, matching Nitro's `endTxNow + error` outcome.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ArbitrumCommittedFailure {
    Deposit,
    Internal,
    SubmitRetryable,
    FilteredTransaction { hash: B256 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ArbitrumExecutionOutcome {
    pub result: ExecutionResult<HaltReason>,
    pub committed_failure: Option<ArbitrumCommittedFailure>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ArbitrumHaltReason(pub HaltReason);

impl From<HaltReason> for ArbitrumHaltReason {
    fn from(value: HaltReason) -> Self {
        Self(value)
    }
}

impl From<ArbitrumHaltReason> for HaltReason {
    fn from(val: ArbitrumHaltReason) -> Self {
        val.0
    }
}

impl From<ArbitrumHaltReason> for InstructionResult {
    fn from(val: ArbitrumHaltReason) -> Self {
        val.0.into()
    }
}
