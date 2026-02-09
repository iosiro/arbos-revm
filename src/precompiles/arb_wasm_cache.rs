use alloy_sol_types::{SolCall, SolError, sol};
use revm::{
    context::JournalTr,
    interpreter::{Gas, InterpreterResult},
    precompile::PrecompileId,
    primitives::{Address, Bytes, Log, U256, address, alloy_primitives::IntoLogData},
};

use crate::{
    ArbitrumContextTr, generate_state_mut_table,
    macros::{emit_event, interpreter_return, interpreter_revert},
    precompile_impl,
    precompiles::{
        ArbPrecompileLogic, ExtendedPrecompile, StateMutability, decode_call, selector_or_revert,
    },
    state::{ArbState, ArbStateGetter, ArbosStateError, try_state, types::StorageBackedTr},
    try_record_cost,
};

sol! {

///
/// @title Methods for managing Stylus caches
/// @notice Precompiled contract that exists in every Arbitrum chain at 0x0000000000000000000000000000000000000072.
/// @notice Available in ArbOS version 30 and above
///
interface IArbWasmCache {
    /// @notice See if the user is a cache manager.
    function isCacheManager(
        address manager
    ) external view returns (bool);

    /// @notice Retrieve all address managers.
    /// @return managers the list of managers.
    function allCacheManagers() external view returns (address[] memory managers);

    /// @dev Deprecated, replaced with cacheProgram
    /// @notice Available in ArbOS version 30 only
    function cacheCodehash(
        bytes32 codehash
    ) external;

    /// @notice Caches all programs with a codehash equal to the given address.
    /// @notice Reverts if the programs have expired.
    /// @notice Caller must be a cache manager or chain owner.
    /// @notice If you're looking for how to bid for position, interact with the chain's cache manager contract.
    /// @notice Available in ArbOS version 31 and above
    function cacheProgram(
        address addr
    ) external;

    /// @notice Evicts all programs with the given codehash.
    /// @notice Caller must be a cache manager or chain owner.
    function evictCodehash(
        bytes32 codehash
    ) external;

    /// @notice Gets whether a program is cached. Note that the program may be expired.
    function codehashIsCached(
        bytes32 codehash
    ) external view returns (bool);

    event UpdateProgramCache(address indexed manager, bytes32 indexed codehash, bool cached);

    /// @notice Reverts if the program is expired
    error ProgramNeedsUpgrade(uint16 version, uint16 stylusVersion);
    /// @notice Reverts if the program is too large
    error ProgramExpired(uint64 ageInSeconds);
}

}

pub fn arb_wasm_cache_precompile<CTX: ArbitrumContextTr>() -> ExtendedPrecompile<CTX> {
    ExtendedPrecompile::new(
        PrecompileId::Custom(std::borrow::Cow::Borrowed("ArbWasmCache")),
        address!("0x0000000000000000000000000000000000000072"),
        precompile_impl!(ArbWasmCache),
    )
}

struct ArbWasmCache {}

