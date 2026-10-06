use alloy_sol_types::{SolCall, SolError, sol};
use revm::{
    context::{Block, Cfg, JournalTr},
    interpreter::{Gas, InterpreterResult},
    precompile::PrecompileId,
    primitives::{
        Address, B256, Bytes, Log, U256, address, alloy_primitives::IntoLogData, keccak256,
    },
};

use crate::{
    ArbitrumContextTr,
    config::ArbitrumConfigTr,
    constants::ARBOS_VERSION_STYLUS_CONTRACT_LIMIT,
    generate_state_mut_table,
    local_context::ArbitrumLocalContextTr,
    macros::{emit_event, interpreter_return, interpreter_revert},
    precompile_impl,
    precompiles::{
        ArbPrecompileLogic, ExtendedPrecompile, StateMutability, decode_call, selector_or_revert,
    },
    state::{ArbState, ArbStateGetter, try_state, types::StorageBackedTr},
    transaction::{ArbitrumRetryTx, arbitrum_retry_tx_hash},
    try_record_cost,
};

const ARBOS_STATE_RETRYABLE_LIFETIME_SECONDS: u64 = 7 * 24 * 60 * 60; // 1 week
const RETRYABLE_REAP_PRICE: u64 = 58_000;

sol! {
///
/// @title Methods for managing retryables.
/// @notice Precompiled contract in every Arbitrum chain for retryable transaction related data retrieval and interactions. Exists at 0x000000000000000000000000000000000000006e
///
interface ArbRetryableTx {
    ///
    /// @notice Schedule an attempt to redeem a redeemable tx, donating all of the call's gas to the redeem.
    /// Revert if ticketId does not exist.
    /// @param ticketId unique identifier of retryable message: keccak256(keccak256(ArbchainId, inbox-sequence-number), uint(0) )
    /// @return txId that the redeem attempt will have
    ///
    function redeem(
        bytes32 ticketId
    ) external returns (bytes32);

    ///
    /// @notice Return the minimum lifetime of redeemable txn.
    /// @return lifetime in seconds
    ///
    function getLifetime() external view returns (uint256);

    ///
    /// @notice Return the timestamp when ticketId will age out, reverting if it does not exist
    /// @param ticketId unique ticket identifier
    /// @return timestamp for ticket's deadline
    ///
    function getTimeout(
        bytes32 ticketId
    ) external view returns (uint256);

    ///
    /// @notice Adds one lifetime period to the life of ticketId.
    /// Donate gas to pay for the lifetime extension.
    /// If successful, emits LifetimeExtended event.
    /// Revert if ticketId does not exist, or if the timeout of ticketId is already at least one lifetime period in the future.
    /// @param ticketId unique ticket identifier
    /// @return new timeout of ticketId
    ///
    function keepalive(
        bytes32 ticketId
    ) external returns (uint256);

    ///
    /// @notice Return the beneficiary of ticketId.
    /// Revert if ticketId doesn't exist.
    /// @param ticketId unique ticket identifier
    /// @return address of beneficiary for ticket
    ///
    function getBeneficiary(
        bytes32 ticketId
    ) external view returns (address);

    ///
    /// @notice Cancel ticketId and refund its callvalue to its beneficiary.
    /// Revert if ticketId doesn't exist, or if called by anyone other than ticketId's beneficiary.
    /// @param ticketId unique ticket identifier
    ///
    function cancel(
        bytes32 ticketId
    ) external;

    ///
    /// @notice Gets the redeemer of the current retryable redeem attempt.
    /// Returns the zero address if the current transaction is not a retryable redeem attempt.
    /// If this is an auto-redeem, returns the fee refund address of the retryable.
    ///
    function getCurrentRedeemer() external view returns (address);

    ///
    /// @notice Do not call. This method represents a retryable submission to aid explorers.
    /// Calling it will always revert.
    ///
    function submitRetryable(
        bytes32 requestId,
        uint256 l1BaseFee,
        uint256 deposit,
        uint256 callvalue,
        uint256 gasFeeCap,
        uint64 gasLimit,
        uint256 maxSubmissionFee,
        address feeRefundAddress,
        address beneficiary,
        address retryTo,
        bytes calldata retryData
    ) external;

    event TicketCreated(bytes32 indexed ticketId);
    event LifetimeExtended(bytes32 indexed ticketId, uint256 newTimeout);
    event RedeemScheduled(
        bytes32 indexed ticketId,
        bytes32 indexed retryTxHash,
        uint64 indexed sequenceNum,
        uint64 donatedGas,
        address gasDonor,
        uint256 maxRefund,
        uint256 submissionFeeRefund
    );
    event Canceled(bytes32 indexed ticketId);

    /// @dev DEPRECATED in favour of new RedeemScheduled event after the nitro upgrade
    event Redeemed(bytes32 indexed userTxHash);

    error NoTicketWithID();
    error NotCallable();
}

}

