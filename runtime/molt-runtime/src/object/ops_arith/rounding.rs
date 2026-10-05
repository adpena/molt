//! Builtin round dispatch and the inherited numeric descriptors share one owner.

use super::*;
use crate::builtins::numbers::{
    index_bigint_integral_bits, index_integral_payload_bits, index_ssize_clamped_from_obj,
};
use crate::object::ops_convert::{float_value_or_descriptor_error, int_method_value_bits_or_error};

fn index_error(py: &PyToken<'_>, bits: u64) -> String {
    format!(
        "'{}' object cannot be interpreted as an integer",
        type_name(py, obj_from_bits(bits))
    )
}

pub(super) fn round(py: &PyToken<'_>, value: u64, digits: u64, supplied: bool) -> u64 {
    let no_digits = !supplied || obj_from_bits(digits).is_none();
    let obj = obj_from_bits(value);
    if builtin_operand(py, obj) {
        if index_integral_payload_bits(value).is_some() {
            return integer_round(py, value, if no_digits { missing_bits(py) } else { digits });
        }
        if is_float_extended(obj) {
            return float_round(
                py,
                value,
                if no_digits {
                    MoltObject::none().bits()
                } else {
                    digits
                },
            );
        }
    }
    let method = unsafe { crate::builtins::attr::lookup_special_method(py, value, b"__round__") };
    if let Some(method) = method {
        // The lookup returned an owned bound callable; the caller retains all
        // argument carriers for this invocation, and py holds the runtime token.
        let result = unsafe {
            if no_digits {
                call_callable0(py, method)
            } else {
                call_callable1(py, method, digits)
            }
        };
        molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, method));
        return result;
    }
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    let name = type_name(py, obj);
    let name = String::from_utf8_lossy(&name.as_bytes()[..name.len().min(100)]);
    raise_exception(
        py,
        "TypeError",
        &format!("type {name} doesn't define __round__ method"),
    )
}

/// Round to an exact integral decimal multiple. Both integer representations
/// and negative float digit counts use this signed ties-to-even decision.
/// A float's fractional tail matters only when its truncated integer is exactly
/// at the integral midpoint (the divisor is an even positive power of ten).
fn nearest_multiple(value: &BigInt, divisor: &BigInt, fractional_tail: bool) -> BigInt {
    let (mut quotient, remainder) = value.div_rem(divisor);
    let twice = remainder.abs() * 2;
    if twice > *divisor || (twice == *divisor && (fractional_tail || quotient.is_odd())) {
        quotient += if value.is_negative() { -1 } else { 1 };
    }
    quotient * divisor
}

fn integer_round(py: &PyToken<'_>, value: u64, digits: u64) -> u64 {
    let Some(payload) = int_method_value_bits_or_error(py, value, "__round__") else {
        return MoltObject::none().bits();
    };
    // The normalized payload remains owned across digit-conversion callbacks.
    // Returning an unchanged heap integer transfers this owner to the caller.
    inc_ref_bits(py, payload);
    let mut owner = PtrDropGuard::preserving(
        obj_from_bits(payload)
            .as_ptr()
            .unwrap_or(std::ptr::null_mut()),
    );
    if digits == missing_bits(py)
        || (runtime_target_at_least(py, 3, 14) && obj_from_bits(digits).is_none())
    {
        owner.release();
        return payload;
    }
    let Some(digits) = index_bigint_from_obj(py, digits, &index_error(py, digits)) else {
        return MoltObject::none().bits();
    };
    if !digits.is_negative() {
        owner.release();
        return payload;
    }
    // int.__round__ keeps the full index value, unlike float.__round__'s
    // ssize_t clipping. The existing integer-power authority owns admission,
    // allocation and overflow; no narrowing cast or second power policy.
    let exponent = int_bits_from_bigint(py, -digits);
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    let _exponent_owner = PtrDropGuard::preserving(
        obj_from_bits(exponent)
            .as_ptr()
            .unwrap_or(std::ptr::null_mut()),
    );
    let power = molt_pow(MoltObject::from_int(10).bits(), exponent);
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    let _power_owner = PtrDropGuard::preserving(
        obj_from_bits(power)
            .as_ptr()
            .unwrap_or(std::ptr::null_mut()),
    );
    let value = index_bigint_integral_bits(payload).expect("admitted integer round payload");
    let divisor = index_bigint_integral_bits(power).expect("exact integer power result");
    int_bits_from_bigint(py, nearest_multiple(&value, &divisor, false))
}

fn float_round(py: &PyToken<'_>, value: u64, digits: u64) -> u64 {
    let Some(value) = float_value_or_descriptor_error(py, value, "__round__") else {
        return MoltObject::none().bits();
    };
    if obj_from_bits(digits).is_none() {
        if value.is_nan() {
            return raise_exception(py, "ValueError", "cannot convert float NaN to integer");
        }
        if value.is_infinite() {
            return raise_exception(
                py,
                "OverflowError",
                "cannot convert float infinity to integer",
            );
        }
        return int_bits_from_bigint(py, bigint_from_f64_trunc(round_half_even(value)));
    }
    // Even a NaN/infinity must first observe __index__ or its original error.
    let Some(digits) = index_ssize_clamped_from_obj(py, digits, &index_error(py, digits)) else {
        return MoltObject::none().bits();
    };
    if !value.is_finite() || digits > 323 {
        return float_result_bits(py, value);
    }
    if digits < -308 {
        return float_result_bits(py, 0.0f64.copysign(value));
    }
    let rounded = if digits >= 0 {
        // Fixed decimal formatting rounds the exact binary64 value once;
        // the standard parser then performs the decimal-to-binary rounding.
        format!("{:.*}", digits as usize, value)
            .parse::<f64>()
            .expect("finite fixed decimal formatting is a valid float")
    } else {
        let integer = bigint_from_f64_trunc(value);
        let divisor = BigInt::from(10).pow((-digits) as u32);
        let rounded = nearest_multiple(&integer, &divisor, value.fract() != 0.0);
        if rounded.is_zero() {
            0.0f64.copysign(value)
        } else {
            rounded.to_f64().unwrap_or(f64::INFINITY.copysign(value))
        }
    };
    if !rounded.is_finite() {
        return raise_exception(py, "OverflowError", "rounded value too large to represent");
    }
    float_result_bits(py, rounded)
}

pub(crate) extern "C" fn int_round_slot(value: u64, digits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { integer_round(py, value, digits) })
}

pub(crate) extern "C" fn float_round_slot(value: u64, digits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { float_round(py, value, digits) })
}