impl<CTX: ArbitrumContextTr> ArbPrecompileLogic<CTX> for ArbWasmCache {
    const STATE_MUT_TABLE: &'static [([u8; 4], StateMutability)] = generate_state_mut_table! {
        IArbWasmCache => {
            isCacheManagerCall(View),
            allCacheManagersCall(View),
            cacheCodehashCall(NonPayable),
            cacheProgramCall(NonPayable),
            evictCodehashCall(NonPayable),
            codehashIsCachedCall(View),
        }
    };

    fn inner(
        context: &mut CTX,
        input: &[u8],
        target_address: &Address,
        caller_address: Address,
        _call_value: U256,
        is_static: bool,
        gas_limit: u64,
    ) -> Option<InterpreterResult> {
        let mut gas = Gas::new(gas_limit);

        let selector = selector_or_revert!(gas, input);

        match selector {
            IArbWasmCache::isCacheManagerCall::SELECTOR => {
                let call = decode_call!(gas, IArbWasmCache::isCacheManagerCall, input);
                let manager = call.manager;

                let is_manager = try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .programs()
                        .cache_managers()
                        .contains(manager)
                );

                let output = IArbWasmCache::isCacheManagerCall::abi_encode_returns(&is_manager);

                interpreter_return!(gas, Bytes::from(output));
            }
            IArbWasmCache::allCacheManagersCall::SELECTOR => {
                let _call = decode_call!(gas, IArbWasmCache::allCacheManagersCall, input);

                let managers = try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .programs()
                        .cache_managers()
                        .all()
                );

                let output = IArbWasmCache::allCacheManagersCall::abi_encode_returns(&managers);

                interpreter_return!(gas, Bytes::from(output));
            }
            IArbWasmCache::cacheCodehashCall::SELECTOR => {
                if !try_state!(gas, has_access(context, caller_address, &mut gas)) {
                    interpreter_revert!(gas);
                }

                let call = decode_call!(gas, IArbWasmCache::cacheCodehashCall, input);
                let codehash = call.codehash;

                let params = try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .programs()
                        .stylus_params()
                        .get()
                );

                let mut program_info = if let Some(program_info) = try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .programs()
                        .program_info(&codehash)
                ) {
                    program_info
                } else {
                    interpreter_revert!(
                        gas,
                        IArbWasmCache::ProgramNeedsUpgrade {
                            version: 0,
                            stylusVersion: params.version
                        }
                        .abi_encode()
                    );
                };

                let output = IArbWasmCache::cacheCodehashCall::abi_encode_returns(
                    &IArbWasmCache::cacheCodehashReturn {},
                );

                if program_info.cached {
                    // already cached, no-op
                    interpreter_return!(gas, Bytes::from(output));
                }

                program_info.cached = true;

                try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .programs()
                        .save_program_info(&codehash, &program_info)
                );

                interpreter_return!(gas, Bytes::from(output));
            }
            IArbWasmCache::cacheProgramCall::SELECTOR => {
                if !try_state!(gas, has_access(context, caller_address, &mut gas)) {
                    interpreter_revert!(gas);
                }

                let call = decode_call!(gas, IArbWasmCache::cacheProgramCall, input);
                let addr = call.addr;

                let params = try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .programs()
                        .stylus_params()
                        .get()
                );

                let code_hash = try_state!(
                    gas,
                    context.arb_state(Some(&mut gas), is_static).code_hash(addr)
                );

                let mut program_info = try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .programs()
                        .get_active_program(&params, &code_hash)
                );

                let output = IArbWasmCache::cacheProgramCall::abi_encode_returns(
                    &IArbWasmCache::cacheProgramReturn {},
                );

                if program_info.cached {
                    // already cached, no-op
                    interpreter_return!(gas, Bytes::from(output));
                }

                // emit event cost
                emit_event!(
                    context,
                    Log {
                        address: *target_address,
                        data: IArbWasmCache::UpdateProgramCache {
                            manager: caller_address,
                            codehash: code_hash,
                            cached: true
                        }
                        .into_log_data()
                    },
                    gas
                );

                try_record_cost!(gas, program_info.init_cost as u64);

                try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .programs()
                        .module_hash(&code_hash)
                        .get()
                );

                program_info.cached = true;

                try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .programs()
                        .save_program_info(&code_hash, &program_info)
                );

                interpreter_return!(gas, Bytes::from(output));
            }
            IArbWasmCache::evictCodehashCall::SELECTOR => {
                if !try_state!(gas, has_access(context, caller_address, &mut gas)) {
                    interpreter_revert!(gas);
                }

                let call = decode_call!(gas, IArbWasmCache::evictCodehashCall, input);
                let code_hash = call.codehash;

                let _ = try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .programs()
                        .stylus_params()
                        .get()
                );

                let mut program_info = try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .programs()
                        .program_info(&code_hash)
                )
                .unwrap_or_default();

                let output = IArbWasmCache::evictCodehashCall::abi_encode_returns(
                    &IArbWasmCache::evictCodehashReturn {},
                );

                if !program_info.cached {
                    // if not cached, no-op
                    interpreter_return!(gas, Bytes::from(output));
                }

                // emit event cost
                emit_event!(
                    context,
                    Log {
                        address: *target_address,
                        data: IArbWasmCache::UpdateProgramCache {
                            manager: caller_address,
                            codehash: code_hash,
                            cached: false
                        }
                        .into_log_data()
                    },
                    gas
                );

                try_record_cost!(gas, program_info.init_cost as u64);

                try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .programs()
                        .module_hash(&code_hash)
                        .get()
                );

                program_info.cached = false;

                try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .programs()
                        .save_program_info(&code_hash, &program_info)
                );

                interpreter_return!(gas, Bytes::from(output));
            }
            IArbWasmCache::codehashIsCachedCall::SELECTOR => {
                let call = decode_call!(gas, IArbWasmCache::codehashIsCachedCall, input);
                let codehash = call.codehash;

                let is_cached = if let Some(program_info) = try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), true)
                        .programs()
                        .program_info(&codehash)
                ) {
                    program_info.cached
                } else {
                    false
                };

                let output = IArbWasmCache::codehashIsCachedCall::abi_encode_returns(&is_cached);

                interpreter_return!(gas, Bytes::from(output));
            }
            _ => interpreter_revert!(gas, Bytes::from("Unknown selector")),
        }
    }
}

