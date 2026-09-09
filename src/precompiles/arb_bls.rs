use revm::{
    interpreter::{Gas, InterpreterResult},
    precompile::PrecompileId,
    primitives::{Address, U256, address},
};

use crate::{
    ArbitrumContextTr, precompile_impl,
    precompiles::{ArbPrecompileLogic, ExtendedPrecompile, StateMutability},
};

/// Disabled legacy BLS registry. Nitro still registers the empty precompile so
/// calls differ from calls to a nonexistent account.
pub fn arb_bls_precompile<CTX: ArbitrumContextTr>() -> ExtendedPrecompile<CTX> {
    ExtendedPrecompile::new(
        PrecompileId::Custom(std::borrow::Cow::Borrowed("ArbBLS")),
        address!("0000000000000000000000000000000000000067"),
        precompile_impl!(ArbBlsPrecompile),
    )
}

struct ArbBlsPrecompile;

impl<CTX: ArbitrumContextTr> ArbPrecompileLogic<CTX> for ArbBlsPrecompile {
    const STATE_MUT_TABLE: &'static [([u8; 4], StateMutability)] = &[];

    fn inner(
        _context: &mut CTX,
        _input: &[u8],
        _target_address: &Address,
        _caller_address: Address,
        _call_value: U256,
        _is_static: bool,
        gas_limit: u64,
    ) -> Option<InterpreterResult> {
        Some(InterpreterResult {
            result: revm::interpreter::InstructionResult::Revert,
            gas: Gas::new_spent_with_reservoir(gas_limit, 0),
            output: Default::default(),
        })
    }
}
