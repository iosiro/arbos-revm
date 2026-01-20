use std::ops::Deref;

use crate::{
    ArbitrumContextTr,
    constants::{
        ARBITRUM_DEPOSIT_TX_TYPE, ARBITRUM_INTERNAL_TX_TYPE, ARBITRUM_SUBMIT_RETRYABLE_TX_TYPE,
        ARBOS_ADDRESS, ARBOS_L1_PRICER_FUNDS_ADDRESS,
    },
    l1_fee,
    local_context::ArbitrumLocalContextTr,
    state::{ArbState, ArbStateGetter, types::StorageBackedTr},
    transaction::ArbitrumTxTr,
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
    primitives::{Bytes, TxKind, U256},
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
    /// Internal transactions are sent by ArbOS to update system state:
    /// - Update L1 pricing
    /// - Update block info (timestamps, etc.)
    /// - Reap expired retryables
    ///
    /// For now, we just validate and succeed without executing state updates,
    /// since full ArbOS state management would require additional implementation.
    fn execute_internal_tx(&mut self, evm: &mut EVM) -> Result<ExecutionResult<HaltReason>, ERROR> {
        let ctx = evm.ctx();
        let caller = ctx.tx().caller();

        // Internal transactions must be from ArbOS
        if caller != ARBOS_ADDRESS {
            return Err(ERROR::from_string(
                "internal transaction must be from ArbOS address".to_string(),
            ));
        }

        // For now, just succeed without applying state updates
        // Full implementation would parse tx.input() and apply updates to ArbOS state
        // similar to ApplyInternalTxUpdate in nitro

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

                // Calculate L1 cost
                let cost = l1_fee::calculate_tx_l1_cost(enveloped_tx, l1_base_fee);

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

    /// Distributes transaction fees to beneficiaries.
    ///
    /// This method:
    /// 1. Calls the mainnet reward_beneficiary for standard L2 fee distribution
    /// 2. Sends the cached L1 cost to the L1 pricer funds pool address
    fn reward_beneficiary(
        &self,
        evm: &mut Self::Evm,
        frame_result: &mut <<Self::Evm as EvmTr>::Frame as FrameTr>::FrameResult,
    ) -> Result<(), Self::Error> {
        // First, do the standard mainnet beneficiary reward (L2 fees)
        self.mainnet.reward_beneficiary(evm, frame_result)?;

        let ctx = evm.ctx();
        let l1_cost = ctx.local().tx_l1_cost().unwrap_or(U256::ZERO);

        // Send L1 fees to the L1 pricer funds pool
        if !l1_cost.is_zero() {
            ctx.journal_mut()
                .balance_incr(ARBOS_L1_PRICER_FUNDS_ADDRESS, l1_cost)?;
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
            Context: ArbitrumContextTr<Journal: JournalTr<State = EvmState>>,
            Frame = EthFrame<EthInterpreter>,
            Inspector: Inspector<<<Self as Handler>::Evm as EvmTr>::Context, EthInterpreter>,
        >,
    ERROR: EvmTrError<EVM> + FromStringError,
{
    type IT = EthInterpreter;
}
