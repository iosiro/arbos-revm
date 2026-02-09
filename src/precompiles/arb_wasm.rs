#![allow(missing_docs)]

use crate::{
    ArbitrumContextTr,
    config::ArbitrumConfigTr,
    constants::{COST_SCALAR_PERCENT, MIN_CACHED_GAS_UNITS, MIN_INIT_GAS_UNITS},
    generate_state_mut_table,
    macros::{emit_event, interpreter_return, interpreter_revert, try_or_halt},
    precompile_impl,
    precompiles::{
        ArbPrecompileLogic, ExtendedPrecompile, StateMutability, decode_call, selector_or_revert,
    },
    state::{
        ArbState, ArbStateGetter, program::activate_program, try_state, types::StorageBackedTr,
    },
    stylus_executor::stylus_code,
    try_record_cost,
};

use alloy_sol_types::{SolCall, SolError, sol};
use arbutil::evm::ARBOS_VERSION_STYLUS_CHARGING_FIXES;
use revm::{
    context::{Block, JournalTr},
    interpreter::{Gas, InterpreterResult},
    precompile::PrecompileId,
    primitives::{Address, Bytes, Log, U256, address, alloy_primitives::IntoLogData},
};
use std::fmt::Debug;

sol! {
#[derive(Debug)] // Keep this list small to avoid unnecessary bloat.
interface IArbWasm {
    /// @notice Activate a wasm program
    /// @param program the program to activate
    /// @return version the stylus version the program was activated against
    /// @return dataFee the data fee paid to store the activated program
    function activateProgram(
        address program
    ) external payable returns (uint16 version, uint256 dataFee);

    /// @notice Gets the latest stylus version
    /// @return version the stylus version
    function stylusVersion() external view returns (uint16 version);

    /// @notice Gets the stylus version the program with codehash was most recently activated against
    /// @return version the program version (reverts for EVM contracts)
    function codehashVersion(
        bytes32 codehash
    ) external view returns (uint16 version);

    /// @notice Extends a program's expiration date.
    /// Reverts if too soon or if the program is not up to date.
    function codehashKeepalive(
        bytes32 codehash
    ) external payable;

    /// @notice Gets a program's asm size.
    /// Reverts if program is not active.
    /// @return size the size in bytes
    function codehashAsmSize(
        bytes32 codehash
    ) external view returns (uint32 size);

    /// @notice Gets the stylus version the program was most recently activated against
    /// @return version the program version (reverts for EVM contracts)
    function programVersion(
        address program
    ) external view returns (uint16 version);

    /// @notice Gets the cost to invoke the program
    /// @return gas the amount of gas
    /// @return gasWhenCached the amount of gas if the program was recently used
    function programInitGas(
        address program
    ) external view returns (uint64 gas, uint64 gasWhenCached);

    /// @notice Gets the memory footprint of the program at the given address in pages
    /// @return footprint the memory footprint of program in pages (reverts for EVM contracts)
    function programMemoryFootprint(
        address program
    ) external view returns (uint16 footprint);

    /// @notice Gets the amount of time remaining until the program expires
    /// @return _secs the time left in seconds (reverts for EVM contracts)
    function programTimeLeft(
        address program
    ) external view returns (uint64 _secs);

    /// @notice Gets the conversion rate between gas and ink
    /// @return price the amount of ink 1 gas buys
    function inkPrice() external view returns (uint32 price);

    /// @notice Gets the wasm stack size limit
    /// @return depth the maximum depth (in wasm words) a wasm stack may grow
    function maxStackDepth() external view returns (uint32 depth);

    /// @notice Gets the number of free wasm pages a program gets
    /// @return pages the number of wasm pages (2^16 bytes)
    function freePages() external view returns (uint16 pages);

    /// @notice Gets the base cost of each additional wasm page (2^16 bytes)
    /// @return gas base amount of gas needed to grow another wasm page
    function pageGas() external view returns (uint16 gas);

    /// @notice Gets the ramp that drives exponential memory costs
    /// @return ramp bits representing the floating point value
    function pageRamp() external view returns (uint64 ramp);

    /// @notice Gets the maximum number of pages a wasm may allocate
    /// @return limit the number of pages
    function pageLimit() external view returns (uint16 limit);

    /// @notice Gets the minimum costs to invoke a program
    /// @return gas amount of gas in increments of 256 when not cached
    /// @return cached amount of gas in increments of 64 when cached
    function minInitGas() external view returns (uint64 gas, uint64 cached);

    /// @notice Gets the linear adjustment made to program init costs.
    /// @return percent the adjustment (100% = no adjustment).
    function initCostScalar() external view returns (uint64 percent);

    /// @notice Gets the number of days after which programs deactivate
    /// @return _days the number of days
    function expiryDays() external view returns (uint16 _days);

    /// @notice Gets the age a program must be to perform a keepalive
    /// @return _days the number of days
    function keepaliveDays() external view returns (uint16 _days);

    /// @notice Gets the number of extra programs ArbOS caches during a given block.
    /// @return count the number of same-block programs.
    function blockCacheSize() external view returns (uint16 count);

    /// @notice Emitted when a program is activated
    event ProgramActivated(
        bytes32 indexed codehash,
        bytes32 moduleHash,
        address program,
        uint256 dataFee,
        uint16 version
    );
    /// @notice Emitted when a program's lifetime is extended
    event ProgramLifetimeExtended(bytes32 indexed codehash, uint256 dataFee);

    /// @notice Reverts if the program is not a wasm program
    error ProgramNotWasm();
    /// @notice Reverts if the program is not active
    error ProgramNotActivated();
    /// @notice Reverts if the program is expired
    error ProgramNeedsUpgrade(uint16 version, uint16 stylusVersion);
    /// @notice Reverts if the program is too large
    error ProgramExpired(uint64 ageInSeconds);
    /// @notice Reverts if the program is up to date
    error ProgramUpToDate();
    /// @notice Reverts if the program keepalive is too soon
    error ProgramKeepaliveTooSoon(uint64 ageInSeconds);
    /// @notice Reverts if the program has insufficient value
    error ProgramInsufficientValue(uint256 have, uint256 want);
}
}

