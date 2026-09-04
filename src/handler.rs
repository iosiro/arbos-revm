use std::ops::Deref;

use alloy_sol_types::{SolCall, sol};

use crate::{
    config::ArbitrumConfigTr,
    constants::{
        ARBITRUM_CONTRACT_TX_TYPE, ARBITRUM_DEPOSIT_TX_TYPE, ARBITRUM_INTERNAL_TX_TYPE,
        ARBITRUM_RETRY_TX_TYPE, ARBITRUM_SUBMIT_RETRYABLE_TX_TYPE, ARBITRUM_UNSIGNED_TX_TYPE,
        ARBOS_ADDRESS, ARBOS_L1_PRICER_FUNDS_ADDRESS, HISTORY_SERVE_WINDOW,
        HISTORY_STORAGE_ADDRESS,
    },
    context::ArbitrumContextMutTr,
    l1_fee,
    local_context::ArbitrumLocalContextTr,
    state::{ArbState, ArbStateGetter, types::StorageBackedTr},
    transaction::{
        ArbitrumInternalTx, ArbitrumTxTr, arbitrum_retry_tx_hash, arbitrum_submit_retryable_tx_hash,
    },
};

sol! {
    function startBlock(
        uint256 l1BaseFee,
        uint64 l1BlockNumber,
        uint64 l2BlockNumber,
        uint64 timeLastBlock
    );

    function batchPostingReport(
        uint256 batchTimestamp,
        address batchPosterAddress,
        uint64 batchNumber,
        uint64 batchDataGas,
        uint256 l1BaseFeeWei
    );

    function batchPostingReportV2(
        uint256 batchTimestamp,
        address batchPosterAddress,
        uint64 batchNumber,
        uint64 batchCalldataLength,
        uint64 batchCalldataNonZeros,
        uint64 batchExtraGas,
        uint256 l1BaseFeeWei
    );

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
        bytes retryData
    );

    event TicketCreated(bytes32 indexed ticketId);
    event RedeemScheduled(
        bytes32 indexed ticketId,
        bytes32 indexed retryTxHash,
        uint64 indexed sequenceNum,
        uint64 donatedGas,
        address gasDonor,
        uint256 maxRefund,
        uint256 submissionFeeRefund
    );
}
use revm::{
    Inspector,
    context::{
        Block, Cfg, ContextTr, Host, JournalTr, LocalContextTr, Transaction,
        result::{ExecutionResult, FromStringError, HaltReason, InvalidTransaction, SuccessReason},
    },
    handler::{
        EthFrame, EvmTr, FrameResult, FrameTr, Handler, MainnetHandler,
        handler::EvmTrError,
        pre_execution::{calculate_caller_fee, validate_account_nonce_and_code_with_components},
    },
    inspector::{InspectorEvmTr, InspectorHandler},
    interpreter::{Gas, InitialAndFloorGas, interpreter::EthInterpreter},
    primitives::{
        Address, Bytes, Log, TxKind, U256, address, alloy_primitives::IntoLogData, keccak256,
    },
    state::EvmState,
};

pub struct ArbitrumHandler<EVM, ERROR, FRAME> {
    /// Mainnet handler allows us to use functions from the mainnet handler inside Arbitrum
    /// handler. So we dont duplicate the logic
    pub mainnet: MainnetHandler<EVM, ERROR, FRAME>,
    /// Phantom data to avoid type inference issues.
    pub _phantom: core::marker::PhantomData<(EVM, ERROR, FRAME)>,
}

impl<EVM, ERROR, FRAME> ArbitrumHandler<EVM, ERROR, FRAME> {
    pub fn new() -> Self {
        Self {
            mainnet: MainnetHandler::default(),
            _phantom: core::marker::PhantomData,
        }
    }
}

impl<EVM, ERROR, FRAME> Default for ArbitrumHandler<EVM, ERROR, FRAME> {
    fn default() -> Self {
        Self::new()
    }
}

