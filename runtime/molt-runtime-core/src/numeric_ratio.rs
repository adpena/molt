//! Exact integer-ratio to binary64 conversion shared by arithmetic and satellites.
//!
//! Uses CPython's `long_true_divide` scaled quotient and guard/sticky rounding:
//! https://github.com/python/cpython/blob/ead82c07db1f111d613926ed1b6667411d768125/Objects/longobject.c (3.14.9)
//! The final, already rounded significand is encoded directly instead of multiplying
//! by `2.0.powi(shift)`: that factor can underflow before a subnormal result is formed.
//! One shifted magnitude and one div/rem operation; no runtime entry or boxed values.

use num_bigint::{BigInt, Sign};
use num_integer::Integer;
use num_traits::{ToPrimitive, Zero};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RatioFloatError {
    ZeroDenominator,
    Overflow,
}

/// Whether converting an unsigned machine integer to binary64 is lossless.
/// Leading zeroes and powers of two do not consume significand precision.
#[inline]
pub fn u64_is_exact_binary64(value: u64) -> bool {
    value == 0 || u64::BITS - value.leading_zeros() - value.trailing_zeros() <= 53
}

/// Round an exact signed rational once, to nearest binary64 with ties to even.
/// A negative denominator also supplies the sign of a zero numerator.
pub fn ratio_to_f64(num: &BigInt, den: &BigInt) -> Result<f64, RatioFloatError> {
    if den.is_zero() {
        return Err(RatioFloatError::ZeroDenominator);
    }
    let sign = u64::from((num.sign() == Sign::Minus) != (den.sign() == Sign::Minus)) << 63;
    let a = num.magnitude();
    let b = den.magnitude();
    if a.is_zero() {
        return Ok(f64::from_bits(sign));
    }
    // Preserve the allocation-free small-ratio path used by Fraction and reductions.
    if let (Some(a), Some(b)) = (a.to_u64(), b.to_u64())
        && u64_is_exact_binary64(a)
        && u64_is_exact_binary64(b)
    {
        let value = a as f64 / b as f64;
        return Ok(f64::from_bits(value.to_bits() | sign));
    }
    let diff = i128::from(a.bits()) - i128::from(b.bits());
    if diff > 1024 {
        return Err(RatioFloatError::Overflow);
    }
    if diff < -1075 {
        return Ok(f64::from_bits(sign));
    }
    // Bounds above make this conversion exact, independently of pointer width.
    let shift = (diff as i64).max(-1021) - 55;
    let (quotient, remainder) = if shift <= 0 {
        (a << (-shift as u64)).div_rem(b)
    } else {
        a.div_rem(&(b << (shift as u64)))
    };
    let mut q = quotient.to_u64().expect("scaled ratio fits in 56 bits");
    q |= u64::from(!remainder.is_zero());
    let bits = i64::from(u64::BITS - q.leading_zeros());
    // Round at the normal significand width OR the subnormal 2^-1074 quantum.
    let discarded = (bits - 53).max(-1074 - shift);
    debug_assert!((2..=3).contains(&discarded));
    let half = 1_u64 << (discarded - 1);
    let tail = q & ((half << 1) - 1);
    let mut significand = q >> discarded;
    if tail > half || (tail == half && significand & 1 != 0) {
        significand += 1;
    }
    if significand == 0 {
        return Ok(f64::from_bits(sign));
    }
    let scale = shift + discarded;
    let bits = i64::from(u64::BITS - significand.leading_zeros());
    let exponent = scale + bits - 1;
    if exponent > 1023 {
        return Err(RatioFloatError::Overflow);
    }
    let magnitude = if exponent >= -1022 {
        // A rounding carry can produce the exact power 2^53 (54 bits).
        let normalized = if bits > 53 {
            significand >> (bits - 53)
        } else {
            significand << (53 - bits)
        };
        ((exponent + 1023) as u64) << 52 | (normalized & ((1_u64 << 52) - 1))
    } else {
        // scale >= -1074 and the result is below 2^52 subnormal quanta.
        significand << (scale + 1074)
    };
    Ok(f64::from_bits(sign | magnitude))
}

