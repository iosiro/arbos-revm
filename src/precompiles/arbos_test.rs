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
    interface ArbosTest {
        function burnArbGas(uint256 gasAmount) external pure;
    }
}

pub fn arbos_test_precompile<CTX: ArbitrumContextTr>() -> ExtendedPrecompile<CTX> {
    ExtendedPrecompile::new(
        PrecompileId::Custom(std::borrow::Cow::Borrowed("ArbosTest")),
        address!("0000000000000000000000000000000000000069"),
        precompile_impl!(ArbosTestPrecompile),
    )
}

struct ArbosTestPrecompile;

impl<CTX: ArbitrumContextTr> ArbPrecompileLogic<CTX> for ArbosTestPrecompile {
    const STATE_MUT_TABLE: &'static [([u8; 4], StateMutability)] = generate_state_mut_table! {
        ArbosTest => { burnArbGasCall(Pure) }
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
        if selector != ArbosTest::burnArbGasCall::SELECTOR {
            gas.spend_all();
            interpreter_revert!(gas, Bytes::new());
        }
        let call = decode_call!(gas, ArbosTest::burnArbGasCall, input);
        let Ok(amount) = u64::try_from(call.gasAmount) else {
            gas.spend_all();
            interpreter_revert!(gas, Bytes::from_static(b"not a uint64"));
        };
        if !gas.record_regular_cost(amount) {
            // Nitro deliberately ignores Burn's out-of-gas error here.
            gas.spend_all();
        }
        interpreter_return!(gas, Bytes::new());
    }
}
