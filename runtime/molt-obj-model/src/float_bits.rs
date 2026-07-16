//! Shared IEEE-754 binary16 conversion authority.

/// Failure to represent a finite source value in a narrower IEEE format.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FloatNarrowError {
    FiniteOverflow,
}

#[inline]
fn round_shift_ties_even(value: u64, shift: u32) -> u64 {
    if shift == 0 {
        return value;
    }
    if shift > 64 {
        return 0;
    }
    let quotient = if shift == 64 { 0 } else { value >> shift };
    let remainder = if shift == 64 {
        value
    } else {
        value & ((1u64 << shift) - 1)
    };
    let halfway = 1u64 << (shift - 1);
    quotient + u64::from(remainder > halfway || (remainder == halfway && quotient & 1 != 0))
}

/// Convert f64 directly to IEEE binary16, with one ties-to-even rounding step.
/// NaNs are canonicalized to the signed quiet payload used by CPython.
pub fn f64_to_f16_bits(value: f64) -> Result<u16, FloatNarrowError> {
    let raw = value.to_bits();
    let sign = ((raw >> 48) & 0x8000) as u16;
    let exponent = ((raw >> 52) & 0x7ff) as i32;
    let fraction = raw & ((1u64 << 52) - 1);
    if exponent == 0x7ff {
        return Ok(sign | 0x7c00 | if fraction == 0 { 0 } else { 0x0200 });
    }
    if exponent == 0 {
        return Ok(sign);
    }
    let unbiased = exponent - 1023;
    let significand = (1u64 << 52) | fraction;
    if unbiased > 15 {
        return Err(FloatNarrowError::FiniteOverflow);
    }
    let magnitude = if unbiased >= -14 {
        let mut rounded = round_shift_ties_even(significand, 42);
        let mut half_exp = unbiased + 15;
        if rounded == 2048 {
            rounded = 1024;
            half_exp += 1;
        }
        if half_exp >= 31 {
            return Err(FloatNarrowError::FiniteOverflow);
        }
        ((half_exp as u16) << 10) | ((rounded as u16) & 0x03ff)
    } else if unbiased < -25 {
        0
    } else {
        let rounded = round_shift_ties_even(significand, (28 - unbiased) as u32);
        if rounded >= 1024 {
            0x0400
        } else {
            rounded as u16
        }
    };
    Ok(sign | magnitude)
}

/// Decode IEEE binary16 exactly into f64.
pub fn f16_bits_to_f64(bits: u16) -> f64 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = (bits >> 10) & 0x1f;
    let fraction = bits & 0x03ff;
    match exponent {
        0 if fraction == 0 => f64::from_bits(u64::from(bits & 0x8000) << 48),
        0 => sign * f64::from(fraction) * 2f64.powi(-24),
        0x1f if fraction == 0 => sign * f64::INFINITY,
        0x1f => f64::from_bits((u64::from(bits & 0x8000) << 48) | 0x7ff8_0000_0000_0000),
        _ => sign * (1.0 + f64::from(fraction) / 1024.0) * 2f64.powi(i32::from(exponent) - 15),
    }
}

/// Narrow f64 to IEEE binary32, rejecting the finite-to-infinity overflow
/// used by both CPython `struct 'f'` and `PyFloat_Pack4`.
pub fn f64_to_f32_bits(value: f64) -> Result<u32, FloatNarrowError> {
    let narrowed = value as f32;
    if narrowed.is_infinite() && value.is_finite() {
        Err(FloatNarrowError::FiniteOverflow)
    } else {
        Ok(narrowed.to_bits())
    }
}

pub fn f32_bits_to_f64(bits: u32) -> f64 {
    f32::from_bits(bits) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ties_subnormals_specials_and_overflow() {
        assert_eq!(f64_to_f16_bits(0.0), Ok(0));
        assert_eq!(f64_to_f16_bits(-0.0), Ok(0x8000));
        assert_eq!(f64_to_f16_bits(2f64.powi(-24)), Ok(1));
        assert_eq!(f64_to_f16_bits(2f64.powi(-25)), Ok(0));
        assert_eq!(f64_to_f16_bits(1.0 + 2f64.powi(-11)), Ok(0x3c00));
        assert_eq!(f64_to_f16_bits(1.0 + 3.0 * 2f64.powi(-11)), Ok(0x3c02));
        assert_eq!(f64_to_f16_bits(f64::INFINITY), Ok(0x7c00));
        assert_eq!(f64_to_f16_bits(f64::NAN), Ok(0x7e00));
        assert!(f64_to_f16_bits(65520.0).is_err());
        assert_eq!(f16_bits_to_f64(0x8000).to_bits(), (-0.0f64).to_bits());
    }
}
