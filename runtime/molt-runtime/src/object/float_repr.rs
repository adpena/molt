//! The single float-format authority for `repr(float)` / `str(float)`.
//!
//! Reproduces CPython's `float_repr` (`Objects/floatobject.c`) exactly:
//!
//! ```c
//! buf = PyOS_double_to_string(PyFloat_AS_DOUBLE(v), 'r', 0,
//!                             Py_DTSF_ADD_DOT_0, NULL);
//! ```
//!
//! which routes through `format_float_short` in `Python/pystrtod.c` with
//! `format_code == 'r'`:
//!
//! * digits come from `_Py_dg_dtoa(d, mode=0, precision=0, ...)` — the
//!   *shortest* decimal string that round-trips to the same `f64`, with
//!   correct rounding (round-half-to-**even** on exact ties);
//! * the fixed-vs-scientific decision is `use_exp = (decpt <= -4 || decpt > 16)`
//!   where `decpt` is the position of the decimal point (the first digit has
//!   place value `10^(decpt-1)`);
//! * scientific form uses `exp = decpt - 1` and formats the exponent as
//!   `e%+.02d` (sign always present, minimum two exponent digits);
//! * `Py_DTSF_ADD_DOT_0` appends `.0` to integer-valued floats in fixed form
//!   (`repr(1.0) == "1.0"`), but never in scientific form (`repr(1e16) ==
//!   "1e+16"`, not `"1.0e+16"`);
//! * `nan`, `inf`, `-inf`, and the sign of `-0.0` are preserved.
//!
//! In Python 3, `float.__str__` is `float.__repr__`, so this single function is
//! the authority for both `repr(float)` and `str(float)`.
//!
//! ## Why we do not use Rust's `f64::to_string()`
//!
//! Rust's `std` shortest-float formatting produces the correct *number* of
//! significant digits, but breaks exact ties differently from CPython. For
//! example the exact value `137839762462415.625` is halfway between
//! `137839762462415.62` and `...63`; CPython's `_Py_dg_dtoa` rounds half-to-even
//! and yields `...62`, while Rust's formatter yields `...63`. Both round-trip,
//! but only one matches CPython. We therefore take only the shortest-digit
//! *count* `k` from `std` (which is unambiguous) and re-render the exact value
//! rounded to `k` significant digits with round-half-to-even, using exact
//! rational (`BigInt`) arithmetic. This is validated bit-exact against CPython
//! across every power of ten, subnormals, threshold neighborhoods, and hundreds
//! of thousands of random doubles (see the tests in this module and the
//! differential harness).

use num_bigint::BigInt;
use num_traits::{One, Zero};

/// The minimal number of significant decimal digits `k` such that rendering
/// `abs` to `k` significant digits round-trips back to `abs`.
///
/// `abs` must be positive and finite. Rust's `{:e}` produces the shortest
/// round-tripping form, so the count of its mantissa digits is exactly `k`.
/// The count is unambiguous even when the last digit differs from CPython on a
/// tie, so it is safe to source it from `std`.
fn shortest_sig_count(abs: f64) -> usize {
    let sci = format!("{abs:e}");
    let mant = sci.split('e').next().unwrap_or("0");
    mant.chars().filter(|c| c.is_ascii_digit()).count().max(1)
}

/// `10^p` as a `BigInt` for `p >= 0`; `10^0 == 1` for `p < 0` (callers guard
/// the sign and scale the other side of the ratio instead).
fn pow10(p: i32) -> BigInt {
    if p >= 0 {
        BigInt::from(10u32).pow(p as u32)
    } else {
        BigInt::one()
    }
}