impl<EVM, ERROR> ArbitrumHandler<EVM, ERROR, EthFrame<EthInterpreter>>
where
    EVM: EvmTr<
            Context: ArbitrumContextMutTr<Journal: JournalTr<State = EvmState>>,
            Frame = EthFrame<EthInterpreter>,
        >,
    ERROR: EvmTrError<EVM> + FromStringError,
{
    /// Executes an Arbitrum deposit transaction.
    ///
    /// Deposit transactions mint ETH from L1 to L2:
    /// 1. Mint the value to the sender's account
    /// 2. Transfer the value from sender to recipient
    /// 3. Return success with 0 gas used
    fn execute_deposit_tx(&mut self, evm: &mut EVM) -> Result<ExecutionResult<HaltReason>, ERROR> {
        let ctx = evm.ctx();
        let caller = ctx.tx().caller();
        let value = ctx.tx().value();
        let to = match ctx.tx().kind() {
            TxKind::Call(addr) => addr,
            TxKind::Create => {
                // Deposit transactions should always have a recipient
                return Err(ERROR::from_string(
                    "deposit transaction must have a recipient".to_string(),
                ));
            }
        };

        // Mint ETH to the caller (from L1), then transfer to recipient
        // This mimics nitro's behavior: MintBalance(&from, value) then Transfer(from, *to, value)
        ctx.journal_mut().balance_incr(caller, value)?;

        // Transfer from caller to recipient (if different)
        if caller != to {
            // The transfer method handles both deduction and increment
            if let Some(transfer_err) = ctx.journal_mut().transfer(caller, to, value)? {
                return Err(ERROR::from_string(format!(
                    "deposit transfer failed: {:?}",
                    transfer_err
                )));
            }
        }

        // Commit the transaction
        ctx.journal_mut().commit_tx();
        ctx.local_mut().clear();
        evm.frame_stack().clear();

        // Return success with 0 gas used
        Ok(ExecutionResult::Success {
            reason: SuccessReason::Stop,
            gas_used: 0,
            gas_refunded: 0,
            output: revm::context::result::Output::Call(Bytes::new()),
            logs: Vec::new(),
        })
    }

    /// Executes an Arbitrum internal transaction.
    ///
    /// Internal transactions are sent by ArbOS to update system state:
    /// - Update L1 pricing
    /// - Update block info (timestamps, etc.)
    /// - Reap expired retryables
    ///
    fn execute_internal_tx(&mut self, evm: &mut EVM) -> Result<ExecutionResult<HaltReason>, ERROR> {
        let ctx = evm.ctx();
        let caller = ctx.tx().caller();

        // Internal transactions must be from ArbOS
        if caller != ARBOS_ADDRESS {
            return Err(ERROR::from_string(
                "internal transaction must be from ArbOS address".to_string(),
            ));
        }

        let input = ctx.tx().input().clone();
        let selector = input.get(..4).ok_or_else(|| {
            ERROR::from_string(format!(
                "internal tx data is too short (only {} bytes, at least 4 required)",
                input.len()
            ))
        })?;
        let current_timestamp = ctx.block().timestamp().saturating_to::<u64>();
        let arbos_version = ctx
            .arb_state(None, false)
            .arbos_version()
            .get()
            .map_err(|err| ERROR::from_string(err.to_string()))?;

        if selector == ArbitrumInternalTx::START_BLOCK_METHOD {
            let call = startBlockCall::abi_decode(&input)
                .map_err(|err| ERROR::from_string(format!("invalid startBlock calldata: {err}")))?;
            let block_number = ctx.block().number().saturating_to::<u64>();
            let previous_hash = if block_number == 0 {
                Default::default()
            } else {
                ctx.block_hash(block_number - 1).unwrap_or_default()
            };
            if arbos_version >= 40 && block_number > 0 {
                // EIP-2935's history contract stores the parent hash in its
                // ring buffer at `(block.number - 1) % HISTORY_SERVE_WINDOW`.
                // This is the state transition performed by Nitro's
                // ProcessParentBlockHash system call.
                ctx.journal_mut()
                    .load_account(HISTORY_STORAGE_ADDRESS)
                    .map_err(|err| ERROR::from_string(err.to_string()))?;
                ctx.journal_mut()
                    .sstore(
                        HISTORY_STORAGE_ADDRESS,
                        U256::from((block_number - 1) % HISTORY_SERVE_WINDOW),
                        U256::from_be_bytes(previous_hash.0),
                    )
                    .map_err(|err| ERROR::from_string(err.to_string()))?;
            }
            let old_l1_block = ctx
                .arb_state(None, false)
                .blockhashes()
                .l1_block_number()
                .get()
                .map_err(|err| ERROR::from_string(err.to_string()))?;
            if call.l1BlockNumber > old_l1_block {
                ctx.arb_state(None, false)
                    .blockhashes()
                    .record_new_l1_block(call.l1BlockNumber - 1, previous_hash, arbos_version)
                    .map_err(|err| ERROR::from_string(err.to_string()))?;
            }
            for _ in 0..2 {
                let _ = ctx
                    .arb_state(None, false)
                    .retryable_state()
                    .try_reap_one(current_timestamp);
            }
            ctx.arb_state(None, false)
                .l2_pricing()
                .update_pricing_model(call.timeLastBlock, arbos_version)
                .map_err(|err| ERROR::from_string(err.to_string()))?;
            let upgraded = ctx
                .arb_state(None, false)
                .upgrade_arbos_version_if_necessary(current_timestamp)
                .map_err(|err| ERROR::from_string(err.to_string()))?;
            if upgraded {
                let new_version = ctx
                    .arb_state(None, false)
                    .arbos_version()
                    .get()
                    .map_err(|err| ERROR::from_string(err.to_string()))?;
                ctx.set_live_arbos_version(new_version);
            }
        } else if selector == ArbitrumInternalTx::BATCH_POSTING_REPORT_METHOD {
            let call = batchPostingReportCall::abi_decode(&input).map_err(|err| {
                ERROR::from_string(format!("invalid batchPostingReport calldata: {err}"))
            })?;
            let per_batch_gas = ctx
                .arb_state(None, false)
                .l1_pricing()
                .per_batch_gas_cost()
                .get()
                .unwrap_or_default();
            let gas_spent = per_batch_gas
                .saturating_add(call.batchDataGas.min(i64::MAX as u64) as i64)
                .max(0) as u64;
            let wei_spent = call.l1BaseFeeWei.saturating_mul(U256::from(gas_spent));
            // Nitro deliberately logs and swallows accounting errors here so a
            // malformed/out-of-order report cannot invalidate the L2 block.
            let _ = ctx
                .arb_state(None, false)
                .l1_pricing()
                .update_for_batch_poster_spending(
                    arbos_version,
                    call.batchTimestamp.saturating_to::<u64>(),
                    current_timestamp,
                    call.batchPosterAddress,
                    wei_spent,
                    call.l1BaseFeeWei,
                );
        } else if selector == ArbitrumInternalTx::BATCH_POSTING_REPORT_V2_METHOD {
            let call = batchPostingReportV2Call::abi_decode(&input).map_err(|err| {
                ERROR::from_string(format!("invalid batchPostingReportV2 calldata: {err}"))
            })?;
            // Nitro's LegacyCostForStats includes calldata, one keccak, and
            // two SSTORE_SET operations used by the batch statistics path.
            let zero_bytes = call
                .batchCalldataLength
                .wrapping_sub(call.batchCalldataNonZeros);
            let keccak_words = call.batchCalldataLength.saturating_add(31) / 32;
            let mut gas_spent = zero_bytes
                .saturating_mul(4)
                .saturating_add(call.batchCalldataNonZeros.saturating_mul(16))
                .saturating_add(30)
                .saturating_add(keccak_words.saturating_mul(6))
                .saturating_add(40_000)
                .saturating_add(call.batchExtraGas);
            let per_batch_gas = ctx
                .arb_state(None, false)
                .l1_pricing()
                .per_batch_gas_cost()
                .get()
                .unwrap_or_default();
            gas_spent = per_batch_gas
                .saturating_add(gas_spent.min(i64::MAX as u64) as i64)
                .max(0) as u64;
            if arbos_version >= 50 {
                const FLOOR_GAS_ADDITIONAL_TOKENS: u64 = 172;
                const TX_GAS: u64 = 21_000;
                let floor_per_token = ctx
                    .arb_state(None, false)
                    .l1_pricing()
                    .gas_floor_per_token()
                    .get()
                    .unwrap_or_default();
                let tokens = call
                    .batchCalldataLength
                    .saturating_add(call.batchCalldataNonZeros.saturating_mul(3))
                    .saturating_add(FLOOR_GAS_ADDITIONAL_TOKENS);
                gas_spent = gas_spent.max(
                    floor_per_token
                        .saturating_mul(tokens)
                        .saturating_add(TX_GAS),
                );
            }
            let wei_spent = call.l1BaseFeeWei.saturating_mul(U256::from(gas_spent));
            let _ = ctx
                .arb_state(None, false)
                .l1_pricing()
                .update_for_batch_poster_spending(
                    arbos_version,
                    call.batchTimestamp.saturating_to::<u64>(),
                    current_timestamp,
                    call.batchPosterAddress,
                    wei_spent,
                    call.l1BaseFeeWei,
                );
        } else {
            return Err(ERROR::from_string(format!(
                "unknown internal tx method selector: {}",
                alloy_primitives_hex(selector)
            )));
        }

        // Commit the transaction
        ctx.journal_mut().commit_tx();
        ctx.local_mut().clear();
        evm.frame_stack().clear();

        // Return success with 0 gas used
        Ok(ExecutionResult::Success {
            reason: SuccessReason::Stop,
            gas_used: 0,
            gas_refunded: 0,
            output: revm::context::result::Output::Call(Bytes::new()),
            logs: Vec::new(),
        })
    }

    /// Executes an Arbitrum submit retryable transaction.
    ///
    /// Submit retryable transactions create a retryable ticket for L1->L2 message passing:
    /// 1. Mint the deposit value to the caller (L1 -> L2 bridging)
    /// 2. Execute the call to ArbRetryableTx precompile to create the ticket
    fn execute_submit_retryable(
        &mut self,
        evm: &mut EVM,
    ) -> Result<ExecutionResult<HaltReason>, ERROR> {
        let ctx = evm.ctx();
        let caller = ctx.tx().caller();
        let call = submitRetryableCall::abi_decode(ctx.tx().input()).map_err(|err| {
            ERROR::from_string(format!("invalid submit-retryable calldata: {err}"))
        })?;
        let retry_to = (!call.retryTo.is_zero()).then_some(call.retryTo);
        let ticket_id = arbitrum_submit_retryable_tx_hash(
            U256::from(ctx.cfg().chain_id()),
            call.requestId,
            caller,
            call.l1BaseFee,
            call.deposit,
            call.gasFeeCap,
            call.gasLimit,
            retry_to,
            call.callvalue,
            call.beneficiary,
            call.maxSubmissionFee,
            call.feeRefundAddress,
            &call.retryData,
        );
        let arbos_version = ctx
            .arb_state(None, false)
            .arbos_version()
            .get()
            .map_err(|err| ERROR::from_string(err.to_string()))?;
        let is_filtered = arbos_version >= 60
            && ctx
                .arb_state(None, false)
                .filtered_transactions()
                .is_filtered(ticket_id)
                .map_err(|err| ERROR::from_string(err.to_string()))?;
        let filtered_recipient = if is_filtered {
            Some(
                ctx.arb_state(None, false)
                    .filtered_funds_recipient_or_default()
                    .map_err(|err| ERROR::from_string(err.to_string()))?,
            )
        } else {
            None
        };
        let fee_refund_address = filtered_recipient.unwrap_or(call.feeRefundAddress);
        let beneficiary = filtered_recipient.unwrap_or(call.beneficiary);
        let retryable_address = address!("000000000000000000000000000000000000006e");
        let network_account = ctx
            .arb_state(None, false)
            .network_fee_account()
            .get()
            .map_err(|err| ERROR::from_string(err.to_string()))?;

        ctx.journal_mut().balance_incr(caller, call.deposit)?;
        let balance_after_mint = ctx.balance(caller).unwrap_or_default().data;
        if balance_after_mint < call.maxSubmissionFee {
            return Err(ERROR::from_string(format!(
                "insufficient funds for max submission fee: have {} want {}",
                balance_after_mint, call.maxSubmissionFee
            )));
        }
        let submission_fee = call.l1BaseFee.saturating_mul(U256::from(
            1_400u64.saturating_add(6u64.saturating_mul(call.retryData.len() as u64)),
        ));
        if call.maxSubmissionFee < submission_fee {
            return Err(ERROR::from_string(format!(
                "max submission fee {} is less than actual submission fee {}",
                call.maxSubmissionFee, submission_fee
            )));
        }
        transfer_or_error::<EVM, ERROR>(ctx, caller, network_account, submission_fee)?;

        let mut available_refund = call.deposit.saturating_sub(call.callvalue);
        let withheld_submission_fee = take_funds(&mut available_refund, submission_fee);
        let submission_fee_refund = take_funds(
            &mut available_refund,
            call.maxSubmissionFee.saturating_sub(submission_fee),
        );
        transfer_or_error::<EVM, ERROR>(ctx, caller, fee_refund_address, submission_fee_refund)?;

        let escrow = retryable_escrow_address(ticket_id);
        if let Err(callvalue_err) =
            transfer_or_error::<EVM, ERROR>(ctx, caller, escrow, call.callvalue)
        {
            // Nitro retains the deposit transaction but compensates the
            // submission charge when callvalue cannot be escrowed.
            transfer_or_error::<EVM, ERROR>(ctx, network_account, caller, submission_fee)?;
            transfer_or_error::<EVM, ERROR>(
                ctx,
                caller,
                fee_refund_address,
                withheld_submission_fee,
            )?;
            return Err(callvalue_err);
        }
        let timeout = ctx
            .block()
            .timestamp()
            .saturating_to::<u64>()
            .saturating_add(7 * 24 * 60 * 60);
        ctx.arb_state(None, false)
            .retryable_state()
            .create_retryable(
                ticket_id,
                timeout,
                caller,
                retry_to,
                call.callvalue,
                beneficiary,
                &call.retryData,
            )
            .map_err(|err| ERROR::from_string(err.to_string()))?;

        let mut logs = vec![Log {
            address: retryable_address,
            data: TicketCreated {
                ticketId: ticket_id,
            }
            .to_log_data(),
        }];

        let base_fee = U256::from(ctx.block().basefee());
        let max_gas_cost = call.gasFeeCap.saturating_mul(U256::from(call.gasLimit));
        let caller_balance = ctx.balance(caller).unwrap_or_default().data;
        let can_pay_for_redeem =
            caller_balance >= max_gas_cost && call.gasLimit >= 21_000 && call.gasFeeCap >= base_fee;
        if can_pay_for_redeem {
            let gas_cost = base_fee.saturating_mul(U256::from(call.gasLimit));
            let infra_account = ctx
                .arb_state(None, false)
                .infra_fee_account()
                .get()
                .map_err(|err| ERROR::from_string(err.to_string()))?;
            let min_base_fee = ctx
                .arb_state(None, false)
                .l2_pricing()
                .min_base_fee_wei()
                .get()
                .map_err(|err| ERROR::from_string(err.to_string()))?;
            let infra_cost = if arbos_version < 11 || infra_account.is_zero() {
                U256::ZERO
            } else {
                min_base_fee
                    .min(base_fee)
                    .saturating_mul(U256::from(call.gasLimit))
                    .min(gas_cost)
            };
            transfer_or_error::<EVM, ERROR>(ctx, caller, infra_account, infra_cost)?;
            transfer_or_error::<EVM, ERROR>(
                ctx,
                caller,
                network_account,
                gas_cost.saturating_sub(infra_cost),
            )?;
            let withheld_gas_funds = take_funds(&mut available_refund, gas_cost);
            let gas_price_refund = take_funds(
                &mut available_refund,
                call.gasFeeCap
                    .saturating_sub(base_fee)
                    .saturating_mul(U256::from(call.gasLimit)),
            );
            transfer_or_error::<EVM, ERROR>(ctx, caller, fee_refund_address, gas_price_refund)?;
            available_refund = available_refund
                .saturating_add(withheld_gas_funds)
                .saturating_add(withheld_submission_fee);
            if !is_filtered {
                ctx.arb_state(None, false)
                    .retryable(ticket_id)
                    .num_tries()
                    .set(1)
                    .map_err(|err| ERROR::from_string(err.to_string()))?;
                let retry_hash = arbitrum_retry_tx_hash(
                    U256::from(ctx.cfg().chain_id()),
                    0,
                    caller,
                    base_fee,
                    call.gasLimit,
                    retry_to,
                    call.callvalue,
                    &call.retryData,
                    ticket_id,
                    fee_refund_address,
                    available_refund,
                    submission_fee,
                );
                logs.push(Log {
                    address: retryable_address,
                    data: RedeemScheduled {
                        ticketId: ticket_id,
                        retryTxHash: retry_hash,
                        sequenceNum: 0,
                        donatedGas: call.gasLimit,
                        gasDonor: fee_refund_address,
                        maxRefund: available_refund,
                        submissionFeeRefund: submission_fee,
                    }
                    .to_log_data(),
                });
            }
        } else {
            let gas_refund = take_funds(&mut available_refund, max_gas_cost);
            transfer_or_error::<EVM, ERROR>(ctx, caller, fee_refund_address, gas_refund)?;
        }

        ctx.journal_mut().commit_tx();
        ctx.local_mut().clear();
        evm.frame_stack().clear();
        Ok(ExecutionResult::Success {
            reason: SuccessReason::Stop,
            gas_used: if can_pay_for_redeem { call.gasLimit } else { 0 },
            gas_refunded: 0,
            output: revm::context::result::Output::Call(Bytes::copy_from_slice(
                ticket_id.as_slice(),
            )),
            logs,
        })
    }

    /// Finish an ArbOS system transaction that returned an execution-level
    /// error. Nitro's start hook returns these with `endTxNow = true`, so any
    /// state changes (including explicit compensation) are retained while the
    /// receipt is marked failed.
    fn commit_system_failure(&self, evm: &mut EVM) -> ExecutionResult<HaltReason> {
        evm.ctx().journal_mut().commit_tx();
        evm.ctx().local_mut().clear();
        evm.frame_stack().clear();
        ExecutionResult::Revert {
            gas_used: 0,
            output: Bytes::new(),
        }
    }
}

