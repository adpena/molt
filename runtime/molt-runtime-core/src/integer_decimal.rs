//! Decimal integer output under the runtime's existing digit-limit policy.
//!
//! The setting is supplied lazily by the owning runtime. Small magnitudes never
//! read it; clearly oversized values are rejected before decimal conversion.

use num_bigint::{BigInt, Sign};

/// Smallest positive limit accepted by `sys.set_int_max_str_digits`.
pub const INT_STR_DIGITS_CHECK_THRESHOLD: i64 = 640;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecimalLimitExceeded {
    pub limit: usize,
}

impl std::fmt::Display for DecimalLimitExceeded {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            out,
            "Exceeds the limit ({} digits) for integer string conversion; use sys.set_int_max_str_digits() to increase the limit",
            self.limit
        )
    }
}

/// Render the magnitude without cloning the integer or counting its sign.
/// `current_limit` must return the live policy: zero, or at least the validated
/// minimum above. Padding, grouping and prefixes belong to the caller.
pub fn decimal_magnitude(
    value: &BigInt,
    current_limit: impl FnOnce() -> usize,
) -> Result<String, DecimalLimitExceeded> {
    let bits = value.bits();
    // 2^3 < 10 proves every b-bit magnitude with b <= 3L has at most L
    // decimal digits. This fast path needs neither the setting nor an extra
    // allocation; even the smallest admitted positive setting permits it.
    let limit = if bits <= 3 * INT_STR_DIGITS_CHECK_THRESHOLD as u64 {
        0
    } else {
        current_limit()
    };
    if limit != 0 {
        // n >= 2^(b-1), and 2^10 > 10^3. This conservative integer-only
        // rejection cannot reject an allowed magnitude. u128 keeps both
        // products bounded for every supported pointer width and bit count.
        if 3 * u128::from(bits - 1) >= 10 * limit as u128 {
            return Err(DecimalLimitExceeded { limit });
        }
    }
    let digits = value.magnitude().to_str_radix(10);
    if limit != 0 && digits.len() > limit {
        return Err(DecimalLimitExceeded { limit });
    }
    Ok(digits)
}

pub fn decimal_string(
    value: &BigInt,
    current_limit: impl FnOnce() -> usize,
) -> Result<String, DecimalLimitExceeded> {
    let mut digits = decimal_magnitude(value, current_limit)?;
    if value.sign() == Sign::Minus {
        digits.insert(0, '-');
    }
    Ok(digits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_limit_boundary_sign_and_disabled_policy() {
        let allowed = BigInt::from(10u8).pow(639);
        let rejected = &allowed * BigInt::from(10u8);
        let expected = format!("1{}", "0".repeat(639));
        assert_eq!(decimal_string(&allowed, || 640).unwrap(), expected);
        assert_eq!(
            decimal_string(&-&allowed, || 640).unwrap(),
            format!("-{expected}")
        );
        for value in [&rejected, &-&rejected] {
            assert_eq!(
                decimal_string(value, || 640),
                Err(DecimalLimitExceeded { limit: 640 })
            );
            assert_eq!(
                decimal_string(value, || 0)
                    .unwrap()
                    .trim_start_matches('-')
                    .len(),
                641
            );
        }
    }

    #[test]
    fn bounded_values_do_not_query_runtime_policy() {
        for value in [
            BigInt::from(0),
            BigInt::from(i64::MIN),
            BigInt::from(u64::MAX),
        ] {
            let rendered = decimal_string(&value, || panic!("bounded value read policy")).unwrap();
            assert!(!rendered.is_empty());
        }
    }

    #[test]
    fn oversized_magnitude_is_rejected_without_decimal_output() {
        let value = BigInt::from(1u8) << 1_000_000usize;
        assert_eq!(
            decimal_string(&value, || 640),
            Err(DecimalLimitExceeded { limit: 640 })
        );
        assert_eq!(
            DecimalLimitExceeded { limit: 640 }.to_string(),
            "Exceeds the limit (640 digits) for integer string conversion; use sys.set_int_max_str_digits() to increase the limit"
        );
    }
}