pub fn arb_retryable_tx_precompile<CTX: ArbitrumContextTr>() -> ExtendedPrecompile<CTX> {
    ExtendedPrecompile::new(
        PrecompileId::Custom(std::borrow::Cow::Borrowed("ArbRetryableTx")),
        address!("0x000000000000000000000000000000000000006e"),
        precompile_impl!(ArbRetryableTxPrecompile),
    )
}

struct ArbRetryableTxPrecompile;

impl<CTX: ArbitrumContextTr> ArbPrecompileLogic<CTX> for ArbRetryableTxPrecompile {
    const STATE_MUT_TABLE: &'static [([u8; 4], StateMutability)] = generate_state_mut_table! {
        ArbRetryableTx => {
            redeemCall(NonPayable),
            getLifetimeCall(View),
            getTimeoutCall(View),
            keepaliveCall(NonPayable),
            getBeneficiaryCall(View),
            cancelCall(NonPayable),
            getCurrentRedeemerCall(View),
            submitRetryableCall(NonPayable),
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
        let arbos_version = context.cfg().arbos_version();

        match selector {
            ArbRetryableTx::cancelCall::SELECTOR => {
                let call = decode_call!(gas, ArbRetryableTx::cancelCall, input);
                if context.local().current_retryable() == Some(call.ticketId) {
                    interpreter_revert!(gas, Bytes::from_static(b"retryable cannot modify itself"));
                }

                let current_time = context.block().timestamp().saturating_to::<u64>();

                let beneficiary = {
                    let mut arb_state = context.arb_state(Some(&mut gas), is_static);
                    let mut retryable = arb_state.retryable(call.ticketId);
                    let timeout = try_state!(gas, retryable.timeout().get());

                    let windows_left = if timeout != 0
                        && timeout < current_time
                        && arbos_version >= ARBOS_VERSION_STYLUS_CONTRACT_LIMIT
                    {
                        try_state!(gas, retryable.timeout_windows_left().get())
                    } else {
                        0
                    };

                    if retryable_is_expired(timeout, windows_left, current_time, arbos_version) {
                        if arbos_version >= 3 {
                            let output = ArbRetryableTx::NoTicketWithID {}.abi_encode();

                            interpreter_revert!(gas, Bytes::from(output));
                        }

                        interpreter_revert!(gas, Bytes::from("ticketId not found"));
                    }

                    try_state!(gas, retryable.beneficiary().get())
                };

                if caller_address != beneficiary {
                    interpreter_revert!(
                        gas,
                        Bytes::from("only the beneficiary may cancel a retryable")
                    );
                }

                // move any funds in escrow to the beneficiary (should be none if the retry
                // succeeded -- see EndTxHook)
                let escrow_address = { retryable_escrow_address(call.ticketId) };

                let escrow_balance = context.balance(escrow_address).unwrap_or_default().data;

                if !escrow_balance.is_zero() {
                    match context.journal_mut().transfer(
                        escrow_address,
                        beneficiary,
                        escrow_balance,
                    ) {
                        Ok(Some(error)) => {
                            return Some(InterpreterResult {
                                result: error.into(),
                                gas,
                                output: Bytes::default(),
                            });
                        }
                        Ok(None) => {}
                        Err(_) => {
                            gas.spend_all();
                            interpreter_revert!(gas, Bytes::new());
                        }
                    }
                }

                let mut arb_state = context.arb_state(Some(&mut gas), is_static);
                let mut retryable = arb_state.retryable(call.ticketId);
                try_state!(gas, retryable.clear());

                emit_event!(
                    context,
                    Log {
                        address: *target_address,
                        data: ArbRetryableTx::Canceled {
                            ticketId: call.ticketId,
                        }
                        .into_log_data(),
                    },
                    gas
                );

                let output = ArbRetryableTx::cancelCall::abi_encode_returns(
                    &ArbRetryableTx::cancelReturn {},
                );

                interpreter_return!(gas, Bytes::from(output));
            }
            ArbRetryableTx::getBeneficiaryCall::SELECTOR => {
                let call = decode_call!(gas, ArbRetryableTx::getBeneficiaryCall, input);

                let current_time = context.block().timestamp().saturating_to::<u64>();
                let beneficiary = {
                    let mut arb_state = context.arb_state(Some(&mut gas), is_static);
                    let mut retryable = arb_state.retryable(call.ticketId);
                    let timeout = try_state!(gas, retryable.timeout().get());
                    let windows_left = if timeout != 0
                        && timeout < current_time
                        && arbos_version >= ARBOS_VERSION_STYLUS_CONTRACT_LIMIT
                    {
                        try_state!(gas, retryable.timeout_windows_left().get())
                    } else {
                        0
                    };

                    if retryable_is_expired(timeout, windows_left, current_time, arbos_version) {
                        if arbos_version >= 3 {
                            let output = ArbRetryableTx::NoTicketWithID {}.abi_encode();

                            interpreter_revert!(gas, Bytes::from(output));
                        }

                        interpreter_revert!(gas, Bytes::from("ticketId not found"));
                    }

                    try_state!(gas, retryable.beneficiary().get())
                };

                let output = ArbRetryableTx::getBeneficiaryCall::abi_encode_returns(&beneficiary);

                interpreter_return!(gas, Bytes::from(output));
            }
            ArbRetryableTx::getCurrentRedeemerCall::SELECTOR => {
                let output =
                    ArbRetryableTx::getCurrentRedeemerCall::abi_encode_returns(&Address::ZERO);

                interpreter_return!(gas, Bytes::from(output));
            }
            ArbRetryableTx::getLifetimeCall::SELECTOR => {
                let output = ArbRetryableTx::getLifetimeCall::abi_encode_returns(&U256::from(
                    ARBOS_STATE_RETRYABLE_LIFETIME_SECONDS,
                ));

                interpreter_return!(gas, Bytes::from(output));
            }
            ArbRetryableTx::getTimeoutCall::SELECTOR => {
                let call = decode_call!(gas, ArbRetryableTx::getTimeoutCall, input);

                let current_time = context.block().timestamp().saturating_to::<u64>();
                let (timeout, windows_left) = {
                    let mut arb_state = context.arb_state(Some(&mut gas), is_static);
                    let mut retryable = arb_state.retryable(call.ticketId);
                    let timeout = try_state!(gas, retryable.timeout().get());
                    let windows_left = try_state!(gas, retryable.timeout_windows_left().get());
                    (timeout, windows_left)
                };

                let calculated_timeout = calculate_retryable_timeout(timeout, windows_left);
                if retryable_is_expired(timeout, windows_left, current_time, arbos_version) {
                    if arbos_version >= 3 {
                        let output = ArbRetryableTx::NoTicketWithID {}.abi_encode();

                        interpreter_revert!(gas, Bytes::from(output));
                    }

                    interpreter_revert!(gas, Bytes::from("ticketId not found"));
                }

                let output = ArbRetryableTx::getTimeoutCall::abi_encode_returns(&U256::from(
                    calculated_timeout,
                ));

                interpreter_return!(gas, Bytes::from(output));
            }
            ArbRetryableTx::keepaliveCall::SELECTOR => {
                let call = decode_call!(gas, ArbRetryableTx::keepaliveCall, input);

                let current_time = context.block().timestamp().saturating_to::<u64>();
                let (timeout, windows_left) = {
                    let mut arb_state = context.arb_state(Some(&mut gas), is_static);
                    let mut retryable = arb_state.retryable(call.ticketId);

                    let timeout = try_state!(gas, retryable.timeout().get());
                    let windows_left = try_state!(gas, retryable.timeout_windows_left().get());

                    (timeout, windows_left)
                };

                if retryable_is_expired(timeout, windows_left, current_time, arbos_version) {
                    if arbos_version >= 3 {
                        let output = ArbRetryableTx::NoTicketWithID {}.abi_encode();

                        interpreter_revert!(gas, Bytes::from(output));
                    }

                    interpreter_revert!(gas, Bytes::from("ticketId not found"));
                }

                let calldata_len = {
                    let mut arb_state = context.arb_state(Some(&mut gas), is_static);
                    try_state!(gas, arb_state.retryable(call.ticketId).calldata().size())
                };

                let nbytes = retryable_size_bytes(calldata_len);

                let update_cost = nbytes.div_ceil(32) * revm::interpreter::gas::SSTORE_SET / 100;

                try_record_cost!(gas, update_cost);

                let window = current_time.saturating_add(ARBOS_STATE_RETRYABLE_LIFETIME_SECONDS);

                let effective_timeout = calculate_retryable_timeout(timeout, windows_left);
                let new_timeout =
                    effective_timeout.saturating_add(ARBOS_STATE_RETRYABLE_LIFETIME_SECONDS);

                if effective_timeout > window {
                    interpreter_revert!(gas, Bytes::from("timeout too far into the future"));
                }

                let mut arb_state = context.arb_state(Some(&mut gas), is_static);
                try_state!(
                    gas,
                    arb_state
                        .timeout_queue()
                        .push(U256::from_be_slice(call.ticketId.as_slice()))
                );

                let mut retryable = arb_state.retryable(call.ticketId);
                try_state!(
                    gas,
                    retryable
                        .timeout_windows_left()
                        .set(windows_left.saturating_add(1))
                );

                try_record_cost!(gas, RETRYABLE_REAP_PRICE);

                emit_event!(
                    context,
                    Log {
                        address: *target_address,
                        data: ArbRetryableTx::LifetimeExtended {
                            ticketId: call.ticketId,
                            newTimeout: U256::from(new_timeout),
                        }
                        .into_log_data()
                    },
                    gas
                );

                let output =
                    ArbRetryableTx::keepaliveCall::abi_encode_returns(&U256::from(new_timeout));

                interpreter_return!(gas, Bytes::from(output));
            }
            ArbRetryableTx::redeemCall::SELECTOR => {
                let call = decode_call!(gas, ArbRetryableTx::redeemCall, input);
                if context.local().current_retryable() == Some(call.ticketId) {
                    interpreter_revert!(gas, Bytes::from_static(b"retryable cannot modify itself"));
                }

                let current_time = context.block().timestamp().saturating_to::<u64>();

                // RetryableSizeBytes first opens the ticket and reads only its
                // calldata length. Keep this separate from the second open and
                // MakeTx below because both the read ordering and gas donation
                // are consensus-visible in Nitro.
                let calldata_size = {
                    let mut arb_state = context.arb_state(Some(&mut gas), is_static);
                    let mut retryable = arb_state.retryable(call.ticketId);
                    let timeout = try_state!(gas, retryable.timeout().get());
                    let windows_left = if timeout != 0
                        && timeout < current_time
                        && arbos_version >= ARBOS_VERSION_STYLUS_CONTRACT_LIMIT
                    {
                        try_state!(gas, retryable.timeout_windows_left().get())
                    } else {
                        0
                    };
                    if retryable_is_expired(timeout, windows_left, current_time, arbos_version) {
                        if arbos_version >= 3 {
                            let output = ArbRetryableTx::NoTicketWithID {}.abi_encode();
                            interpreter_revert!(gas, Bytes::from(output));
                        }
                        interpreter_revert!(gas, Bytes::from("ticketId not found"));
                    }
                    try_state!(gas, retryable.calldata().size())
                };

                // Charge for accessing retryable storage slots.
                let byte_count = retryable_size_bytes(calldata_size);
                let write_words = byte_count.div_ceil(32);
                // Nitro deliberately uses the legacy Frontier SLOAD constant
                // (50) to price the retryable storage words copied into the
                // scheduled retry transaction.
                try_record_cost!(gas, 50u64.saturating_mul(write_words));

                {
                    let mut arb_state = context.arb_state(Some(&mut gas), is_static);
                    let mut retryable = arb_state.retryable(call.ticketId);
                    let timeout = try_state!(gas, retryable.timeout().get());
                    let windows_left = if timeout != 0
                        && timeout < current_time
                        && arbos_version >= ARBOS_VERSION_STYLUS_CONTRACT_LIMIT
                    {
                        try_state!(gas, retryable.timeout_windows_left().get())
                    } else {
                        0
                    };
                    if retryable_is_expired(timeout, windows_left, current_time, arbos_version) {
                        if arbos_version >= 3 {
                            let output = ArbRetryableTx::NoTicketWithID {}.abi_encode();
                            interpreter_revert!(gas, Bytes::from(output));
                        }
                        interpreter_revert!(gas, Bytes::from("ticketId not found"));
                    }
                }

                let nonce = {
                    let mut arb_state = context.arb_state(Some(&mut gas), is_static);
                    let mut retryable = arb_state.retryable(call.ticketId);
                    let num_tries = try_state!(gas, retryable.num_tries().get());
                    try_state!(gas, retryable.num_tries().set(num_tries.saturating_add(1)));
                    num_tries
                };

                let (from, to, callvalue, calldata) = {
                    let mut arb_state = context.arb_state(Some(&mut gas), is_static);
                    let mut retryable = arb_state.retryable(call.ticketId);
                    let from = try_state!(gas, retryable.from().get());
                    let to = try_state!(gas, retryable.to().get());
                    let callvalue = try_state!(gas, retryable.callvalue().get());
                    let calldata = Bytes::from(try_state!(gas, retryable.calldata().get()));
                    (from, to, callvalue, calldata)
                };

                let max_refund = U256::MAX;
                let submission_fee_refund = U256::ZERO;
                let backlog_update_cost = {
                    let mut arb_state = context.arb_state(None, is_static);
                    try_state!(
                        gas,
                        arb_state.l2_pricing().backlog_update_cost(arbos_version)
                    )
                };
                let event_cost = revm::interpreter::gas::LOG
                    .saturating_add(context.cfg().gas_params().log_cost(4, 4 * 32));
                let future_cost = event_cost
                    .saturating_add(revm::interpreter::gas::COPY)
                    .saturating_add(backlog_update_cost);
                if gas.remaining() < future_cost {
                    gas.spend_all();
                    return Some(crate::macros::interpreter_result_revert_out_of_gas(
                        &mut gas,
                    ));
                }
                let gas_to_donate = gas.remaining() - future_cost;
                if gas_to_donate < 21_000 {
                    interpreter_revert!(gas, Bytes::from("not enough gas to run redeem attempt"));
                }

                let retry_tx_hash = arbitrum_retry_tx_hash(
                    U256::from(context.cfg().chain_id()),
                    nonce,
                    from,
                    U256::from(context.block().basefee()),
                    gas_to_donate,
                    to,
                    callvalue,
                    &calldata,
                    call.ticketId,
                    caller_address,
                    max_refund,
                    submission_fee_refund,
                );
                let chain_id = U256::from(context.cfg().chain_id());
                let gas_fee_cap = U256::from(context.block().basefee());
                context
                    .local_mut()
                    .schedule_retry_on_commit(ArbitrumRetryTx {
                        chain_id,
                        nonce,
                        from,
                        gas_fee_cap,
                        gas_limit: gas_to_donate,
                        to,
                        value: callvalue,
                        data: calldata,
                        ticket_id: call.ticketId,
                        refund_to: caller_address,
                        max_refund,
                        submission_fee_refund,
                    });

                emit_event!(
                    context,
                    Log {
                        address: *target_address,
                        data: ArbRetryableTx::RedeemScheduled {
                            ticketId: call.ticketId,
                            retryTxHash: retry_tx_hash,
                            sequenceNum: nonce,
                            donatedGas: gas_to_donate,
                            gasDonor: caller_address,
                            maxRefund: max_refund,
                            submissionFeeRefund: submission_fee_refund,
                        }
                        .into_log_data()
                    },
                    gas
                );

                // The outer redeem donates this gas to the scheduled retry.
                // It is removed from the current pricing backlog because the
                // retry execution will add back only what it actually uses.
                try_record_cost!(gas, gas_to_donate);
                if arbos_version >= 60 {
                    try_record_cost!(gas, backlog_update_cost);
                    let mut arb_state = context.arb_state(None, is_static);
                    try_state!(
                        gas,
                        arb_state
                            .l2_pricing()
                            .shrink_backlog(gas_to_donate, arbos_version)
                    );
                } else {
                    let mut arb_state = context.arb_state(Some(&mut gas), is_static);
                    try_state!(
                        gas,
                        arb_state
                            .l2_pricing()
                            .shrink_backlog(gas_to_donate, arbos_version)
                    );
                }

                let output = ArbRetryableTx::redeemCall::abi_encode_returns(&retry_tx_hash);

                interpreter_return!(gas, Bytes::from(output));
            }
            ArbRetryableTx::submitRetryableCall::SELECTOR => {
                let _ = decode_call!(gas, ArbRetryableTx::submitRetryableCall, input);

                let output = ArbRetryableTx::NotCallable {}.abi_encode();

                interpreter_revert!(gas, Bytes::from(output));
            }
            _ => interpreter_revert!(gas, Bytes::from("Unknown function selector")),
        }
    }
}