const STYLUS_ACTIVATION_FIXED_COST: u64 = 1659168;

pub fn arb_wasm_precompile<CTX: ArbitrumContextTr>() -> ExtendedPrecompile<CTX> {
    ExtendedPrecompile::new(
        PrecompileId::Custom(std::borrow::Cow::Borrowed("ArbWasm")),
        address!("0x0000000000000000000000000000000000000071"),
        precompile_impl!(ArbWasmPrecompile),
    )
}
struct ArbWasmPrecompile;

impl<CTX: ArbitrumContextTr> ArbPrecompileLogic<CTX> for ArbWasmPrecompile {
    const STATE_MUT_TABLE: &'static [([u8; 4], StateMutability)] = generate_state_mut_table! {
        IArbWasm => {
            activateProgramCall(Payable),
            stylusVersionCall(View),
            codehashVersionCall(View),
            codehashKeepaliveCall(Payable),
            codehashAsmSizeCall(View),
            programVersionCall(View),
            programInitGasCall(View),
            programMemoryFootprintCall(View),
            programTimeLeftCall(View),
            inkPriceCall(View),
            maxStackDepthCall(View),
            freePagesCall(View),
            pageGasCall(View),
            pageRampCall(View),
            pageLimitCall(View),
            minInitGasCall(View),
            initCostScalarCall(View),
            expiryDaysCall(View),
            keepaliveDaysCall(View),
            blockCacheSizeCall(View),
        }
    };

    fn inner(
        context: &mut CTX,
        input: &[u8],
        target_address: &Address,
        caller_address: Address,
        call_value: U256,
        is_static: bool,
        gas_limit: u64,
    ) -> Option<InterpreterResult> {
        let mut gas = Gas::new(gas_limit);

        let selector = selector_or_revert!(gas, input);

        match selector {
            IArbWasm::activateProgramCall::SELECTOR => {
                let call = decode_call!(gas, IArbWasm::activateProgramCall, input);

                let params = try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .programs()
                        .stylus_params()
                        .get()
                );

                try_record_cost!(gas, STYLUS_ACTIVATION_FIXED_COST);

                let code_hash = try_state!(
                    gas,
                    context.arb_state(None, is_static).code_hash(call.program)
                );

                let cached = if let Some(program_info) = try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .programs()
                        .program_info(&code_hash)
                ) {
                    let expired = program_info.age > params.expiry_days as u32 * 24 * 60 * 60;

                    // program is already activated
                    if program_info.version == params.version && !expired {
                        interpreter_revert!(gas, IArbWasm::ProgramUpToDate {}.abi_encode());
                    }

                    program_info.cached
                } else {
                    false
                };

                let bytecode = context
                    .journal_mut()
                    .code(call.program)
                    .ok()
                    .unwrap_or_default()
                    .data;

                let bytecode = match stylus_code(&bytecode) {
                    Ok(Some(code)) => code,
                    Ok(None) => {
                        interpreter_revert!(gas, IArbWasm::ProgramNotWasm {}.abi_encode());
                    }
                    Err(err) => {
                        interpreter_revert!(gas, err);
                    }
                };

                let activation_info =
                    try_or_halt!(gas, activate_program(context, code_hash, &bytecode, cached));

                let data_fee = U256::from(activation_info.data_fee);
                if call_value < data_fee {
                    interpreter_revert!(
                        gas,
                        IArbWasm::ProgramInsufficientValue {
                            have: call_value,
                            want: data_fee
                        }
                        .abi_encode()
                    );
                }

                let fee_recipient = try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .network_fee_account()
                        .get()
                );

                if let Some(error) = context
                    .journal_mut()
                    .transfer(*target_address, fee_recipient, data_fee)
                    .unwrap()
                {
                    return Some(InterpreterResult {
                        result: error.into(),
                        gas,
                        output: Bytes::default(),
                    });
                }

                let refund = call_value.saturating_sub(data_fee);
                if let Some(error) = context
                    .journal_mut()
                    .transfer(*target_address, caller_address, refund)
                    .unwrap()
                {
                    return Some(InterpreterResult {
                        result: error.into(),
                        gas,
                        output: Bytes::default(),
                    });
                }

                emit_event!(
                    context,
                    Log {
                        address: *target_address,
                        data: IArbWasm::ProgramActivated {
                            codehash: code_hash,
                            moduleHash: activation_info.module_hash,
                            program: call.program,
                            dataFee: data_fee,
                            version: activation_info.version,
                        }
                        .into_log_data()
                    },
                    gas
                );

                let output = IArbWasm::activateProgramCall::abi_encode_returns(
                    &IArbWasm::activateProgramReturn {
                        version: activation_info.version,
                        dataFee: data_fee,
                    },
                );

                interpreter_return!(gas, Bytes::from(output));
            }
            IArbWasm::stylusVersionCall::SELECTOR => {
                let params = try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .programs()
                        .stylus_params()
                        .get()
                );

                let output = IArbWasm::stylusVersionCall::abi_encode_returns(&params.version);

                interpreter_return!(gas, Bytes::from(output));
            }
            IArbWasm::codehashVersionCall::SELECTOR => {
                let call = decode_call!(gas, IArbWasm::codehashVersionCall, input);

                let params = try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .programs()
                        .stylus_params()
                        .get()
                );

                let program_info = try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .programs()
                        .get_active_program(&params, &call.codehash)
                );

                let output =
                    IArbWasm::codehashVersionCall::abi_encode_returns(&program_info.version);

                interpreter_return!(gas, Bytes::from(output));
            }
            IArbWasm::codehashKeepaliveCall::SELECTOR => {
                let call = decode_call!(gas, IArbWasm::codehashKeepaliveCall, input);

                let params = try_state!(
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
                        .get_active_program(&params, &call.codehash)
                );

                if program_info.age < params.keepalive_days as u32 * 24 * 60 * 60 {
                    interpreter_revert!(
                        gas,
                        IArbWasm::ProgramKeepaliveTooSoon {
                            ageInSeconds: program_info.age as u64
                        }
                        .abi_encode()
                    );
                }

                if program_info.version != params.version {
                    interpreter_revert!(
                        gas,
                        IArbWasm::ProgramNeedsUpgrade {
                            version: program_info.version,
                            stylusVersion: params.version,
                        }
                        .abi_encode()
                    );
                }

                let timestamp = context.block().timestamp();
                let data_fee = try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .programs()
                        .data_pricer()
                        .update(
                            program_info.asm_estimated_kb.saturating_mul(1024),
                            timestamp.saturating_to(),
                        )
                );

                if call_value < U256::from(data_fee) {
                    interpreter_revert!(
                        gas,
                        IArbWasm::ProgramInsufficientValue {
                            have: call_value,
                            want: U256::from(data_fee),
                        }
                        .abi_encode()
                    );
                }

                let fee_recipient = try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .network_fee_account()
                        .get()
                );

                if let Some(error) = context
                    .journal_mut()
                    .transfer(*target_address, fee_recipient, U256::from(data_fee))
                    .unwrap()
                {
                    return Some(InterpreterResult {
                        result: error.into(),
                        gas,
                        output: Bytes::default(),
                    });
                }

                // refund excess
                let refund = call_value.saturating_sub(U256::from(data_fee));
                if let Some(error) = context
                    .journal_mut()
                    .transfer(*target_address, caller_address, refund)
                    .unwrap()
                    && !refund.is_zero()
                {
                    return Some(InterpreterResult {
                        result: error.into(),
                        gas,
                        output: Bytes::default(),
                    });
                }

                program_info.age = 0;

                try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .programs()
                        .save_program_info(&call.codehash, &program_info)
                );

                // emit ProgramLifetimeExtended
                emit_event!(
                    context,
                    Log {
                        address: *target_address,
                        data: IArbWasm::ProgramLifetimeExtended {
                            codehash: call.codehash,
                            dataFee: U256::from(data_fee),
                        }
                        .into_log_data()
                    },
                    gas
                );

                interpreter_return!(gas);
            }
            IArbWasm::codehashAsmSizeCall::SELECTOR => {
                let call = decode_call!(gas, IArbWasm::codehashAsmSizeCall, input);

                let params = try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .programs()
                        .stylus_params()
                        .get()
                );

                let program_info = try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .programs()
                        .get_active_program(&params, &call.codehash)
                );

                let output = IArbWasm::codehashAsmSizeCall::abi_encode_returns(
                    &program_info.asm_estimated_kb,
                );

                interpreter_return!(gas, Bytes::from(output));
            }
            IArbWasm::programVersionCall::SELECTOR => {
                let call = decode_call!(gas, IArbWasm::programVersionCall, input);

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
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .code_hash(call.program)
                );

                let program_info = try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .programs()
                        .get_active_program(&params, &code_hash)
                );

                let output =
                    IArbWasm::programVersionCall::abi_encode_returns(&program_info.version);

                interpreter_return!(gas, Bytes::from(output));
            }
            IArbWasm::programInitGasCall::SELECTOR => {
                let call = decode_call!(gas, IArbWasm::programInitGasCall, input);

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
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .code_hash(call.program)
                );

                let program_info = try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .programs()
                        .get_active_program(&params, &code_hash)
                );

                let cached_gas = crate::stylus_executor::init_gas_cost(
                    program_info.cached_cost,
                    params.min_cached_init_gas,
                    params.init_cost_scalar,
                );
                let init_gas = crate::stylus_executor::init_gas_cost(
                    program_info.init_cost,
                    params.min_init_gas,
                    params.init_cost_scalar,
                );

                let output = IArbWasm::programInitGasCall::abi_encode_returns(
                    &IArbWasm::programInitGasReturn {
                        gas: init_gas,
                        gasWhenCached: cached_gas,
                    },
                );

                interpreter_return!(gas, Bytes::from(output));
            }
            IArbWasm::programMemoryFootprintCall::SELECTOR => {
                let call = decode_call!(gas, IArbWasm::programMemoryFootprintCall, input);

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
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .code_hash(call.program)
                );

                let program_info = try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .programs()
                        .get_active_program(&params, &code_hash)
                );

                let output = IArbWasm::programMemoryFootprintCall::abi_encode_returns(
                    &program_info.footprint,
                );

                interpreter_return!(gas, Bytes::from(output));
            }
            IArbWasm::programTimeLeftCall::SELECTOR => {
                let call = decode_call!(gas, IArbWasm::programTimeLeftCall, input);

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
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .code_hash(call.program)
                );

                let program_info = try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .programs()
                        .get_active_program(&params, &code_hash)
                );

                let output =
                    IArbWasm::programTimeLeftCall::abi_encode_returns(&(program_info.age as u64));

                interpreter_return!(gas, Bytes::from(output));
            }
            IArbWasm::inkPriceCall::SELECTOR => {
                let params = try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .programs()
                        .stylus_params()
                        .get()
                );

                let output = IArbWasm::inkPriceCall::abi_encode_returns(&params.ink_price);

                interpreter_return!(gas, Bytes::from(output));
            }
            IArbWasm::maxStackDepthCall::SELECTOR => {
                let params = try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .programs()
                        .stylus_params()
                        .get()
                );

                let output =
                    IArbWasm::maxStackDepthCall::abi_encode_returns(&params.max_stack_depth);

                interpreter_return!(gas, Bytes::from(output));
            }
            IArbWasm::freePagesCall::SELECTOR => {
                let params = try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .programs()
                        .stylus_params()
                        .get()
                );

                let output = IArbWasm::freePagesCall::abi_encode_returns(&params.free_pages);

                interpreter_return!(gas, Bytes::from(output));
            }
            IArbWasm::pageGasCall::SELECTOR => {
                let params = try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .programs()
                        .stylus_params()
                        .get()
                );

                let output = IArbWasm::pageGasCall::abi_encode_returns(&params.page_gas);

                interpreter_return!(gas, Bytes::from(output));
            }
            IArbWasm::pageRampCall::SELECTOR => {
                let params = try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .programs()
                        .stylus_params()
                        .get()
                );

                let output = IArbWasm::pageRampCall::abi_encode_returns(&params.page_ramp);

                interpreter_return!(gas, Bytes::from(output));
            }
            IArbWasm::pageLimitCall::SELECTOR => {
                let params = try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .programs()
                        .stylus_params()
                        .get()
                );

                let output = IArbWasm::pageLimitCall::abi_encode_returns(&params.page_limit);

                interpreter_return!(gas, Bytes::from(output));
            }
            IArbWasm::minInitGasCall::SELECTOR => {
                let params = try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .programs()
                        .stylus_params()
                        .get()
                );

                if context.cfg().arbos_version() < ARBOS_VERSION_STYLUS_CHARGING_FIXES as u16 {
                    interpreter_revert!(gas);
                }

                let output =
                    IArbWasm::minInitGasCall::abi_encode_returns(&IArbWasm::minInitGasReturn {
                        gas: params.min_init_gas as u64 * MIN_INIT_GAS_UNITS,
                        cached: params.min_cached_init_gas as u64 * MIN_CACHED_GAS_UNITS,
                    });

                interpreter_return!(gas, Bytes::from(output));
            }
            IArbWasm::initCostScalarCall::SELECTOR => {
                let params = try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .programs()
                        .stylus_params()
                        .get()
                );

                let output = IArbWasm::initCostScalarCall::abi_encode_returns(
                    &(params.init_cost_scalar as u64 * COST_SCALAR_PERCENT),
                );

                interpreter_return!(gas, Bytes::from(output));
            }
            IArbWasm::expiryDaysCall::SELECTOR => {
                let params = try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .programs()
                        .stylus_params()
                        .get()
                );

                let output = IArbWasm::expiryDaysCall::abi_encode_returns(&params.expiry_days);

                interpreter_return!(gas, Bytes::from(output));
            }
            IArbWasm::keepaliveDaysCall::SELECTOR => {
                let params = try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .programs()
                        .stylus_params()
                        .get()
                );

                let output =
                    IArbWasm::keepaliveDaysCall::abi_encode_returns(&params.keepalive_days);

                interpreter_return!(gas, Bytes::from(output));
            }
            IArbWasm::blockCacheSizeCall::SELECTOR => {
                let params = try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .programs()
                        .stylus_params()
                        .get()
                );

                let output =
                    IArbWasm::blockCacheSizeCall::abi_encode_returns(&params.block_cache_size);

                interpreter_return!(gas, Bytes::from(output));
            }
            _ => interpreter_revert!(gas, Bytes::from("Unknown function selector")),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use revm::{
        Journal,
        context::{BlockEnv, ContextTr},
        database::EmptyDBTyped,
        primitives::keccak256,
        state::Bytecode,
    };
    use stylus::brotli;
    use wasmer::wat2wasm;

    use crate::{
        ArbitrumContext, config::ArbitrumConfig, constants::STYLUS_DISCRIMINANT,
        local_context::ArbitrumLocalContext, transaction::ArbitrumTransaction,
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
    fn test_activate_codehash_version_and_asm_size() {
        use crate::state::{ArbState, ArbStateGetter, arbos_state::ArbosStateParams};

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

        // Get the code hash for later queries
        let code_hash = context
            .arb_state(None, false)
            .code_hash(program_address)
            .expect("failed to get code hash");

        // Activate the program via ArbWasm precompile
        let arb_wasm_addr = address!("0x0000000000000000000000000000000000000071");
        let call_value = U256::from(100_000_000_000_000u64);
        context
            .journal_mut()
            .balance_incr(arb_wasm_addr, call_value)
            .unwrap();

        let input = IArbWasm::activateProgramCall::abi_encode(&IArbWasm::activateProgramCall {
            program: program_address,
        });

        let result = ArbWasmPrecompile::run(
            &mut context,
            &input,
            &arb_wasm_addr,
            address!("0x000000000000000000000000000000000000c0de"),
            call_value,
            false,
            10_000_000,
        )
        .unwrap();
        assert!(result.is_ok(), "activation failed: {:?}", result.result);

        // Decode activation result to get the version
        let activation = IArbWasm::activateProgramCall::abi_decode_returns(&result.output)
            .expect("decode activation result");
        assert!(activation.version > 0, "activated version should be > 0");

        // Query codehashVersion and verify it matches the activation version
        let input = IArbWasm::codehashVersionCall::abi_encode(&IArbWasm::codehashVersionCall {
            codehash: code_hash,
        });
        let result = ArbWasmPrecompile::run(
            &mut context,
            &input,
            &arb_wasm_addr,
            address!("0x000000000000000000000000000000000000c0de"),
            U256::ZERO,
            true,
            10_000_000,
        )
        .unwrap();
        assert!(
            result.is_ok(),
            "codehashVersion failed: {:?}",
            result.result
        );
        let version = IArbWasm::codehashVersionCall::abi_decode_returns(&result.output)
            .expect("decode codehashVersion");
        assert_eq!(
            version, activation.version,
            "codehashVersion should match activation version"
        );

        // Query codehashAsmSize and verify it is non-zero
        let input = IArbWasm::codehashAsmSizeCall::abi_encode(&IArbWasm::codehashAsmSizeCall {
            codehash: code_hash,
        });
        let result = ArbWasmPrecompile::run(
            &mut context,
            &input,
            &arb_wasm_addr,
            address!("0x000000000000000000000000000000000000c0de"),
            U256::ZERO,
            true,
            10_000_000,
        )
        .unwrap();
        assert!(
            result.is_ok(),
            "codehashAsmSize failed: {:?}",
            result.result
        );
        let asm_size = IArbWasm::codehashAsmSizeCall::abi_decode_returns(&result.output)
            .expect("decode codehashAsmSize");
        assert!(asm_size > 0, "asm size should be non-zero");
    }
}
