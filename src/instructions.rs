//! Arbitrum opcode semantics over REVM's Ethereum instruction table.

use crate::{
    ArbitrumContextTr,
    state::{ArbState, ArbStateGetter, ArbosStateError},
};
use revm::{
    bytecode::opcode,
    context::ContextError,
    handler::instructions::{EthInstructions, InstructionProvider},
    interpreter::{
        Instruction, InstructionContext, InstructionResult,
        instructions::{InstructionTable, block_info, host},
        interpreter::EthInterpreter,
    },
    primitives::U256,
};

/// Ethereum instructions with ArbOS's L1 BLOCKHASH source and unsupported BLOBBASEFEE.
/// Uninitialized contexts retain Ethereum behavior for ordinary Foundry execution.
#[derive(Debug)]
pub struct ArbitrumInstructions<CTX>(EthInstructions<EthInterpreter, CTX>);

impl<CTX: ArbitrumContextTr> Default for ArbitrumInstructions<CTX> {
    fn default() -> Self {
        let mut instructions = EthInstructions::default();
        let gas = instructions.instruction_table[opcode::BLOCKHASH as usize].static_gas();
        instructions.insert_instruction(opcode::BLOCKHASH, Instruction::new(blockhash::<CTX>, gas));
        let gas = instructions.instruction_table[opcode::BLOBBASEFEE as usize].static_gas();
        instructions.insert_instruction(
            opcode::BLOBBASEFEE,
            Instruction::new(blob_basefee::<CTX>, gas),
        );
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
}

fn blockhash<CTX: ArbitrumContextTr>(context: InstructionContext<'_, CTX, EthInterpreter>) {
    if !context.host.chain().arbos_initialized {
        return host::blockhash(context);
    }
    let Some(number) = context.interpreter.stack.data_mut().last_mut() else {
        context.interpreter.halt(InstructionResult::StackUnderflow);
        return;
    };
    let Ok(requested) = u64::try_from(*number) else {
        *number = U256::ZERO;
        return;
    };
    let chain = context.host.chain();
    let synthetic = chain.synthetic_block_hashes && chain.rpc_block_number.is_none();
    if synthetic || !chain.block_hash_overrides.is_empty() {
        let current = context.host.block_number();
        if *number >= current || current - *number > U256::from(256) {
            *number = U256::ZERO;
            return;
        }
        if let Some(hash) = chain.block_hash_overrides.get(&requested) {
            *number = U256::from_be_bytes(hash.0);
            return;
        }
    }
    match context
        .host
        .arb_state(None, true)
        .blockhashes()
        .block_hash(requested)
    {
        Ok(hash) => *number = U256::from_be_bytes(hash.0),
        Err(ArbosStateError::InvalidBlockNumberForBlockHash) => {
            *number = if synthetic {
                revm::primitives::keccak256(requested.to_string().as_bytes()).into()
            } else {
                U256::ZERO
            };
        }
        Err(error) => {
            *context.host.error() = Err(ContextError::Custom(error.to_string()));
            context.interpreter.halt_fatal();
        }
    }
}

fn blob_basefee<CTX: ArbitrumContextTr>(context: InstructionContext<'_, CTX, EthInterpreter>) {
    if !context.host.chain().arbos_initialized {
        return block_info::blob_basefee(context);
    }
    context.interpreter.halt(InstructionResult::NotActivated);
}