fn take_funds(available: &mut U256, requested: U256) -> U256 {
    let taken = (*available).min(requested);
    *available = available.saturating_sub(taken);
    taken
}

fn retryable_escrow_address(ticket_id: revm::primitives::B256) -> Address {
    let mut input = b"retryable escrow".to_vec();
    input.extend_from_slice(ticket_id.as_slice());
    Address::from_slice(&keccak256(input)[12..])
}

fn transfer_or_error<EVM, ERROR>(
    ctx: &mut EVM::Context,
    from: Address,
    to: Address,
    amount: U256,
) -> Result<(), ERROR>
where
    EVM: EvmTr<Context: ArbitrumContextMutTr<Journal: JournalTr<State = EvmState>>>,
    ERROR: EvmTrError<EVM> + FromStringError,
{
    if amount.is_zero() {
        return Ok(());
    }
    if let Some(err) = ctx.journal_mut().transfer(from, to, amount)? {
        return Err(ERROR::from_string(format!(
            "balance transfer failed: {err:?}"
        )));
    }
    Ok(())
}

fn alloy_primitives_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(HEX[(byte >> 4) as usize] as char);
        result.push(HEX[(byte & 0x0f) as usize] as char);
    }
    result
}

impl<EVM, ERROR> Handler for ArbitrumHandler<EVM, ERROR, EthFrame<EthInterpreter>>
where
    EVM: EvmTr<
            Context: ArbitrumContextMutTr<Journal: JournalTr<State = EvmState>>,
            Frame = EthFrame<EthInterpreter>,
        >,
    ERROR: EvmTrError<EVM> + FromStringError,
{
    type Evm = EVM;
    type Error = ERROR;
    type HaltReason = HaltReason;

    /// Main entry point for transaction execution.
    ///
    /// Overrides the default to handle Arbitrum system transactions (deposit, internal)
    /// which skip normal EVM execution and have special semantics.
    fn run(
        &mut self,
        evm: &mut Self::Evm,
    ) -> Result<ExecutionResult<Self::HaltReason>, Self::Error> {
        // Persisted ArbOS state is authoritative. Foundry may reuse an EVM
        // context across transactions, including across a scheduled upgrade.
        let persisted_version = evm
            .ctx()
            .arb_state(None, false)
            .arbos_version()
            .get()
            .map_err(|err| ERROR::from_string(err.to_string()))?;
        if persisted_version != 0 {
            evm.ctx().set_live_arbos_version(persisted_version);
        }
        let tx_type = evm.ctx().tx().tx_type();

        match tx_type {
            ARBITRUM_DEPOSIT_TX_TYPE => match self.execute_deposit_tx(evm) {
                Ok(result) => Ok(result),
                Err(_) => Ok(self.commit_system_failure(evm)),
            },
            ARBITRUM_INTERNAL_TX_TYPE => match self.execute_internal_tx(evm) {
                Ok(result) => Ok(result),
                Err(_) => Ok(self.commit_system_failure(evm)),
            },
            ARBITRUM_SUBMIT_RETRYABLE_TX_TYPE => match self.execute_submit_retryable(evm) {
                Ok(result) => Ok(result),
                Err(_) => Ok(self.commit_system_failure(evm)),
            },
            _ => {
                let collect_tips =
                    {
                        let version = evm.ctx().cfg().arbos_version();
                        version == 9
                            || (version >= 60
                                && evm.ctx().arb_state(None, false).collect_tips().map_err(
                                    |err| ERROR::from_string(format!("collect tips: {err}")),
                                )?)
                    };
                if !collect_tips {
                    let base_fee = evm.ctx().block().basefee() as u128;
                    evm.ctx().drop_transaction_tip(base_fee);
                }
                // Standard transaction - use normal execution flow
                match self.run_without_catch_error(evm) {
                    Ok(output) => Ok(output),
                    Err(e) => self.catch_error(evm, e),
                }
            }
        }
    }

    /// Validates caller state, calculates L1 fee, and deducts both L1 and L2 fees from caller's balance.
    ///
    /// This method:
    /// 1. Validates account nonce and code
    /// 2. Reads L1 pricing from ArbOS state
    /// 3. Calculates the L1 data fee based on transaction bytes
    /// 4. Deducts both the L1 fee and the standard L2 gas fee from the caller
    /// 5. Caches the L1 cost in local context for later distribution
    fn validate_against_state_and_deduct_caller(
        &self,
        evm: &mut Self::Evm,
    ) -> Result<(), Self::Error> {
        let ctx = evm.ctx();

        // Get L1 pricing from state if we have enveloped transaction data
        // Clone the enveloped_tx from transaction to avoid borrow issues
        let enveloped_tx = ctx.tx().enveloped_tx().cloned();

        let tx_type = ctx.tx().tx_type();
        let has_poster_cost = !matches!(
            tx_type,
            ARBITRUM_DEPOSIT_TX_TYPE
                | ARBITRUM_UNSIGNED_TX_TYPE
                | ARBITRUM_CONTRACT_TX_TYPE
                | ARBITRUM_RETRY_TX_TYPE
                | ARBITRUM_INTERNAL_TX_TYPE
                | ARBITRUM_SUBMIT_RETRYABLE_TX_TYPE
        );
        let poster_is_batch_poster = ctx
            .tx()
            .poster()
            .is_none_or(|poster| poster == crate::constants::ARBOS_BATCH_POSTER_ADDRESS);
        let l1_cost = if !ctx.cfg().is_fee_charge_disabled()
            && ctx.block().basefee() > 0
            && has_poster_cost
            && poster_is_batch_poster
        {
            if let Some(ref enveloped_tx) = enveloped_tx {
                // Read L1 price per unit from ArbOS state
                let (l1_base_fee, brotli_level) = {
                    let mut state = ctx.arb_state(None, false);
                    let price = state
                        .l1_pricing()
                        .price_per_unit()
                        .get()
                        .unwrap_or(U256::ZERO);
                    let level = state.brotli_compression_level().get().unwrap_or(1);
                    (price, level)
                };

                let (cost, units) = l1_fee::calculate_compressed_tx_l1_cost(
                    enveloped_tx,
                    l1_base_fee,
                    brotli_level,
                )
                .map_err(|err| ERROR::from_string(err))?;
                ctx.local_mut().set_poster_units(Some(units));
                if units > 0 {
                    let previous = ctx
                        .arb_state(None, false)
                        .l1_pricing()
                        .units_since_update()
                        .get()
                        .map_err(|err| ERROR::from_string(format!("L1 units: {err}")))?;
                    ctx.arb_state(None, false)
                        .l1_pricing()
                        .units_since_update()
                        .set(previous.saturating_add(units))
                        .map_err(|err| ERROR::from_string(format!("L1 units: {err}")))?;
                }

                // Calculate and cache poster gas
                // Tip dropping has already normalized the transaction, so its
                // effective price is Nitro's GetPaidGasPrice here.
                let paid_gas_price = ctx.tx().effective_gas_price(ctx.block().basefee() as u128);
                let poster_gas = l1_fee::calculate_poster_gas(cost, U256::from(paid_gas_price));
                ctx.local_mut().set_poster_gas(Some(poster_gas));

                Some(cost)
            } else {
                None
            }
        } else {
            None
        };

        // Cache the L1 cost in local context
        ctx.local_mut().set_tx_l1_cost(l1_cost);

        // Now do the standard validation with all_mut
        let (block, tx, cfg, journal, _, _) = ctx.all_mut();

        // Load caller's account
        let mut caller = journal.load_account_with_code_mut(tx.caller())?.data;

        // Validate nonce and code (JournaledAccount derefs to Account)
        validate_account_nonce_and_code_with_components(&caller.deref().info, tx, cfg)?;

        let mut balance = *caller.balance();

        // Deduct L1 fee if calculated
        if let Some(l1_cost) = l1_cost {
            let Some(new_balance) = balance.checked_sub(l1_cost) else {
                return Err(InvalidTransaction::LackOfFundForMaxFee {
                    fee: Box::new(l1_cost),
                    balance: Box::new(balance),
                }
                .into());
            };
            balance = new_balance;
        }

        // Calculate and deduct L2 gas fee
        let balance = calculate_caller_fee(balance, tx, block, cfg)?;

        // Update caller balance and nonce
        caller.set_balance(balance);
        if tx.kind().is_call() {
            caller.bump_nonce();
        }

        Ok(())
    }

    fn execution(
        &mut self,
        evm: &mut Self::Evm,
        init_and_floor_gas: &InitialAndFloorGas,
    ) -> Result<FrameResult, Self::Error> {
        let requested = evm
            .ctx()
            .tx()
            .gas_limit()
            .saturating_sub(init_and_floor_gas.initial_gas);
        let arbos_version = evm
            .ctx()
            .arb_state(None, false)
            .arbos_version()
            .get()
            .map_err(|err| ERROR::from_string(format!("ArbOS version: {err}")))?;
        let cap = if arbos_version >= 50 {
            evm.ctx()
                .arb_state(None, false)
                .l2_pricing()
                .per_tx_gas_limit()
                .get()
                .map_err(|err| ERROR::from_string(format!("per-tx gas limit: {err}")))?
                .saturating_sub(init_and_floor_gas.initial_gas)
        } else {
            evm.ctx()
                .arb_state(None, false)
                .l2_pricing()
                .per_block_gas_limit()
                .get()
                .map_err(|err| ERROR::from_string(format!("per-block gas limit: {err}")))?
        };
        let frame_gas = requested.min(cap);
        evm.ctx()
            .local_mut()
            .set_held_gas(requested.saturating_sub(frame_gas));
        let first_frame_input = self.first_frame_input(evm, frame_gas)?;
        let mut frame_result = self.run_exec_loop(evm, first_frame_input)?;
        self.last_frame_result(evm, &mut frame_result)?;
        Ok(frame_result)
    }

    fn last_frame_result(
        &mut self,
        evm: &mut Self::Evm,
        frame_result: &mut FrameResult,
    ) -> Result<(), Self::Error> {
        let instruction_result = frame_result.interpreter_result().result;
        let held_gas = evm.ctx().local().held_gas();
        let transaction_gas_limit = evm.ctx().tx().gas_limit();
        let gas = frame_result.gas_mut();
        let remaining = gas.remaining();
        let refunded = gas.refunded();

        *gas = Gas::new_spent(transaction_gas_limit);
        // Held gas never enters the interpreter and is always returned, even
        // when execution exhausts its capped allowance.
        gas.erase_cost(held_gas);
        if instruction_result.is_ok_or_revert() {
            gas.erase_cost(remaining);
        }
        if instruction_result.is_ok() {
            gas.record_refund(refunded);
        }
        Ok(())
    }

    /// Distributes transaction fees using ArbOS's single-dimensional fee rules.
    /// Base-fee compute revenue is split between the infrastructure and network
    /// accounts, poster fees go to the L1 pricer pool, and tips are paid to the
    /// network account only when ArbOS 60's `collectTips` flag is enabled.
    fn reward_beneficiary(
        &self,
        evm: &mut Self::Evm,
        frame_result: &mut <<Self::Evm as EvmTr>::Frame as FrameTr>::FrameResult,
    ) -> Result<(), Self::Error> {
        let ctx = evm.ctx();
        let gas_used = frame_result.gas().used();
        let arbos_version = ctx
            .arb_state(None, false)
            .arbos_version()
            .get()
            .map_err(|err| ERROR::from_string(format!("ArbOS version: {err}")))?;
        if ctx.cfg().is_fee_charge_disabled() {
            return Ok(());
        }
        if ctx.tx().gas_price() > 0 {
            ctx.arb_state(None, false)
                .l2_pricing()
                .grow_backlog(gas_used, arbos_version)
                .map_err(|err| ERROR::from_string(format!("L2 gas backlog: {err}")))?;
        }

        // Unlike Nitro's geth hook, this handler deducts the L1 poster charge
        // directly in wei rather than inserting poster gas into the EVM gas
        // budget. Consequently all gas reported by this frame is compute gas.
        let compute_gas = gas_used;
        let base_fee = U256::from(ctx.block().basefee());
        let effective_gas_price =
            U256::from(ctx.tx().effective_gas_price(ctx.block().basefee() as u128));
        let l1_cost = ctx.local().tx_l1_cost().unwrap_or(U256::ZERO);

        let (network_fee_account, infra_fee_account, min_base_fee, collect_tips) = {
            let mut state = ctx.arb_state(None, false);
            let network = state
                .network_fee_account()
                .get()
                .map_err(|err| ERROR::from_string(format!("network fee account: {err}")))?;
            let infra = state
                .infra_fee_account()
                .get()
                .map_err(|err| ERROR::from_string(format!("infra fee account: {err}")))?;
            let minimum = state
                .l2_pricing()
                .min_base_fee_wei()
                .get()
                .map_err(|err| ERROR::from_string(format!("minimum base fee: {err}")))?;
            let collect = arbos_version == 9
                || (arbos_version >= 60
                    && state
                        .collect_tips()
                        .map_err(|err| ERROR::from_string(format!("collect tips: {err}")))?);
            (network, infra, minimum, collect)
        };

        let infra_price = if arbos_version < 11 || infra_fee_account.is_zero() {
            U256::ZERO
        } else {
            min_base_fee.min(base_fee)
        };
        let infra_reward = infra_price.saturating_mul(U256::from(compute_gas));
        let network_base_reward = base_fee
            .saturating_sub(infra_price)
            .saturating_mul(U256::from(compute_gas));
        let tip_reward = if collect_tips {
            effective_gas_price
                .saturating_sub(base_fee)
                .saturating_mul(U256::from(gas_used))
        } else {
            U256::ZERO
        };

        if !infra_reward.is_zero() {
            ctx.journal_mut()
                .balance_incr(infra_fee_account, infra_reward)?;
        }
        let network_reward = network_base_reward.saturating_add(tip_reward);
        if !network_reward.is_zero() {
            ctx.journal_mut()
                .balance_incr(network_fee_account, network_reward)?;
        }

        // Send L1 fees to the L1 pricer funds pool
        if !l1_cost.is_zero() {
            ctx.journal_mut()
                .balance_incr(ARBOS_L1_PRICER_FUNDS_ADDRESS, l1_cost)?;
            let recognized = ctx
                .arb_state(None, false)
                .l1_pricing()
                .l1_fees_available()
                .get()
                .map_err(|err| ERROR::from_string(format!("L1 fees available: {err}")))?;
            ctx.arb_state(None, false)
                .l1_pricing()
                .l1_fees_available()
                .set(recognized.saturating_add(l1_cost))
                .map_err(|err| ERROR::from_string(format!("L1 fees available: {err}")))?;
        }

        Ok(())
    }

    /// Processes the final execution output and cleans up L1 fee state.
    fn execution_result(
        &mut self,
        evm: &mut Self::Evm,
        result: <<Self::Evm as EvmTr>::Frame as FrameTr>::FrameResult,
    ) -> Result<ExecutionResult<Self::HaltReason>, Self::Error> {
        // Get the execution result using the mainnet implementation
        // Note: mainnet.execution_result already clears local context
        self.mainnet.execution_result(evm, result)
    }

    /// Handles cleanup when an error occurs during execution.
    fn catch_error(
        &self,
        evm: &mut Self::Evm,
        error: Self::Error,
    ) -> Result<ExecutionResult<Self::HaltReason>, Self::Error> {
        // mainnet.catch_error clears local context which includes L1 cost
        self.mainnet.catch_error(evm, error)
    }
}

impl<EVM, ERROR> InspectorHandler for ArbitrumHandler<EVM, ERROR, EthFrame<EthInterpreter>>
where
    EVM: InspectorEvmTr<
            Context: ArbitrumContextMutTr<Journal: JournalTr<State = EvmState>>,
            Frame = EthFrame<EthInterpreter>,
            Inspector: Inspector<<<Self as Handler>::Evm as EvmTr>::Context, EthInterpreter>,
        >,
    ERROR: EvmTrError<EVM> + FromStringError,
{
    type IT = EthInterpreter;
}
