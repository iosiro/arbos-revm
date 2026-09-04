use revm::primitives::U256;

pub const ONE_IN_BIPS: i64 = 10_000;

/// Integer approximation of `10_000 * exp(value / 10_000)`, matching Nitro's
/// `arbmath.ApproxExpBasisPoints` Horner-series implementation.
pub fn approx_exp_basis_points(value: i64, accuracy: u64) -> i64 {
    let negative = value < 0;
    let x = value.unsigned_abs();
    let b = ONE_IN_BIPS as u64;
    let mut result = b.saturating_add(x / accuracy);
    for i in (1..accuracy).rev() {
        result = b.saturating_add(result.saturating_mul(x) / (i * b));
    }
    if negative {
        (b.saturating_mul(b) / result) as i64
    } else {
        result as i64
    }
}

/// Nitro's unsigned saturating basis-point multiplication helper.
pub fn uint_saturating_mul_by_bips(value: U256, bips: i64) -> U256 {
    if bips <= 0 {
        U256::ZERO
    } else {
        value.saturating_mul(U256::from(bips as u64)) / U256::from(ONE_IN_BIPS as u64)
    }
}
