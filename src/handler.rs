// TODO — Override the GASPRICE opcode to return `basefee` instead of the
// transaction's gas price for ArbOS >= 3. revm does not currently expose
// per-opcode overrides, so the standard EVM GASPRICE behaviour is used.

use std::ops::Deref;

use alloy_sol_types::{SolCall, sol};

use crate::{
    ArbitrumContextTr,
    config::ArbitrumConfigTr,
    constants::{
        ARBITRUM_DEPOSIT_TX_TYPE, ARBITRUM_INTERNAL_TX_TYPE, ARBITRUM_RETRY_TX_TYPE,
        ARBITRUM_SUBMIT_RETRYABLE_TX_TYPE, ARBITRUM_UNSIGNED_TX_TYPE, ARBOS_ADDRESS,
        ARBOS_L1_PRICER_FUNDS_ADDRESS,
    },
    l1_fee,
    local_context::ArbitrumLocalContextTr,
    state::{ArbState, ArbStateGetter, types::StorageBackedTr},
    transaction::{ArbitrumInternalTx, ArbitrumTxTr},
};
use revm::{
    Inspector,
    context::{
        Block, Cfg, ContextTr, JournalTr, LocalContextTr, Transaction,
        result::{ExecutionResult, FromStringError, HaltReason, InvalidTransaction, SuccessReason},
    },
    handler::{
        EthFrame, EvmTr, FrameTr, Handler, MainnetHandler,
        handler::EvmTrError,
        pre_execution::{calculate_caller_fee, validate_account_nonce_and_code_with_components},
    },
    inspector::{InspectorEvmTr, InspectorHandler},
    interpreter::interpreter::EthInterpreter,
    primitives::{Address, Bytes, TxKind, U256},
    state::EvmState,
};

sol! {
    function startBlock(uint256 l1BaseFee, uint64 l1BlockNumber, uint64 l2BlockNumber, uint64 timePassed);
    function batchPostingReport(uint256 batchTimestamp, address batchPosterAddress, uint64 batchNumber, uint64 batchDataGas, uint256 l1BaseFeeWei);
    function batchPostingReportV2(uint256 batchTimestamp, address batchPosterAddress, uint64 batchNumber, uint64 batchCalldataLength, uint64 batchCalldataNonZeros, uint64 batchExtraGas, uint256 l1BaseFeeWei);
}

