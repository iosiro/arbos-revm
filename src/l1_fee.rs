//! L1 Fee calculation utilities for Arbitrum
//!
//! This module provides utility functions for calculating L1 data fees.
//! The L1 fee represents the cost of posting transaction data to L1.

use revm::primitives::{Bytes, U256};

/// Gas cost per non-zero byte of calldata (EIP-2028)
pub const TX_DATA_NON_ZERO_GAS: u64 = 16;

/// Gas cost per zero byte of calldata
pub const TX_DATA_ZERO_GAS: u64 = 4;

/// Calculate the data gas cost for transaction bytes.
///
/// This counts 16 gas per non-zero byte and 4 gas per zero byte,
/// following EIP-2028 pricing.
///
/// Note: In production Arbitrum, the transaction would be compressed
/// with Brotli first, and this calculation would be done on the
/// compressed bytes.
pub fn data_gas(data: &Bytes) -> u64 {
    let mut gas: u64 = 0;
    for &byte in data.iter() {
        if byte == 0 {
            gas = gas.saturating_add(TX_DATA_ZERO_GAS);
        } else {
            gas = gas.saturating_add(TX_DATA_NON_ZERO_GAS);
        }
    }
    gas
}

/// Calculate the L1 data cost for a transaction.
///
/// Formula:
/// ```text
/// data_gas = sum(16 for each non-zero byte, 4 for each zero byte)
/// l1_cost = data_gas * l1_base_fee
/// ```
///
/// # Arguments
/// * `enveloped_tx` - The enveloped transaction bytes
/// * `l1_base_fee` - The L1 base fee (price per unit) from ArbOS state
///
/// # Returns
/// The L1 cost in wei
pub fn calculate_tx_l1_cost(enveloped_tx: &Bytes, l1_base_fee: U256) -> U256 {
    if l1_base_fee.is_zero() {
        return U256::ZERO;
    }

    let gas = data_gas(enveloped_tx);
    U256::from(gas).saturating_mul(l1_base_fee)
}

/// Calculate the poster gas (L1 gas converted to L2 gas units).
///
/// This is the amount of L2 gas that will be charged to cover the L1 data cost.
/// The formula is: poster_gas = l1_cost / l2_base_fee (rounded up)
///
/// # Arguments
/// * `l1_cost` - The L1 cost in wei
/// * `l2_base_fee` - The L2 base fee in wei
///
/// # Returns
/// The poster gas amount in L2 gas units
pub fn calculate_poster_gas(l1_cost: U256, l2_base_fee: U256) -> u64 {
    if l2_base_fee.is_zero() {
        return 0;
    }

    // poster_gas = l1_cost / l2_base_fee (rounded up)
    let poster_gas = l1_cost.saturating_add(l2_base_fee - U256::from(1)) / l2_base_fee;

    // Saturate to u64::MAX if the result is too large
    poster_gas.try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_data_gas_calculation() {
        // All zeros
        let data = Bytes::from(vec![0u8; 10]);
        assert_eq!(data_gas(&data), 40); // 10 * 4

        // All non-zeros
        let data = Bytes::from(vec![1u8; 10]);
        assert_eq!(data_gas(&data), 160); // 10 * 16

        // Mixed
        let data = Bytes::from(vec![0, 1, 0, 1, 0]);
        assert_eq!(data_gas(&data), 44); // 3*4 + 2*16 = 12 + 32 = 44
    }

    #[test]
    fn test_l1_cost_calculation() {
        // 10 non-zero bytes = 160 gas * 1000 = 160,000 wei
        let data = Bytes::from(vec![1u8; 10]);
        assert_eq!(
            calculate_tx_l1_cost(&data, U256::from(1000)),
            U256::from(160_000)
        );
    }

    #[test]
    fn test_poster_gas_calculation() {
        // L1 cost = 160,000 wei, L2 base fee = 1000 wei
        // poster_gas = 160,000 / 1000 = 160
        let poster_gas = calculate_poster_gas(U256::from(160_000), U256::from(1000));
        assert_eq!(poster_gas, 160);

        // Test rounding up
        // L1 cost = 160,001 wei, L2 base fee = 1000 wei
        // poster_gas = ceil(160,001 / 1000) = ceil(160.001) = 161
        let poster_gas = calculate_poster_gas(U256::from(160_001), U256::from(1000));
        assert_eq!(poster_gas, 161);
    }

    #[test]
    fn test_zero_base_fee() {
        let data = Bytes::from(vec![1u8; 10]);
        assert_eq!(calculate_tx_l1_cost(&data, U256::ZERO), U256::ZERO);
        assert_eq!(calculate_poster_gas(U256::from(1000), U256::ZERO), 0);
    }
}