/// Round the exact value of the positive, finite, non-zero `abs` to `k`
/// significant decimal digits using round-half-to-even, returning
/// `(digit_string, decpt)` in CPython's `_Py_dg_dtoa` convention: the digit
/// string carries no leading or trailing zeros, and the first digit has place
/// value `10^(decpt-1)` (equivalently `value == 0.<digits> * 10^decpt`).
fn round_sig_half_even(abs: f64, k: usize) -> (String, i32) {
    // Decompose abs exactly as mant * 2^exp2 with integer mant, exp2.
    let bits = abs.to_bits();
    let raw_exp = ((bits >> 52) & 0x7ff) as i64;
    let raw_mant = (bits & 0x000f_ffff_ffff_ffff) as i64;
    let (mant, exp2): (i64, i64) = if raw_exp == 0 {
        // Subnormal: no implicit leading bit, unbiased exponent is -1074.
        (raw_mant, -1074)
    } else {
        (raw_mant | 0x0010_0000_0000_0000, raw_exp - 1075)
    };

    // Exact value = mant * 2^exp2 = num / den.
    let mant_bi = BigInt::from(mant);
    let (mut num, mut den) = if exp2 >= 0 {
        (mant_bi * BigInt::from(2u32).pow(exp2 as u32), BigInt::one())
    } else {
        (mant_bi, BigInt::from(2u32).pow((-exp2) as u32))
    };

    // Find e = floor(log10(value)) exactly: 10^e <= value < 10^(e+1).
    // Seed from the f64 log10 then correct with exact BigInt comparisons.
    let mut e = abs.log10().floor() as i32;
    loop {
        // lower bound: 10^e <= num/den
        let lower_ok = if e >= 0 {
            &pow10(e) * &den <= num
        } else {
            den <= &num * pow10(-e)
        };
        if !lower_ok {
            e -= 1;
            continue;
        }
        // upper bound: num/den < 10^(e+1)
        let upper_ok = if (e + 1) >= 0 {
            num < &pow10(e + 1) * &den
        } else {
            &num * pow10(-(e + 1)) < den
        };
        if !upper_ok {
            e += 1;
            continue;
        }
        break;
    }

    // Scale so that round(value * 10^(k-1-e)) yields the k significant digits.
    let s = (k as i32) - 1 - e;
    if s >= 0 {
        num *= pow10(s);
    } else {
        den *= pow10(-s);
    }

    // Integer division with round-half-to-even remainder handling.
    let q = &num / &den;
    let r = &num - &q * &den;
    let twice = &r * BigInt::from(2u32);
    let mut digit_int = q;
    if twice > den {
        digit_int += 1;
    } else if twice == den && (&digit_int % 2u32) != BigInt::zero() {
        // Exact tie: round to even.
        digit_int += 1;
    }

    let mut ds = digit_int.to_str_radix(10);
    let mut decpt = e + 1;
    // A carry (e.g. 9.99 -> 10.0) can add a digit; that shifts decpt up.
    if ds.len() as i32 > k as i32 {
        decpt += ds.len() as i32 - k as i32;
    }
    // CPython's dtoa returns digits with no trailing zeros.
    let trimmed = ds.trim_end_matches('0');
    if trimmed.is_empty() {
        ds = "0".to_string();
    } else if trimmed.len() != ds.len() {
        ds = trimmed.to_string();
    }
    (ds, decpt)
}

/// The single authority for `repr(float)` / `str(float)`, byte-for-byte
/// identical to CPython 3.12's `float_repr`.
///
/// Handles `nan`, `±inf`, `±0.0`, and every finite value.
pub fn repr_float(f: f64) -> String {
    if f.is_nan() {
        return "nan".to_string();
    }
    if f.is_infinite() {
        return if f < 0.0 {
            "-inf".to_string()
        } else {
            "inf".to_string()
        };
    }
    let sign = if f.is_sign_negative() { "-" } else { "" };
    let abs = f.abs();
    if abs == 0.0 {
        // Preserves -0.0 via `sign`; Py_DTSF_ADD_DOT_0 gives "0.0".
        return format!("{sign}0.0");
    }

    let k = shortest_sig_count(abs);
    let (digits, decpt) = round_sig_half_even(abs, k);
    let dbytes = digits.as_bytes();
    let ndigits = dbytes.len() as i32;
    let use_exp = decpt <= -4 || decpt > 16;

    let mut out = String::with_capacity(sign.len() + digits.len() + 6);
    out.push_str(sign);

    if use_exp {
        // Scientific: one digit before the point, no Py_DTSF_ADD_DOT_0.
        let exp = decpt - 1;
        out.push(dbytes[0] as char);
        if ndigits > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if exp < 0 { '-' } else { '+' });
        // Minimum two exponent digits (CPython "%+.02d").
        let _ = write_min2(&mut out, exp.unsigned_abs());
    } else if decpt <= 0 {
        // 0.00...<digits>
        out.push_str("0.");
        for _ in 0..(-decpt) {
            out.push('0');
        }
        out.push_str(&digits);
    } else if decpt >= ndigits {
        // <digits>000... .0  (Py_DTSF_ADD_DOT_0 for integer-valued floats)
        out.push_str(&digits);
        for _ in 0..(decpt - ndigits) {
            out.push('0');
        }
        out.push_str(".0");
    } else {
        // Decimal point falls inside the digit string.
        let cut = decpt as usize;
        out.push_str(&digits[..cut]);
        out.push('.');
        out.push_str(&digits[cut..]);
    }
    out
}

