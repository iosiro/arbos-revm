//! L1 Fee calculation utilities for Arbitrum
//!
//! This module provides utility functions for calculating L1 data fees.
//! The L1 fee represents the cost of posting transaction data to L1.

use crate::utils::{Dictionary, brotli_compress};
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
/// The transaction bytes are always compressed with Brotli first, and the
/// data gas is calculated on the compressed bytes (matching production
/// Arbitrum behavior). Falls back to uncompressed calculation on compression
/// error.
///
/// # Arguments
/// * `enveloped_tx` - The enveloped transaction bytes
/// * `l1_base_fee` - The L1 base fee (price per unit) from ArbOS state
/// * `brotli_compression_level` - Brotli compression level (0-11)
///
/// # Returns
/// The L1 cost in wei
pub fn calculate_tx_l1_cost(
    enveloped_tx: &Bytes,
    l1_base_fee: U256,
    brotli_compression_level: u64,
) -> U256 {
    if l1_base_fee.is_zero() {
        return U256::ZERO;
    }

    let units = match brotli_compress(
        enveloped_tx,
        brotli_compression_level as u32,
        22,
        Dictionary::Empty,
    ) {
        Ok(compressed) => TX_DATA_NON_ZERO_GAS * compressed.len() as u64,
        Err(_) => data_gas(enveloped_tx), // fallback
    };
    U256::from(units).saturating_mul(l1_base_fee)
}

/// Calculate the L1 data cost and calldata units for a transaction.
///
/// Returns both the cost and the calldata units so that
/// `units_since_update` can be tracked in L1 pricing state.
///
/// # Arguments
/// * `enveloped_tx` - The enveloped transaction bytes
/// * `l1_base_fee` - The L1 base fee (price per unit) from ArbOS state
///
/// # Returns
/// Tuple of (l1_cost_in_wei, calldata_units)
pub fn calculate_tx_l1_cost_and_units(enveloped_tx: &Bytes, l1_base_fee: U256) -> (U256, u64) {
    if l1_base_fee.is_zero() {
        return (U256::ZERO, 0);
    }

    let units = data_gas(enveloped_tx);
    let cost = U256::from(units).saturating_mul(l1_base_fee);
    (cost, units)
}

/// Calculate the poster gas (L1 gas converted to L2 gas units).
///
/// This is the amount of L2 gas that will be charged to cover the L1 data cost.
/// The formula is: poster_gas = l1_cost / l2_base_fee (truncating/floor division).
///
/// Matches nitro `tx_processor.go:432`: `BigDiv(posterCost, baseFee)` which truncates.
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

    // poster_gas = l1_cost / l2_base_fee (floor division, matching nitro BigDiv)
    let poster_gas = l1_cost / l2_base_fee;

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
        // Even at compression level 0, brotli is used (matching nitro behavior).
        // Cost is based on compressed size * 16 * l1_base_fee.
        let data = Bytes::from(vec![1u8; 10]);
        let cost = calculate_tx_l1_cost(&data, U256::from(1000), 0);
        // Brotli at level 0 still compresses, so cost should differ from raw data_gas
        let raw_cost = U256::from(data_gas(&data)).saturating_mul(U256::from(1000));
        // The cost should be positive (non-zero data with non-zero base fee)
        assert!(cost > U256::ZERO);
        // With only 10 bytes, brotli overhead may make compressed larger,
        // but the important thing is we use the brotli path
        assert_ne!(cost, raw_cost);
    }

    #[test]
    fn test_l1_cost_with_brotli_compression() {
        // Higher compression levels should produce smaller or equal output
        let data = Bytes::from(vec![0xABu8; 100]);
        let cost_level_0 = calculate_tx_l1_cost(&data, U256::from(1000), 0);
        let cost_level_1 = calculate_tx_l1_cost(&data, U256::from(1000), 1);
        // Both should be based on compressed size; level 1 should be <= level 0
        assert!(cost_level_1 <= cost_level_0);
    }

    #[test]
    fn test_poster_gas_calculation() {
        // L1 cost = 160,000 wei, L2 base fee = 1000 wei
        // poster_gas = 160,000 / 1000 = 160
        let poster_gas = calculate_poster_gas(U256::from(160_000), U256::from(1000));
        assert_eq!(poster_gas, 160);

        // Test truncation (floor division, matching nitro BigDiv)
        // L1 cost = 160,001 wei, L2 base fee = 1000 wei
        // poster_gas = floor(160,001 / 1000) = floor(160.001) = 160
        let poster_gas = calculate_poster_gas(U256::from(160_001), U256::from(1000));
        assert_eq!(poster_gas, 160);
    }

    #[test]
    fn test_zero_base_fee() {
        let data = Bytes::from(vec![1u8; 10]);
        assert_eq!(calculate_tx_l1_cost(&data, U256::ZERO, 0), U256::ZERO);
        assert_eq!(calculate_poster_gas(U256::from(1000), U256::ZERO), 0);
    }
}