fn has_access<CTX: ArbitrumContextTr>(
    context: &mut CTX,
    caller: Address,
    gas: &mut Gas,
) -> Result<bool, ArbosStateError> {
    let mut arb_state = context.arb_state(Some(gas), true);
    if arb_state.programs().cache_managers().contains(caller)? {
        return Ok(true);
    }

    arb_state.is_chain_owner(caller)
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use alloy_sol_types::SolCall;
    use revm::{
        Journal,
        context::{BlockEnv, ContextTr, JournalTr},
        database::EmptyDBTyped,
        primitives::{Address, Bytes, U256, address, keccak256},
        state::Bytecode,
    };
    use stylus::brotli;
    use wasmer::wat2wasm;

    use crate::{
        ArbitrumContext, ArbitrumTransaction,
        config::ArbitrumConfig,
        constants::STYLUS_DISCRIMINANT,
        local_context::ArbitrumLocalContext,
        precompiles::{ArbPrecompileLogic, arb_wasm_cache::ArbWasmCache},
        state::{ArbState, ArbStateGetter, arbos_state::ArbosStateParams},
    };

    use super::*;

    fn setup() -> ArbitrumContext<EmptyDBTyped<Infallible>> {
        let db = EmptyDBTyped::<Infallible>::default();

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

    fn deploy_program(
        context: &mut ArbitrumContext<EmptyDBTyped<Infallible>>,
        wat: &[u8],
    ) -> Address {
        let wasm_bytes = wat2wasm(wat).unwrap();

        let wasm = brotli::compress(&wasm_bytes, 11, 22, brotli::Dictionary::Empty).unwrap();
        let wasm = {
            let mut v = Vec::with_capacity(STYLUS_DISCRIMINANT.len() + wasm.len() + 1);
            v.extend_from_slice(STYLUS_DISCRIMINANT);
            v.extend_from_slice(&[0]);
            v.extend_from_slice(&wasm);
            v
        };

        let code_address = Address::from_slice(&keccak256(&wasm)[12..32]);

        context.journal_mut().load_account(code_address).unwrap();

        context
            .journal_mut()
            .set_code(code_address, Bytecode::new_raw(Bytes::from(wasm)));

        code_address
    }

    #[test]
    fn test_wasm_cache_code_hash_is_cached() {
        let mut context = setup();

        let codehash = [0u8; 32];

        let input =
            IArbWasmCache::codehashIsCachedCall::abi_encode(&IArbWasmCache::codehashIsCachedCall {
                codehash: codehash.into(),
            });

        let result = ArbWasmCache::run(
            &mut context,
            &input,
            &super::arb_wasm_cache_precompile::<ArbitrumContext<EmptyDBTyped<Infallible>>>()
                .address,
            address!("0x0000000000000000000000000000000000000001"),
            U256::ZERO,
            true,
            1_000_000,
        )
        .unwrap();
        let output = result.output;
        let decoded = IArbWasmCache::codehashIsCachedCall::abi_decode_returns(&output)
            .expect("decode precompile output");
        assert!(!decoded);
    }

    #[test]
    fn test_caching_flow() {
        let mut context = setup();
        context.cfg.disable_auto_activate = true;
        context.cfg.disable_auto_cache = true;

        // Initialize ArbOS state with default params
        context
            .arb_state(None, false)
            .initialize(&ArbosStateParams::default())
            .expect("failed to initialize ArbOS state");

        let wat = include_bytes!("../../test-data/memory.wat");
        let program_address = deploy_program(&mut context, wat);

        // Get code hash
        let code_hash = context
            .arb_state(None, false)
            .code_hash(program_address)
            .expect("failed to get code hash");

        // Activate the program directly (not cached)
        let wasm_bytes = wat2wasm(wat).unwrap();
        crate::state::program::activate_program(
            &mut context,
            code_hash,
            &Bytes::from(wasm_bytes.to_vec()),
            false,
        )
        .expect("activation should succeed");

        let arb_wasm_cache_addr = address!("0x0000000000000000000000000000000000000072");
        let caller = address!("0x000000000000000000000000000000000000c0de");

        // Verify program is NOT cached
        let input =
            IArbWasmCache::codehashIsCachedCall::abi_encode(&IArbWasmCache::codehashIsCachedCall {
                codehash: code_hash,
            });
        let result = ArbWasmCache::run(
            &mut context,
            &input,
            &arb_wasm_cache_addr,
            caller,
            U256::ZERO,
            true,
            1_000_000,
        )
        .unwrap();
        assert!(result.is_ok());
        let is_cached = IArbWasmCache::codehashIsCachedCall::abi_decode_returns(&result.output)
            .expect("decode codehashIsCached");
        assert!(!is_cached, "program should not be cached initially");

        // Add caller as cache manager so they can cache programs
        context
            .arb_state(None, false)
            .programs()
            .cache_managers()
            .add(caller)
            .expect("failed to add cache manager");

        // Cache the program via cacheProgram precompile
        let input = IArbWasmCache::cacheProgramCall::abi_encode(&IArbWasmCache::cacheProgramCall {
            addr: program_address,
        });
        let result = ArbWasmCache::run(
            &mut context,
            &input,
            &arb_wasm_cache_addr,
            caller,
            U256::ZERO,
            false,
            10_000_000,
        )
        .unwrap();
        assert!(result.is_ok(), "cacheProgram failed: {:?}", result.result);

        // Verify program IS now cached
        let input =
            IArbWasmCache::codehashIsCachedCall::abi_encode(&IArbWasmCache::codehashIsCachedCall {
                codehash: code_hash,
            });
        let result = ArbWasmCache::run(
            &mut context,
            &input,
            &arb_wasm_cache_addr,
            caller,
            U256::ZERO,
            true,
            1_000_000,
        )
        .unwrap();
        assert!(result.is_ok());
        let is_cached = IArbWasmCache::codehashIsCachedCall::abi_decode_returns(&result.output)
            .expect("decode codehashIsCached");
        assert!(is_cached, "program should be cached after cacheProgram");
    }
}
