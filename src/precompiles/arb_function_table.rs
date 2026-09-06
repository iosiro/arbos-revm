use alloy_sol_types::{SolCall, sol};
use revm::{
    interpreter::{Gas, InterpreterResult},
    precompile::PrecompileId,
    primitives::{Address, Bytes, U256, address},
};

use crate::{
    ArbitrumContextTr, generate_state_mut_table,
    macros::{interpreter_return, interpreter_revert},
    precompile_impl,
    precompiles::{
        ArbPrecompileLogic, ExtendedPrecompile, StateMutability, decode_call, selector_or_revert,
    },
};

sol! {
    interface ArbFunctionTable {
        function upload(bytes calldata buf) external;
        function size(address addr) external view returns (uint256);
        function get(address addr, uint256 index) external view returns (uint256, bool, uint256);
    }
}

pub fn arb_function_table_precompile<CTX: ArbitrumContextTr>() -> ExtendedPrecompile<CTX> {
    ExtendedPrecompile::new(
        PrecompileId::Custom(std::borrow::Cow::Borrowed("ArbFunctionTable")),
        address!("0000000000000000000000000000000000000068"),
        precompile_impl!(ArbFunctionTablePrecompile),
    )
}

struct ArbFunctionTablePrecompile;

impl<CTX: ArbitrumContextTr> ArbPrecompileLogic<CTX> for ArbFunctionTablePrecompile {
    const STATE_MUT_TABLE: &'static [([u8; 4], StateMutability)] = generate_state_mut_table! {
        ArbFunctionTable => {
            uploadCall(NonPayable),
            sizeCall(View),
            getCall(View),
        }
    };

    fn inner(
        _context: &mut CTX,
        input: &[u8],
        _target_address: &Address,
        _caller_address: Address,
        _call_value: U256,
        _is_static: bool,
        gas_limit: u64,
    ) -> Option<InterpreterResult> {
        let mut gas = Gas::new(gas_limit);
        let selector = selector_or_revert!(gas, input);
        match selector {
            ArbFunctionTable::uploadCall::SELECTOR => {
                let _ = decode_call!(gas, ArbFunctionTable::uploadCall, input);
                interpreter_return!(gas);
            }
            ArbFunctionTable::sizeCall::SELECTOR => {
                let _ = decode_call!(gas, ArbFunctionTable::sizeCall, input);
                interpreter_return!(
                    gas,
                    ArbFunctionTable::sizeCall::abi_encode_returns(&U256::ZERO)
                );
            }
            ArbFunctionTable::getCall::SELECTOR => {
                let _ = decode_call!(gas, ArbFunctionTable::getCall, input);
                interpreter_revert!(gas, Bytes::from_static(b"table is empty"));
            }
            _ => interpreter_revert!(gas),
        }
    }
}
