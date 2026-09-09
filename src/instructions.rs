//! Arbitrum opcode semantics over the ordinary Ethereum instruction and gas tables.

use crate::{
    ArbitrumContextTr,
    state::{ArbState, ArbStateGetter, ArbosStateError},
};
use revm::{
    bytecode::opcode,
    context::ContextError,
    handler::instructions::{EthInstructions, InstructionProvider},
    interpreter::{
        Host, Instruction, InstructionContext, InstructionExecResult, InstructionResult,
        InterpreterTypes,
        instructions::{GasTable, InstructionTable, gas_table_spec},
        interpreter::EthInterpreter,
    },
    primitives::{U256, hardfork::SpecId},
};

/// Ethereum instructions with ArbOS's L1 BLOCKHASH source.
///
/// The database must supply L2 block hashes for ArbSys and native block processing.
#[derive(Debug)]
pub struct ArbitrumInstructions<CTX>(EthInstructions<EthInterpreter, CTX>);

/// Instruction tables whose gas schedule can follow a persisted ArbOS upgrade.
pub trait ArbitrumInstructionProvider: InstructionProvider {
    fn set_spec(&mut self, spec: SpecId);
}

impl<IT: InterpreterTypes, CTX: Host> ArbitrumInstructionProvider for EthInstructions<IT, CTX> {
    fn set_spec(&mut self, spec: SpecId) {
        if self.spec != spec {
            self.spec = spec;
            *self.gas_table_mut() = gas_table_spec(spec);
        }
    }
}

impl<CTX: ArbitrumContextTr> ArbitrumInstructionProvider for ArbitrumInstructions<CTX> {
    fn set_spec(&mut self, spec: SpecId) {
        self.0.set_spec(spec);
    }
}

impl<CTX: ArbitrumContextTr> ArbitrumInstructions<CTX> {
    pub fn new(spec: SpecId) -> Self {
        let mut instructions = EthInstructions::new_mainnet_with_spec(spec);
        // Retain the upstream static gas charge; reading ArbOS history adds no SLOAD charge.
        instructions.instruction_table_mut()[opcode::BLOCKHASH as usize] =
            Instruction::new(blockhash::<CTX>);
        Self(instructions)
    }
}

impl<CTX: ArbitrumContextTr> Clone for ArbitrumInstructions<CTX> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<CTX: ArbitrumContextTr> InstructionProvider for ArbitrumInstructions<CTX> {
    type Context = CTX;
    type InterpreterTypes = EthInterpreter;

    fn instruction_table(&self) -> &InstructionTable<EthInterpreter, CTX> {
        self.0.instruction_table()
    }

    fn gas_table(&self) -> &GasTable {
        self.0.gas_table()
    }
}

fn blockhash<CTX: ArbitrumContextTr>(
    context: InstructionContext<'_, CTX, EthInterpreter>,
) -> InstructionExecResult {
    let number = context
        .interpreter
        .stack
        .top()
        .ok_or(InstructionResult::StackUnderflow)?;
    let Ok(requested) = u64::try_from(*number) else {
        *number = U256::ZERO;
        return Ok(());
    };
    match context
        .host
        .arb_state(None, true)
        .blockhashes()
        .block_hash(requested)
    {
        Ok(hash) => *number = U256::from_be_bytes(hash.0),
        Err(ArbosStateError::InvalidBlockNumberForBlockHash) => *number = U256::ZERO,
        Err(error) => {
            *context.host.error() = Err(ContextError::Custom(error.to_string()));
            return Err(InstructionResult::FatalExternalError);
        }
    }
    Ok(())
}