fn retryable_size_bytes(calldata_length: u64) -> u64 {
    let calldata_words = calldata_length.div_ceil(32);
    // 6 storage words for the fixed fields plus a length word and the calldata contents.
    6 * 32 + 32 + 32 * calldata_words
}

fn calculate_retryable_timeout(timeout: u64, windows_left: u64) -> u64 {
    timeout.saturating_add(windows_left.saturating_mul(ARBOS_STATE_RETRYABLE_LIFETIME_SECONDS))
}

/// Nitro only consults queued lifetime-extension windows after the raw timeout
/// has elapsed, and only from ArbOS 60 onward. This preserves the historical
/// pre-v60 behavior and its cheaper live-ticket read path.
fn retryable_is_expired(
    timeout: u64,
    windows_left: u64,
    current_time: u64,
    arbos_version: u64,
) -> bool {
    timeout == 0
        || (timeout < current_time
            && (arbos_version < ARBOS_VERSION_STYLUS_CONTRACT_LIMIT
                || calculate_retryable_timeout(timeout, windows_left) < current_time))
}

fn retryable_escrow_address(ticket_id: B256) -> Address {
    let mut hasher_input = Vec::with_capacity(32 + "retryable escrow".len());
    hasher_input.extend_from_slice(b"retryable escrow");
    hasher_input.extend_from_slice(ticket_id.as_ref());

    let hash = keccak256(&hasher_input);
    Address::from_slice(&hash[12..32])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arbos_60_honors_unconsumed_keepalive_windows_after_raw_timeout() {
        let raw_timeout = 1_000;
        let now = raw_timeout + ARBOS_STATE_RETRYABLE_LIFETIME_SECONDS / 2;

        assert!(retryable_is_expired(raw_timeout, 1, now, 59));
        assert!(!retryable_is_expired(raw_timeout, 1, now, 60));
        assert!(retryable_is_expired(
            raw_timeout,
            1,
            raw_timeout + ARBOS_STATE_RETRYABLE_LIFETIME_SECONDS + 1,
            60,
        ));
    }

    #[test]
    fn zero_timeout_never_identifies_a_ticket() {
        assert!(retryable_is_expired(0, u64::MAX, 0, 60));
    }
}
