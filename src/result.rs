use revm::{context_interface::result::HaltReason, interpreter::InstructionResult};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ArbitrumHaltReason(pub HaltReason);

impl From<HaltReason> for ArbitrumHaltReason {
    fn from(value: HaltReason) -> Self {
        ArbitrumHaltReason(value)
    }
}

impl Into<HaltReason> for ArbitrumHaltReason {
    fn into(self) -> HaltReason {
        self.0
    }
}

impl Into<InstructionResult> for ArbitrumHaltReason {
    fn into(self) -> InstructionResult {
        self.0.into()
    }
}