/// Returns true for Arbitrum-specific tx types that should have their
/// tip dropped (gas price capped at basefee). Matches nitro DropTip().
fn should_drop_tip(tx_type: u8) -> bool {
    matches!(
        tx_type,
        ARBITRUM_DEPOSIT_TX_TYPE
            | ARBITRUM_UNSIGNED_TX_TYPE
            | ARBITRUM_RETRY_TX_TYPE
            | ARBITRUM_SUBMIT_RETRYABLE_TX_TYPE
            | ARBITRUM_INTERNAL_TX_TYPE
    )
}

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
            Context: ArbitrumContextTr<Journal: JournalTr<State = EvmState>>,
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
    /// Internal transactions are sent by ArbOS to update system state.
    /// Dispatches to `startBlock` or `batchPostingReport` based on the
    /// 4-byte selector in `tx.input()`.
    ///
    /// Matches nitro `internal_tx.go:68-141`.
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
        let arbos_version = ctx.cfg().arbos_version() as u64;
        let current_time = ctx.block().timestamp().saturating_to::<u64>();

        if input.len() >= 4 {
            let selector: [u8; 4] = input[..4].try_into().unwrap_or_default();

            match selector {
                <startBlockCall as SolCall>::SELECTOR => {
                    self.handle_start_block(evm, &input[4..], arbos_version, current_time);
                }
                <batchPostingReportCall as SolCall>::SELECTOR => {
                    self.handle_batch_posting_report(evm, &input[4..], current_time);
                }
                s if s == ArbitrumInternalTx::BATCH_POSTING_REPORT_V2_METHOD => {
                    self.handle_batch_posting_report_v2(
                        evm,
                        &input[4..],
                        arbos_version,
                        current_time,
                    );
                }
                _ => {
                    // Unknown internal TX type — ignore silently
                }
            }
        }

        // Commit the transaction
        let ctx = evm.ctx();
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

    /// Handles the `startBlock` internal transaction.
    ///
    /// 1. Processes parent block hash (EIP-2935, ArbOS >= 40)
    /// 2. Records new L1 block hash (only when l1_block_number > old)
    /// 3. Reaps up to 2 expired retryables
    /// 4. Updates L2 pricing model
    /// 5. Checks for ArbOS version upgrades
    ///
    /// Matches nitro `internal_tx.go:68-109`.
    fn handle_start_block(
        &self,
        evm: &mut EVM,
        data: &[u8],
        arbos_version: u64,
        current_time: u64,
    ) {
        let Ok(decoded) = startBlockCall::abi_decode(data) else {
            return;
        };

        let mut l1_block_number = decoded.l1BlockNumber;
        let time_passed = decoded.timePassed;

        // Version corrections (nitro internal_tx.go:89-92)
        if arbos_version < 8 {
            l1_block_number = l1_block_number.saturating_add(1);
        }

        let ctx = evm.ctx();

        // For ArbOS >= 40, call EIP-2935 ProcessParentBlockHash.
        // TODO: revm does not expose ProcessParentBlockHash directly;
        // implement when revm gains EIP-2935 system-call support.

        // Use B256::ZERO as the previous block hash placeholder.
        // In production nitro this is evm.Context.GetHash(blockNumber - 1),
        // but we don't have access to the block hash oracle here.
        // TODO: Pass the actual previous block hash when the execution context
        // provides access to block hash history.
        let prev_hash = revm::primitives::B256::ZERO;

        // Only call record_new_l1_block when l1_block_number > old_l1_block_number
        // (nitro internal_tx.go:97-99)
        let old_l1_block_number = ctx
            .arb_state(None, false)
            .blockhashes()
            .l1_block_number()
            .get()
            .unwrap_or(0);

        if l1_block_number > old_l1_block_number {
            // Pass l1_block_number - 1 to record_new_l1_block (nitro internal_tx.go:98)
            let _ = ctx
                .arb_state(None, false)
                .blockhashes()
                .record_new_l1_block(l1_block_number - 1, prev_hash, arbos_version);
        }

        // Reap up to 2 expired retryable tickets
        let _ = ctx
            .arb_state(None, false)
            .retryable_state()
            .try_to_reap_one_retryable(current_time);
        let _ = ctx
            .arb_state(None, false)
            .retryable_state()
            .try_to_reap_one_retryable(current_time);

        // Update L2 pricing model
        let _ = ctx
            .arb_state(None, false)
            .l2_pricing()
            .update_pricing_model(time_passed);

        // Check for ArbOS version upgrade (stub)
        let _ = ctx
            .arb_state(None, false)
            .upgrade_arbos_version_if_necessary(current_time);
    }

    /// Handles the `batchPostingReport` internal transaction.
    ///
    /// Computes L1 spending and forwards to L1 pricing settlement.
    fn handle_batch_posting_report(&self, evm: &mut EVM, data: &[u8], current_time: u64) {
        let Ok(decoded) = batchPostingReportCall::abi_decode(data) else {
            return;
        };

        let batch_timestamp = decoded.batchTimestamp.saturating_to::<u64>();
        let batch_poster = decoded.batchPosterAddress;
        let batch_data_gas = decoded.batchDataGas;
        let l1_base_fee_wei = decoded.l1BaseFeeWei;

        let ctx = evm.ctx();

        // Compute gas spent: per_batch_gas_cost + batch_data_gas
        let per_batch_gas_cost = ctx
            .arb_state(None, false)
            .l1_pricing()
            .per_batch_gas_cost()
            .get()
            .unwrap_or(0);
        let gas_spent = U256::from(per_batch_gas_cost).saturating_add(U256::from(batch_data_gas));
        let wei_spent = l1_base_fee_wei.saturating_mul(gas_spent);

        // Ensure batch poster is registered
        let _ = ctx
            .arb_state(None, false)
            .l1_pricing()
            .batch_poster_table()
            .add_if_missing(batch_poster, batch_poster);

        let _ = ctx
            .arb_state(None, false)
            .l1_pricing()
            .update_for_batch_poster_spending(
                batch_timestamp,
                current_time,
                batch_poster,
                wei_spent,
                l1_base_fee_wei,
            );
    }

    /// Handles the `batchPostingReportV2` internal transaction.
    ///
    /// V2 adds `batchExtraGas` field and uses calldata stats (length + non-zeros)
    /// instead of a single `batchDataGas` value. Computes gas spent using the
    /// legacy cost formula plus extra gas and per-batch gas cost.
    ///
    /// Matches nitro `internal_tx.go:142-195`.
    fn handle_batch_posting_report_v2(
        &self,
        evm: &mut EVM,
        data: &[u8],
        arbos_version: u64,
        current_time: u64,
    ) {
        let Ok(decoded) = batchPostingReportV2Call::abi_decode(data) else {
            return;
        };

        let batch_timestamp = decoded.batchTimestamp.saturating_to::<u64>();
        let batch_poster = decoded.batchPosterAddress;
        let batch_calldata_length = decoded.batchCalldataLength;
        let batch_calldata_non_zeros = decoded.batchCalldataNonZeros;
        let batch_extra_gas = decoded.batchExtraGas;
        let l1_base_fee_wei = decoded.l1BaseFeeWei;

        let ctx = evm.ctx();

        // Compute legacy cost for stats (nitro arbostypes/incomingmessage.go:169-176)
        // gas = TxDataZeroGas*(length-nonZeros) + TxDataNonZeroGasEIP2028*nonZeros
        // + Keccak256Gas + WordsForBytes(length)*Keccak256WordGas
        // + 2*SstoreSetGasEIP2200
        let tx_data_zero_gas: u64 = 4;
        let tx_data_non_zero_gas_eip2028: u64 = 16;
        let keccak256_gas: u64 = 30;
        let keccak256_word_gas: u64 = 6;
        let sstore_set_gas_eip2200: u64 = 20000;
        let words_for_bytes = batch_calldata_length.saturating_add(31) / 32;

        let mut gas_spent: u64 = tx_data_zero_gas
            .saturating_mul(batch_calldata_length.saturating_sub(batch_calldata_non_zeros))
            .saturating_add(tx_data_non_zero_gas_eip2028.saturating_mul(batch_calldata_non_zeros))
            .saturating_add(keccak256_gas)
            .saturating_add(words_for_bytes.saturating_mul(keccak256_word_gas))
            .saturating_add(2u64.saturating_mul(sstore_set_gas_eip2200));

        // Add batchExtraGas
        gas_spent = gas_spent.saturating_add(batch_extra_gas);

        // Add per-batch gas cost
        let per_batch_gas_cost = ctx
            .arb_state(None, false)
            .l1_pricing()
            .per_batch_gas_cost()
            .get()
            .unwrap_or(0);
        gas_spent = gas_spent.saturating_add(per_batch_gas_cost);

        // For ArbOS >= 50, apply gas floor per token (nitro internal_tx.go:169-178)
        if arbos_version >= 50 {
            let gas_floor_per_token = ctx
                .arb_state(None, false)
                .l1_pricing()
                .gas_floor_per_token()
                .get()
                .unwrap_or(0);
            // FloorGasAdditionalTokens = 172 (nitro internal_tx.go:60)
            let floor_gas_additional_tokens: u64 = 172;
            let tx_gas: u64 = 21000; // params.TxGas
            let floor_gas_spent = gas_floor_per_token
                .saturating_mul(
                    batch_calldata_length
                        .saturating_add(batch_calldata_non_zeros.saturating_mul(3))
                        .saturating_add(floor_gas_additional_tokens),
                )
                .saturating_add(tx_gas);
            if floor_gas_spent > gas_spent {
                gas_spent = floor_gas_spent;
            }
        }

        let wei_spent = l1_base_fee_wei.saturating_mul(U256::from(gas_spent));

        // Ensure batch poster is registered
        let _ = ctx
            .arb_state(None, false)
            .l1_pricing()
            .batch_poster_table()
            .add_if_missing(batch_poster, batch_poster);

        let _ = ctx
            .arb_state(None, false)
            .l1_pricing()
            .update_for_batch_poster_spending(
                batch_timestamp,
                current_time,
                batch_poster,
                wei_spent,
                l1_base_fee_wei,
            );
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
        // Mint the deposit value to the caller (L1 -> L2 bridging)
        // This is similar to how deposit transactions work
        let ctx = evm.ctx();
        let caller = ctx.tx().caller();
        let value = ctx.tx().value();

        // Mint ETH to the caller (from L1)
        ctx.journal_mut().balance_incr(caller, value)?;

        // Now proceed with normal execution flow
        // The transaction will call ARB_RETRYABLE_TX_ADDRESS with the submit retryable calldata
        match self.run_without_catch_error(evm) {
            Ok(output) => Ok(output),
            Err(e) => self.catch_error(evm, e),
        }
    }
}