#[cfg(test)]
mod tests {
    use super::*;
    /// `ratio_to_f64` must produce the exact same IEEE-754 double as
    /// CPython's `long_true_divide` (`int / int`), bit-for-bit. Reference bit
    /// patterns were captured from CPython 3.12 via
    /// `struct.pack('<d', a / b)`.
    #[test]
    fn ratio_to_f64_matches_cpython_bit_exact() {
        // (numerator, denominator, expected f64 bits from CPython `a / b`).
        let cases: &[(&str, &str, u64)] = &[
            ("1152921504606846976", "2", 0x43a0_0000_0000_0000), // (1<<60)/2
            ("1152921504606846976", "3", 0x4395_5555_5555_5555), // (1<<60)/3
            ("1152921504606846977", "2", 0x43a0_0000_0000_0000), // ((1<<60)+1)/2 (ties-to-even)
            (
                "1000000000000000000000000000000",
                "7",
                0x45fc_d98a_8b00_a10b,
            ), // 10**30/7
            ("-1180591620717411303424", "3", 0xc435_5555_5555_5555), // -(1<<70)/3
            ("1", "3", 0x3fd5_5555_5555_5555),
            (
                "1267650600228229401496703205376", // 2**100
                "1125899906842624",                // 2**50
                0x4310_0000_0000_0000,
            ),
            ("1152921504606846976", "1", 0x43b0_0000_0000_0000), // (1<<60)/1
            (
                "-10000000000000000000000000000000000000000", // -(10**40)
                "99991",
                0xc733_42d3_0df6_e471,
            ),
            ("0", "5", 0x0000_0000_0000_0000), // 0/5 == +0.0
            // 2**1000 / 3: large numerator near the top of the f64 range.
            (
                "10715086071862673209484250490600018105614048117055336074437503883703510511249361224931983788156958581275946729175531468251871452856923140435984577574698574803934567774824230985421074605062371141877954182153046474983581941267398767559165543946077062914571196477686542167660429831652624386837205668069376",
                "3",
                0x7e55_5555_5555_5555,
            ),
            // 3 / 2**1000: tiny quotient, exercises the subnormal-adjacent path.
            (
                "3",
                "10715086071862673209484250490600018105614048117055336074437503883703510511249361224931983788156958581275946729175531468251871452856923140435984577574698574803934567774824230985421074605062371141877954182153046474983581941267398767559165543946077062914571196477686542167660429831652624386837205668069376",
                0x0188_0000_0000_0000,
            ),
        ];
        for (num_s, den_s, expected_bits) in cases {
            let num: BigInt = num_s.parse().expect("numerator parses");
            let den: BigInt = den_s.parse().expect("denominator parses");
            let got = ratio_to_f64(&num, &den)
                .unwrap_or_else(|_| panic!("{num_s} / {den_s} unexpectedly overflowed f64"));
            assert_eq!(
                got.to_bits(),
                *expected_bits,
                "{num_s} / {den_s}: got {got:?} (0x{:016x}), expected 0x{expected_bits:016x}",
                got.to_bits()
            );
        }
    }

    /// Magnitudes beyond `f64::MAX` must report overflow (mapped to
    /// `OverflowError` by the caller), matching CPython.
    #[test]
    fn ratio_to_f64_overflows_past_f64_max() {
        // 2**2000 / 1 far exceeds f64::MAX (~1.8e308).
        let num: BigInt = BigInt::from(2).pow(2000);
        let den: BigInt = BigInt::from(1);
        assert!(ratio_to_f64(&num, &den).is_err());
        // Negative direction overflows too.
        assert!(ratio_to_f64(&(-num.clone()), &den).is_err());
    }
    #[test]
    fn binary64_rounding_boundaries() {
        let one = BigInt::from(1);
        // Independent binary64 bit patterns for exact rational boundary values.
        let cases: [(BigInt, BigInt, u64); 12] = [
            (one.clone(), &one << 1074_u32, 0x0000_0000_0000_0001),
            (one.clone(), &one << 1075_u32, 0x0000_0000_0000_0000),
            (BigInt::from(3), &one << 1076_u32, 0x0000_0000_0000_0001),
            (BigInt::from(3), &one << 1075_u32, 0x0000_0000_0000_0002),
            (BigInt::from(5), &one << 1075_u32, 0x0000_0000_0000_0002),
            (
                (&one << 52_u32) - 1,
                &one << 1074_u32,
                0x000f_ffff_ffff_ffff,
            ),
            (
                (&one << 53_u32) - 1,
                &one << 1075_u32,
                0x0010_0000_0000_0000,
            ),
            (
                (&one << 54_u32) - 3,
                &one << 1076_u32,
                0x000f_ffff_ffff_ffff,
            ),
            ((&one << 53_u32) + 1, &one << 53_u32, 0x3ff0_0000_0000_0000),
            ((&one << 53_u32) + 3, &one << 53_u32, 0x3ff0_0000_0000_0002),
            (
                (&one << 2000_u32) + 1,
                (&one << 2000_u32) - 1,
                0x3ff0_0000_0000_0000,
            ),
            (
                ((&one << 54_u32) - 1) * (&one << 970_u32) - 1,
                one.clone(),
                0x7fef_ffff_ffff_ffff,
            ),
        ];
        for (num, den, expected) in cases {
            for (n, d, sign) in [
                (num.clone(), den.clone(), 0),
                (-&num, den.clone(), 1_u64 << 63),
                (num.clone(), -&den, 1_u64 << 63),
                (-&num, -&den, 0),
            ] {
                assert_eq!(ratio_to_f64(&n, &d).unwrap().to_bits(), expected | sign);
            }
        }
        let overflow: BigInt = ((&one << 54_u32) - 1) * (&one << 970_u32);
        for numerator in [overflow.clone(), -overflow] {
            assert_eq!(
                ratio_to_f64(&numerator, &one),
                Err(RatioFloatError::Overflow)
            );
        }
        assert_eq!(
            ratio_to_f64(&BigInt::from(0), &(-&one)).unwrap().to_bits(),
            1_u64 << 63
        );
        assert_eq!(
            ratio_to_f64(&one, &BigInt::from(0)),
            Err(RatioFloatError::ZeroDenominator)
        );
    }
}
