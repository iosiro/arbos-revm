//! Shared math utilities for ArbOS pricing models.
//!
//! Extracted from program.rs so that both the Stylus data pricer and the
//! L2 gas pricing model can share the same exponential/basis-point helpers.

use revm::primitives::U256;

/// Basis points constant: 1.0 = 10000 bips.
pub const ONE_IN_BIPS: i64 = 10_000;

/// Approximates `b * e^(x/b)` using the Maclaurin series with Horner's method,
/// where x is in basis points and b = [`ONE_IN_BIPS`] (10000).
/// Matches nitro's `arbmath.ApproxExpBasisPoints`.
pub fn approx_exp_basis_points(value: i64, accuracy: u64) -> i64 {
    let negative = value < 0;
    let x = if negative { -value } else { value } as u64;
    let b = ONE_IN_BIPS as u64;

    // Horner's method: res = b + x/accuracy, then iterate
    let mut res = b + x / accuracy;
    let mut i = accuracy - 1;
    while i > 0 {
        res = b + res.saturating_mul(x) / (i * b);
        i -= 1;
    }

    if negative {
        // e^(-x) = b^2 / res (in bips representation)
        (b.saturating_mul(b) / res) as i64
    } else {
        res as i64
    }
}

/// Convert a natural-unit value to basis points (multiply by 10000).
pub fn natural_to_bips(value: i64) -> i64 {
    value.saturating_mul(ONE_IN_BIPS)
}

/// Multiply a U256 value by a basis-point factor. Returns zero if `bips <= 0`.
pub fn big_mul_by_bips(value: U256, bips: i64) -> U256 {
    if bips <= 0 {
        return U256::ZERO;
    }
    value.saturating_mul(U256::from(bips as u64)) / U256::from(ONE_IN_BIPS as u64)
}

/// Adjust a gas backlog by a signed delta.
/// Positive `gas` drains the backlog; negative `gas` adds to it.
pub fn apply_gas_delta(backlog: u64, gas: i64) -> u64 {
    if gas > 0 {
        backlog.saturating_sub(gas as u64)
    } else {
        backlog.saturating_add((-gas) as u64)
    }
}