impl<EVM, ERROR> Handler for ArbitrumHandler<EVM, ERROR, EthFrame<EthInterpreter>>
where
    EVM: EvmTr<
            Context: ArbitrumContextTr<Journal: JournalTr<State = EvmState>>,
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
        let tx_type = evm.ctx().tx().tx_type();

        match tx_type {
            ARBITRUM_DEPOSIT_TX_TYPE => self.execute_deposit_tx(evm),
            ARBITRUM_INTERNAL_TX_TYPE => self.execute_internal_tx(evm),
            ARBITRUM_SUBMIT_RETRYABLE_TX_TYPE => self.execute_submit_retryable(evm),
            _ => {
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

        let l1_cost = if !ctx.cfg().is_fee_charge_disabled() {
            if let Some(ref enveloped_tx) = enveloped_tx {
                // Read L1 price per unit from ArbOS state
                let l1_base_fee = ctx
                    .arb_state(None, false)
                    .l1_pricing()
                    .price_per_unit()
                    .get()
                    .unwrap_or(U256::ZERO);

                // Calculate L1 cost (with Brotli compression)
                let brotli_compression_level = ctx
                    .arb_state(None, false)
                    .brotli_compression_level()
                    .get()
                    .unwrap_or(0);
                let cost = l1_fee::calculate_tx_l1_cost(
                    enveloped_tx,
                    l1_base_fee,
                    brotli_compression_level,
                );
                // Also get calldata units for tracking
                let (_, calldata_units) =
                    l1_fee::calculate_tx_l1_cost_and_units(enveloped_tx, l1_base_fee);

                // Update units_since_update in L1 pricing state
                if calldata_units > 0 {
                    let _ = ctx
                        .arb_state(None, false)
                        .l1_pricing()
                        .add_to_units_since_update(calldata_units);
                }
                ctx.local_mut().set_calldata_units(calldata_units);

                // Calculate and cache poster gas
                let basefee = ctx.block().basefee() as u128;
                let poster_gas = l1_fee::calculate_poster_gas(cost, U256::from(basefee));
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

    /// Distributes transaction fees using the Arbitrum fee model.
    ///
    /// Instead of the mainnet behaviour (tip to coinbase, basefee burned), Arbitrum
    /// splits fees between:
    /// - **Infra fee account**: `min(min_base_fee, basefee) * compute_gas`
    /// - **Network fee account**: remaining compute cost
    /// - **L1 pricer pool**: the cached L1 data cost
    ///
    /// Override reimburse_caller to enforce NonrefundableGas and DropTip.
    /// - Poster gas is non-refundable (nitro NonrefundableGas = posterGas)
    /// - Arbitrum-specific tx types use basefee, not effective_gas_price (DropTip)
    fn reimburse_caller(
        &self,
        evm: &mut Self::Evm,
        frame_result: &mut <<Self::Evm as EvmTr>::Frame as FrameTr>::FrameResult,
    ) -> Result<(), Self::Error> {
        let gas = frame_result.gas();
        let remaining = gas.remaining();
        let refunded = gas.refunded() as u64;

        let ctx = evm.ctx();
        let poster_gas = ctx.local().poster_gas().unwrap_or(0);
        let tx_type = ctx.tx().tx_type();
        // DropTip — use basefee for Arbitrum-specific tx types
        let basefee_u128 = ctx.block().basefee() as u128;
        let effective_gas_price = if should_drop_tip(tx_type) {
            U256::from(basefee_u128)
        } else {
            U256::from(ctx.tx().effective_gas_price(basefee_u128))
        };

        // Poster gas is non-refundable. Cap refund so we don't refund more
        // than gas_used - poster_gas.
        let gas_used = gas.used();
        let refundable_used = gas_used.saturating_sub(poster_gas);
        let max_refund = remaining.min(refundable_used);
        let total_refund = max_refund.saturating_add(refunded.min(max_refund / 5));

        let caller = ctx.tx().caller();
        let refund_amount = effective_gas_price.saturating_mul(U256::from(total_refund));

        // Use the mainnet handler's balance_incr pattern
        let _ = evm.ctx().journal_mut().balance_incr(caller, refund_amount);

        Ok(())
    }

    /// Also drains the L2 gas pool by the compute gas consumed.
    fn reward_beneficiary(
        &self,
        evm: &mut Self::Evm,
        frame_result: &mut <<Self::Evm as EvmTr>::Frame as FrameTr>::FrameResult,
    ) -> Result<(), Self::Error> {
        let gas_used = frame_result.gas().used();

        let ctx = evm.ctx();
        let basefee = U256::from(ctx.block().basefee() as u128);
        let l1_cost = ctx.local().tx_l1_cost().unwrap_or(U256::ZERO);
        let poster_gas = ctx.local().poster_gas().unwrap_or(0);

        // compute_cost = total_cost - poster_fee (nitro tx_processor.go:645-646)
        // total_cost = basefee * gas_used
        // poster_fee = the L1 cost already charged (cached as l1_cost)
        let total_cost = basefee.saturating_mul(U256::from(gas_used));
        let mut compute_cost = total_cost.saturating_sub(l1_cost);

        // compute_gas = gas_used - poster_gas (nitro tx_processor.go:663)
        // Used for infra fee calculation and gas pool deduction
        let compute_gas = gas_used.saturating_sub(poster_gas);

        // Infra fee split (nitro tx_processor.go:656-668)
        let infra_fee_account = ctx
            .arb_state(None, false)
            .infra_fee_account()
            .get()
            .unwrap_or(Address::ZERO);
        if infra_fee_account != Address::ZERO && !compute_cost.is_zero() {
            let min_base_fee = ctx
                .arb_state(None, false)
                .l2_pricing()
                .min_base_fee_wei()
                .get()
                .unwrap_or(U256::ZERO);
            let infra_fee = basefee.min(min_base_fee);
            // Use compute_gas (not gas_used) for infra fee calculation
            let infra_compute_cost = infra_fee.saturating_mul(U256::from(compute_gas));
            ctx.journal_mut()
                .balance_incr(infra_fee_account, infra_compute_cost)?;
            compute_cost = compute_cost.saturating_sub(infra_compute_cost);
        }

        // Network fee account gets remaining compute cost
        let network_fee_account = ctx
            .arb_state(None, false)
            .network_fee_account()
            .get()
            .unwrap_or(Address::ZERO);
        if network_fee_account != Address::ZERO && !compute_cost.is_zero() {
            ctx.journal_mut()
                .balance_incr(network_fee_account, compute_cost)?;
        }

        // L1 cost goes to L1 pricer funds pool (nitro tx_processor.go:672-676)
        if !l1_cost.is_zero() {
            ctx.journal_mut()
                .balance_incr(ARBOS_L1_PRICER_FUNDS_ADDRESS, l1_cost)?;

            // Update l1FeesAvailable after minting poster fee (nitro tx_processor.go:677-681)
            let current_l1_fees = ctx
                .arb_state(None, false)
                .l1_pricing()
                .l1_fees_available()
                .get()
                .unwrap_or(U256::ZERO);
            let _ = ctx
                .arb_state(None, false)
                .l1_pricing()
                .l1_fees_available()
                .set(current_l1_fees.saturating_add(l1_cost));
        }

        // Update L2 gas pool using compute_gas (not total gas_used)
        // (nitro tx_processor.go:688-698)
        let _ = ctx
            .arb_state(None, false)
            .l2_pricing()
            .add_to_gas_pool(-(compute_gas as i64));

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
            Context: ArbitrumContextTr<Journal: JournalTr<State = EvmState>>,
            Frame = EthFrame<EthInterpreter>,
            Inspector: Inspector<<<Self as Handler>::Evm as EvmTr>::Context, EthInterpreter>,
        >,
    ERROR: EvmTrError<EVM> + FromStringError,
{
    type IT = EthInterpreter;
}
