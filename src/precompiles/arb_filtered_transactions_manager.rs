use alloy_sol_types::{SolCall, sol};
use revm::{
    context::JournalTr,
    interpreter::{Gas, InterpreterResult},
    precompile::PrecompileId,
    primitives::{Address, B256, Bytes, Log, U256, address, alloy_primitives::IntoLogData},
};

use crate::{
    ArbitrumContextTr,
    config::ArbitrumConfigTr,
    generate_state_mut_table,
    macros::{emit_event, interpreter_return, interpreter_revert},
    precompile_impl,
    precompiles::{
        ArbPrecompileLogic, ExtendedPrecompile, StateMutability, decode_call, selector_or_revert,
    },
    state::{ArbState, ArbStateGetter, try_state},
};

sol! {
    interface ArbFilteredTransactionsManager {
        function addFilteredTransaction(bytes32 txHash) external;
        function deleteFilteredTransaction(bytes32 txHash) external;
        function isTransactionFiltered(bytes32 txHash) external view returns (bool);

        event FilteredTransactionAdded(bytes32 indexed txHash);
        event FilteredTransactionDeleted(bytes32 indexed txHash);
    }
}

pub fn arb_filtered_transactions_manager_precompile<CTX: ArbitrumContextTr>()
-> ExtendedPrecompile<CTX> {
    ExtendedPrecompile::new(
        PrecompileId::Custom(std::borrow::Cow::Borrowed("ArbFilteredTransactionsManager")),
        address!("0x0000000000000000000000000000000000000074"),
        precompile_impl!(ArbFilteredTransactionsManagerPrecompile),
    )
}

struct ArbFilteredTransactionsManagerPrecompile;

impl<CTX: ArbitrumContextTr> ArbPrecompileLogic<CTX> for ArbFilteredTransactionsManagerPrecompile {
    const FREE_FOR_FILTERERS: bool = true;
    const MIN_ARBOS_VERSION: u64 = 60;

    const STATE_MUT_TABLE: &'static [([u8; 4], StateMutability)] = generate_state_mut_table! {
        ArbFilteredTransactionsManager => {
            addFilteredTransactionCall(NonPayable),
            deleteFilteredTransactionCall(NonPayable),
            isTransactionFilteredCall(View),
        }
    };

    fn inner(
        context: &mut CTX,
        input: &[u8],
        _target_address: &Address,
        caller_address: Address,
        _call_value: U256,
        is_static: bool,
        gas_limit: u64,
    ) -> Option<InterpreterResult> {
        let mut gas = Gas::new(gas_limit);
        let selector = selector_or_revert!(gas, input);

        if context.cfg().arbos_version() < 60 {
            interpreter_revert!(gas, Bytes::from("precompile requires ArbOS 60"));
        }

        match selector {
            ArbFilteredTransactionsManager::addFilteredTransactionCall::SELECTOR => {
                let call = decode_call!(
                    gas,
                    ArbFilteredTransactionsManager::addFilteredTransactionCall,
                    input
                );
                let authorized = try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), true)
                        .transaction_filterers()
                        .contains(caller_address)
                );
                if !authorized {
                    gas.spend_all();
                    interpreter_revert!(gas, Bytes::from("unauthorized transaction filterer"));
                }
                let tx_hash = B256::from(call.txHash);
                try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .filtered_transactions()
                        .add(tx_hash)
                );
                emit_event!(
                    context,
                    Log {
                        address: address!("0x0000000000000000000000000000000000000074"),
                        data: ArbFilteredTransactionsManager::FilteredTransactionAdded {
                            txHash: call.txHash,
                        }
                        .into_log_data(),
                    },
                    gas
                );
                interpreter_return!(gas, Bytes::new());
            }
            ArbFilteredTransactionsManager::deleteFilteredTransactionCall::SELECTOR => {
                let call = decode_call!(
                    gas,
                    ArbFilteredTransactionsManager::deleteFilteredTransactionCall,
                    input
                );
                let authorized = try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), true)
                        .transaction_filterers()
                        .contains(caller_address)
                );
                if !authorized {
                    gas.spend_all();
                    interpreter_revert!(gas, Bytes::from("unauthorized transaction filterer"));
                }
                let tx_hash = B256::from(call.txHash);
                try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .filtered_transactions()
                        .delete(tx_hash)
                );
                emit_event!(
                    context,
                    Log {
                        address: address!("0x0000000000000000000000000000000000000074"),
                        data: ArbFilteredTransactionsManager::FilteredTransactionDeleted {
                            txHash: call.txHash,
                        }
                        .into_log_data(),
                    },
                    gas
                );
                interpreter_return!(gas, Bytes::new());
            }
            ArbFilteredTransactionsManager::isTransactionFilteredCall::SELECTOR => {
                let call = decode_call!(
                    gas,
                    ArbFilteredTransactionsManager::isTransactionFilteredCall,
                    input
                );
                let filtered = try_state!(
                    gas,
                    context
                        .arb_state(Some(&mut gas), is_static)
                        .filtered_transactions()
                        .is_filtered(B256::from(call.txHash))
                );
                let output =
                    ArbFilteredTransactionsManager::isTransactionFilteredCall::abi_encode_returns(
                        &filtered,
                    );
                interpreter_return!(gas, Bytes::from(output));
            }
            _ => interpreter_revert!(gas, Bytes::from("Unknown selector")),
        }
    }
}