/// Append `v` as decimal with a minimum width of two digits (CPython's
/// exponent `%+.02d` field, sign already emitted by the caller).
fn write_min2(out: &mut String, v: u32) -> std::fmt::Result {
    use std::fmt::Write;
    if v < 10 {
        write!(out, "0{v}")
    } else {
        write!(out, "{v}")
    }
}

#[cfg(test)]
mod tests {
    use super::repr_float;

    /// The task's calibrated edge-case table, verified against CPython 3.12
    /// `repr(float)`.
    #[test]
    fn edge_case_table_matches_cpython() {
        let cases: &[(f64, &str)] = &[
            (0.1, "0.1"),
            (0.2, "0.2"),
            (0.3, "0.3"),
            (1.0, "1.0"),
            (100.0, "100.0"),
            (1e16, "1e+16"),
            (9999999999999998.0, "9999999999999998.0"),
            (1e-4, "0.0001"),
            (1e-5, "1e-05"),
            (1234567890123456.0, "1234567890123456.0"),
            (1.5e300, "1.5e+300"),
            (2.5e-300, "2.5e-300"),
            (f64::INFINITY, "inf"),
            (f64::NEG_INFINITY, "-inf"),
            (123.456, "123.456"),
            (0.0001, "0.0001"),
            (1e100, "1e+100"),
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (12345.6789, "12345.6789"),
            (1e15, "1000000000000000.0"),
            (5e-324, "5e-324"),
            (1.7976931348623157e308, "1.7976931348623157e+308"),
            (-1.5, "-1.5"),
            (2.0, "2.0"),
            (-0.0001, "-0.0001"),
            // Round-half-to-even ties that Rust std gets wrong on its own.
            (137839762462415.62, "137839762462415.62"),
            (255615187364188.12, "255615187364188.12"),
        ];
        for (v, want) in cases {
            assert_eq!(&repr_float(*v), want, "repr_float({v:?})");
        }
    }

    #[test]
    fn nan_matches_cpython() {
        assert_eq!(repr_float(f64::NAN), "nan");
        // CPython prints "nan" for negative-NaN too.
        assert_eq!(repr_float(-f64::NAN), "nan");
    }

    #[test]
    fn negative_zero_preserved() {
        assert_eq!(repr_float(-0.0), "-0.0");
        assert_eq!(repr_float(0.0), "0.0");
    }

    #[test]
    fn exponent_is_min_two_digits_with_sign() {
        assert_eq!(repr_float(1e-5), "1e-05");
        assert_eq!(repr_float(1e5), "100000.0");
        assert_eq!(repr_float(1e100), "1e+100");
        assert_eq!(repr_float(1e-300), "1e-300");
    }

    #[test]
    fn fixed_scientific_threshold_boundaries() {
        // decpt == 16 -> fixed; decpt == 17 -> scientific.
        assert_eq!(repr_float(1e15), "1000000000000000.0");
        assert_eq!(repr_float(1e16), "1e+16");
        // decpt == -3 -> fixed; decpt == -4 -> scientific.
        assert_eq!(repr_float(1e-4), "0.0001");
        assert_eq!(repr_float(1e-5), "1e-05");
    }
}
