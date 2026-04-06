#![allow(dead_code, unused_imports)]

use crate::bridge::*;
use molt_runtime_core::prelude::*;
use std::cell::RefCell;
use std::cmp::Ordering;
use std::ptr;

use num_bigint::BigInt;
use num_traits::{One, Signed, ToPrimitive, Zero};

const MPD_CLAMPED: u32 = 0x00000001;
const MPD_CONVERSION_SYNTAX: u32 = 0x00000002;
const MPD_DIVISION_BY_ZERO: u32 = 0x00000004;
const MPD_DIVISION_IMPOSSIBLE: u32 = 0x00000008;
const MPD_DIVISION_UNDEFINED: u32 = 0x00000010;
const MPD_FPU_ERROR: u32 = 0x00000020;
const MPD_INEXACT: u32 = 0x00000040;
const MPD_INVALID_CONTEXT: u32 = 0x00000080;
const MPD_INVALID_OPERATION: u32 = 0x00000100;
const MPD_MALLOC_ERROR: u32 = 0x00000200;
const MPD_NOT_IMPLEMENTED: u32 = 0x00000400;
const MPD_OVERFLOW: u32 = 0x00000800;
const MPD_ROUNDED: u32 = 0x00001000;
const MPD_SUBNORMAL: u32 = 0x00002000;
const MPD_UNDERFLOW: u32 = 0x00004000;

const MPD_IEEE_INVALID_OPERATION: u32 = MPD_CONVERSION_SYNTAX
    | MPD_DIVISION_IMPOSSIBLE
    | MPD_DIVISION_UNDEFINED
    | MPD_FPU_ERROR
    | MPD_INVALID_CONTEXT
    | MPD_INVALID_OPERATION
    | MPD_MALLOC_ERROR;

const MPD_ROUND_UP: i32 = 0;
const MPD_ROUND_DOWN: i32 = 1;
const MPD_ROUND_CEILING: i32 = 2;
const MPD_ROUND_FLOOR: i32 = 3;
const MPD_ROUND_HALF_UP: i32 = 4;
const MPD_ROUND_HALF_DOWN: i32 = 5;
const MPD_ROUND_HALF_EVEN: i32 = 6;
const MPD_ROUND_05UP: i32 = 7;

const DECIMAL_DEFAULT_PREC: i64 = 28;
const DECIMAL_DEFAULT_TRAPS: u32 = MPD_IEEE_INVALID_OPERATION | MPD_DIVISION_BY_ZERO | MPD_OVERFLOW;

thread_local! {
    static DECIMAL_CONTEXT: RefCell<*mut DecimalContextHandle> = const { RefCell::new(ptr::null_mut()) };
}

#[derive(Clone)]
struct DecimalContextHandle {
    prec: i64,
    traps: u32,
    status: u32,
    rounding: i32,
    capitals: i32,
    refs: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DecimalSpecial {
    Finite,
    Infinity,
    Nan,
    SNan,
}

#[derive(Clone)]
struct DecimalHandle {
    sign: bool,
    coeff: BigInt,
    exp: i64,
    special: DecimalSpecial,
}

fn default_context() -> DecimalContextHandle {
    DecimalContextHandle {
        prec: DECIMAL_DEFAULT_PREC,
        traps: DECIMAL_DEFAULT_TRAPS,
        status: 0,
        rounding: MPD_ROUND_HALF_EVEN,
        capitals: 1,
        refs: 1,
    }
}

fn ensure_current_context() -> *mut DecimalContextHandle {
    DECIMAL_CONTEXT.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_null() {
            let handle = Box::new(default_context());
            *slot = Box::into_raw(handle);
        }
        *slot
    })
}

fn context_inc(ptr: *mut DecimalContextHandle) {
    if ptr.is_null() {
        return;
    }
    // SAFETY: pointer originates from Box::into_raw and is only mutated under GIL entrypoints.
    unsafe {
        (*ptr).refs = (*ptr).refs.saturating_add(1);
    }
}

fn context_dec(ptr: *mut DecimalContextHandle) {
    if ptr.is_null() {
        return;
    }
    // SAFETY: pointer originates from Box::into_raw and is only mutated under GIL entrypoints.
    unsafe {
        if (*ptr).refs <= 1 {
            release_ptr(ptr as *mut u8);
            drop(Box::from_raw(ptr));
        } else {
            (*ptr).refs -= 1;
        }
    }
}

fn context_ptr_from_bits(bits: u64) -> Option<*mut DecimalContextHandle> {
    let ptr = ptr_from_bits(bits);
    if ptr.is_null() {
        None
    } else {
        Some(ptr as *mut DecimalContextHandle)
    }
}

fn decimal_handle_from_bits(bits: u64) -> Option<&'static mut DecimalHandle> {
    let ptr = ptr_from_bits(bits);
    if ptr.is_null() {
        return None;
    }
    // SAFETY: bits encode a DecimalHandle pointer owned by this runtime.
    Some(unsafe { &mut *(ptr as *mut DecimalHandle) })
}

fn pow10_u32(exp: u32) -> BigInt {
    BigInt::from(10u8).pow(exp)
}

fn pow10_i64(exp: i64) -> Option<BigInt> {
    if exp < 0 {
        return None;
    }
    let n = u32::try_from(exp).ok()?;
    if n > 200_000 {
        return None;
    }
    Some(pow10_u32(n))
}

fn digits_len(n: &BigInt) -> i64 {
    if n.is_zero() {
        1
    } else {
        n.to_string().len() as i64
    }
}

fn decimal_signal_name(flags: u32) -> &'static str {
    if flags & MPD_INVALID_OPERATION != 0
        || flags & MPD_CONVERSION_SYNTAX != 0
        || flags & MPD_DIVISION_IMPOSSIBLE != 0
        || flags & MPD_DIVISION_UNDEFINED != 0
        || flags & MPD_INVALID_CONTEXT != 0
        || flags & MPD_NOT_IMPLEMENTED != 0
    {
        return "InvalidOperation";
    }
    if flags & MPD_DIVISION_BY_ZERO != 0 {
        return "DivisionByZero";
    }
    if flags & MPD_OVERFLOW != 0 {
        return "Overflow";
    }
    if flags & MPD_UNDERFLOW != 0 {
        return "Underflow";
    }
    if flags & MPD_SUBNORMAL != 0 {
        return "Subnormal";
    }
    if flags & MPD_INEXACT != 0 {
        return "Inexact";
    }
    if flags & MPD_ROUNDED != 0 {
        return "Rounded";
    }
    if flags & MPD_CLAMPED != 0 {
        return "Clamped";
    }
    "InvalidOperation"
}

fn apply_status(_py: &PyToken, ctx: &mut DecimalContextHandle, status: u32) -> Result<(), u64> {
    if status == 0 {
        return Ok(());
    }
    ctx.status |= status;
    if status & MPD_MALLOC_ERROR != 0 {
        return Err(raise_exception::<u64>(
            _py,
            "MemoryError",
            "decimal allocation failed",
        ));
    }
    let trapped = ctx.traps & status;
    if trapped != 0 {
        let name = decimal_signal_name(trapped);
        return Err(raise_exception::<u64>(_py, name, "decimal signal"));
    }
    Ok(())
}

fn round_increment(rounding: i32, sign: bool, q: &BigInt, rem: &BigInt, divisor: &BigInt) -> bool {
    if rem.is_zero() {
        return false;
    }
    match rounding {
        MPD_ROUND_UP => true,
        MPD_ROUND_DOWN => false,
        MPD_ROUND_CEILING => !sign,
        MPD_ROUND_FLOOR => sign,
        MPD_ROUND_HALF_UP => rem * 2 >= *divisor,
        MPD_ROUND_HALF_DOWN => rem * 2 > *divisor,
        MPD_ROUND_HALF_EVEN => {
            let twice = rem * 2;
            if twice > *divisor {
                true
            } else if twice < *divisor {
                false
            } else {
                (q % 2u8) == BigInt::one()
            }
        }
        MPD_ROUND_05UP => {
            let last = (q % 10u8).to_u8().unwrap_or(0);
            last == 0 || last == 5
        }
        _ => false,
    }
}

fn parse_decimal_text(text: &str) -> Result<DecimalHandle, u32> {
    let trimmed = text.trim();
    let (sign, mut rest) = if let Some(stripped) = trimmed.strip_prefix('-') {
        (true, stripped)
    } else if let Some(stripped) = trimmed.strip_prefix('+') {
        (false, stripped)
    } else {
        (false, trimmed)
    };

    if rest == "Infinity" {
        return Ok(DecimalHandle {
            sign,
            coeff: BigInt::zero(),
            exp: 0,
            special: DecimalSpecial::Infinity,
        });
    }
    if rest == "NaN" {
        return Ok(DecimalHandle {
            sign,
            coeff: BigInt::zero(),
            exp: 0,
            special: DecimalSpecial::Nan,
        });
    }
    if rest == "sNaN" {
        return Ok(DecimalHandle {
            sign,
            coeff: BigInt::zero(),
            exp: 0,
            special: DecimalSpecial::SNan,
        });
    }

    let mut exp_part: i64 = 0;
    if let Some(idx) = rest.find(['e', 'E']) {
        let (base, exp) = rest.split_at(idx);
        rest = base;
        let exp_str = exp[1..].trim();
        exp_part = exp_str.parse::<i64>().map_err(|_| MPD_CONVERSION_SYNTAX)?;
    }

    let mut digits = String::new();
    let mut frac_len: i64 = 0;
    let mut in_frac = false;
    for ch in rest.chars() {
        if ch == '.' {
            if in_frac {
                return Err(MPD_CONVERSION_SYNTAX);
            }
            in_frac = true;
            continue;
        }
        if !ch.is_ascii_digit() {
            return Err(MPD_CONVERSION_SYNTAX);
        }
        digits.push(ch);
        if in_frac {
            frac_len += 1;
        }
    }
    if digits.is_empty() {
        return Err(MPD_CONVERSION_SYNTAX);
    }

    let coeff = digits
        .parse::<BigInt>()
        .map_err(|_| MPD_CONVERSION_SYNTAX)?;
    Ok(DecimalHandle {
        sign,
        coeff,
        exp: exp_part - frac_len,
        special: DecimalSpecial::Finite,
    })
}

fn decimal_to_string(dec: &DecimalHandle, capitals: i32) -> String {
    match dec.special {
        DecimalSpecial::Infinity => {
            if dec.sign {
                "-Infinity".to_string()
            } else {
                "Infinity".to_string()
            }
        }
        DecimalSpecial::Nan => "NaN".to_string(),
        DecimalSpecial::SNan => {
            if capitals != 0 {
                "sNaN".to_string()
            } else {
                "snan".to_string()
            }
        }
        DecimalSpecial::Finite => {
            let digits = if dec.coeff.is_zero() {
                "0".to_string()
            } else {
                dec.coeff.to_string()
            };
            let n = i64::try_from(digits.len()).unwrap_or(1);
            let adjusted = dec.exp + n - 1;
            let mut text = if dec.exp <= 0 && adjusted >= -6 {
                let point = n + dec.exp;
                if point > 0 {
                    let idx = usize::try_from(point).unwrap_or(0);
                    let (left, right) = digits.split_at(idx);
                    if right.is_empty() {
                        left.to_string()
                    } else {
                        format!("{left}.{right}")
                    }
                } else {
                    let zeros = usize::try_from(-point).unwrap_or(0);
                    format!("0.{}{}", "0".repeat(zeros), digits)
                }
            } else {
                let mut chars = digits.chars();
                let first = chars.next().unwrap_or('0');
                let tail: String = chars.collect();
                if tail.is_empty() {
                    format!("{first}E{adjusted:+}")
                } else {
                    format!("{first}.{tail}E{adjusted:+}")
                }
            };
            if dec.sign {
                text.insert(0, '-');
            }
            text
        }
    }
}

fn decimal_tuple_bits(_py: &PyToken, dec: &DecimalHandle) -> u64 {
    let sign_bits = int_bits_from_i64(_py, if dec.sign { 1 } else { 0 });

    let (digit_bits, exp_bits) = match dec.special {
        DecimalSpecial::Infinity => {
            let zero = int_bits_from_i64(_py, 0);
            let digits_ptr = alloc_tuple(_py, &[zero]);
            if digits_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let digits_bits = MoltObject::from_ptr(digits_ptr).bits();
            let exp_ptr = alloc_string(_py, b"F");
            if exp_ptr.is_null() {
                dec_ref_bits(_py, digits_bits);
                return MoltObject::none().bits();
            }
            (digits_bits, MoltObject::from_ptr(exp_ptr).bits())
        }
        DecimalSpecial::Nan | DecimalSpecial::SNan => {
            let digits_ptr = alloc_tuple(_py, &[]);
            if digits_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let digits_bits = MoltObject::from_ptr(digits_ptr).bits();
            let exp_ptr = alloc_string(
                _py,
                if dec.special == DecimalSpecial::SNan {
                    b"N"
                } else {
                    b"n"
                },
            );
            if exp_ptr.is_null() {
                dec_ref_bits(_py, digits_bits);
                return MoltObject::none().bits();
            }
            (digits_bits, MoltObject::from_ptr(exp_ptr).bits())
        }
        DecimalSpecial::Finite => {
            let digits_str = if dec.coeff.is_zero() {
                "0".to_string()
            } else {
                dec.coeff.to_string()
            };
            let mut parts: Vec<u64> = Vec::with_capacity(digits_str.len());
            for ch in digits_str.chars() {
                let digit = i64::from(ch as u8 - b'0');
                parts.push(int_bits_from_i64(_py, digit));
            }
            let digits_ptr = alloc_tuple(_py, &parts);
            if digits_ptr.is_null() {
                return MoltObject::none().bits();
            }
            (
                MoltObject::from_ptr(digits_ptr).bits(),
                int_bits_from_i64(_py, dec.exp),
            )
        }
    };

    let tuple_ptr = alloc_tuple(_py, &[sign_bits, digit_bits, exp_bits]);
    if tuple_ptr.is_null() {
        dec_ref_bits(_py, digit_bits);
        dec_ref_bits(_py, exp_bits);
        return MoltObject::none().bits();
    }
    let tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
    dec_ref_bits(_py, digit_bits);
    dec_ref_bits(_py, exp_bits);
    tuple_bits
}

fn compact_trailing_zeros(dec: &mut DecimalHandle) {
    if dec.special != DecimalSpecial::Finite || dec.coeff.is_zero() {
        if dec.coeff.is_zero() {
            dec.exp = 0;
        }
        return;
    }
    while (&dec.coeff % 10u8).is_zero() {
        dec.coeff /= 10u8;
        dec.exp += 1;
    }
}

fn apply_precision(
    ctx: &DecimalContextHandle,
    dec: &mut DecimalHandle,
    status: &mut u32,
) -> Result<(), u32> {
    if dec.special != DecimalSpecial::Finite {
        return Ok(());
    }
    if ctx.prec <= 0 {
        return Err(MPD_INVALID_CONTEXT);
    }
    if dec.coeff.is_zero() {
        return Ok(());
    }
    let digits = digits_len(&dec.coeff);
    if digits <= ctx.prec {
        return Ok(());
    }

    let drop = digits - ctx.prec;
    let divisor = pow10_i64(drop).ok_or(MPD_INVALID_CONTEXT)?;
    let q = &dec.coeff / &divisor;
    let rem = &dec.coeff % &divisor;

    if !rem.is_zero() {
        *status |= MPD_INEXACT;
    }
    *status |= MPD_ROUNDED;

    let mut rounded = q;
    if round_increment(ctx.rounding, dec.sign, &rounded, &rem, &divisor) {
        rounded += 1u8;
    }

    dec.coeff = rounded;
    dec.exp += drop;

    let rounded_digits = digits_len(&dec.coeff);
    if rounded_digits > ctx.prec {
        dec.coeff /= 10u8;
        dec.exp += 1;
    }
    Ok(())
}

fn compare_finite(a: &DecimalHandle, b: &DecimalHandle) -> Ordering {
    if a.coeff.is_zero() && b.coeff.is_zero() {
        return Ordering::Equal;
    }
    if a.sign != b.sign {
        return if a.sign {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }
    let common_exp = a.exp.min(b.exp);
    let shift_a = a.exp - common_exp;
    let shift_b = b.exp - common_exp;
    let sa = &a.coeff * pow10_u32(u32::try_from(shift_a).unwrap_or(0));
    let sb = &b.coeff * pow10_u32(u32::try_from(shift_b).unwrap_or(0));
    let ord = sa.cmp(&sb);
    if a.sign { ord.reverse() } else { ord }
}

fn cmp_num_to_den_pow10(num: &BigInt, den: &BigInt, exp: i64) -> Ordering {
    if exp >= 0 {
        let scale = pow10_i64(exp).unwrap_or_else(BigInt::zero);
        num.cmp(&(den * scale))
    } else {
        let scale = pow10_i64(-exp).unwrap_or_else(BigInt::zero);
        (num * scale).cmp(den)
    }
}

fn round_ratio_to_precision(
    num: &BigInt,
    den: &BigInt,
    prec: i64,
    rounding: i32,
    sign: bool,
) -> Option<(BigInt, i64, u32)> {
    if num.is_zero() {
        return Some((BigInt::zero(), 0, 0));
    }
    if prec <= 0 {
        return None;
    }

    let mut k = digits_len(num) - digits_len(den);
    while cmp_num_to_den_pow10(num, den, k) == Ordering::Less {
        k -= 1;
    }
    while cmp_num_to_den_pow10(num, den, k + 1) != Ordering::Less {
        k += 1;
    }

    let shift = prec - 1 - k;
    let (scaled_num, scaled_den) = if shift >= 0 {
        let scale = pow10_i64(shift)?;
        (num * scale, den.clone())
    } else {
        let scale = pow10_i64(-shift)?;
        (num.clone(), den * scale)
    };

    let mut q = &scaled_num / &scaled_den;
    let rem = &scaled_num % &scaled_den;

    let mut status: u32 = 0;
    if !rem.is_zero() {
        status |= MPD_INEXACT | MPD_ROUNDED;
    }

    if round_increment(rounding, sign, &q, &rem, &scaled_den) {
        q += 1u8;
    }

    let mut exp = k - (prec - 1);
    if digits_len(&q) > prec {
        q /= 10u8;
        exp += 1;
    }

    while (&q % 10u8).is_zero() && !q.is_zero() {
        q /= 10u8;
        exp += 1;
    }

    Some((q, exp, status))
}

fn decimal_from_cmp(value: i64) -> DecimalHandle {
    DecimalHandle {
        sign: value < 0,
        coeff: BigInt::from(value.unsigned_abs()),
        exp: 0,
        special: DecimalSpecial::Finite,
    }
}

fn decimal_bits(dec: DecimalHandle) -> u64 {
    bits_from_ptr(Box::into_raw(Box::new(dec)) as *mut u8)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_context_new() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let handle = Box::new(default_context());
        bits_from_ptr(Box::into_raw(handle) as *mut u8)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_context_get_current() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let ptr = ensure_current_context();
        context_inc(ptr);
        bits_from_ptr(ptr as *mut u8)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_context_set_current(ctx_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(new_ptr) = context_ptr_from_bits(ctx_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal context");
        };
        context_inc(new_ptr);
        let old_ptr = DECIMAL_CONTEXT.with(|slot| {
            let mut slot = slot.borrow_mut();
            let old = *slot;
            *slot = new_ptr;
            old
        });
        if !old_ptr.is_null() {
            context_inc(old_ptr);
            context_dec(old_ptr);
            return bits_from_ptr(old_ptr as *mut u8);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_context_copy(ctx_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        // SAFETY: pointer validated above.
        let mut cloned = unsafe { (*ctx_ptr).clone() };
        cloned.refs = 1;
        bits_from_ptr(Box::into_raw(Box::new(cloned)) as *mut u8)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_context_drop(ctx_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let ptr = ptr_from_bits(ctx_bits) as *mut DecimalContextHandle;
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        context_dec(ptr);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_context_get_prec(ctx_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        // SAFETY: pointer validated above.
        int_bits_from_i64(_py, unsafe { (*ctx_ptr).prec })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_context_set_prec(ctx_bits: u64, prec_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let Some(prec) = obj_from_bits(prec_bits).as_int() else {
            return raise_exception::<u64>(_py, "TypeError", "prec must be int");
        };
        if prec <= 0 {
            return raise_exception::<u64>(_py, "ValueError", "invalid decimal precision");
        }
        // SAFETY: pointer validated above.
        unsafe {
            (*ctx_ptr).prec = prec;
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_context_get_rounding(ctx_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        // SAFETY: pointer validated above.
        int_bits_from_i64(_py, i64::from(unsafe { (*ctx_ptr).rounding }))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_context_set_rounding(ctx_bits: u64, round_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let Some(rounding) = obj_from_bits(round_bits).as_int() else {
            return raise_exception::<u64>(_py, "TypeError", "rounding must be int");
        };
        if !(0..=7).contains(&rounding) {
            return raise_exception::<u64>(_py, "ValueError", "invalid rounding mode");
        }
        // SAFETY: pointer validated above.
        unsafe {
            (*ctx_ptr).rounding = i32::try_from(rounding).unwrap_or(MPD_ROUND_HALF_EVEN);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_context_clear_flags(ctx_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        // SAFETY: pointer validated above.
        unsafe {
            (*ctx_ptr).status = 0;
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_context_get_flag(ctx_bits: u64, flag_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let Some(flag) = obj_from_bits(flag_bits).as_int() else {
            return raise_exception::<u64>(_py, "TypeError", "flag must be int");
        };
        // SAFETY: pointer validated above.
        let status = unsafe { (*ctx_ptr).status };
        MoltObject::from_bool((status & flag as u32) != 0).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_context_set_flag(
    ctx_bits: u64,
    flag_bits: u64,
    value_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let Some(flag) = obj_from_bits(flag_bits).as_int() else {
            return raise_exception::<u64>(_py, "TypeError", "flag must be int");
        };
        let set = is_truthy(_py, obj_from_bits(value_bits));
        // SAFETY: pointer validated above.
        unsafe {
            if set {
                (*ctx_ptr).status |= flag as u32;
            } else {
                (*ctx_ptr).status &= !(flag as u32);
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_context_get_trap(ctx_bits: u64, flag_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let Some(flag) = obj_from_bits(flag_bits).as_int() else {
            return raise_exception::<u64>(_py, "TypeError", "flag must be int");
        };
        // SAFETY: pointer validated above.
        let traps = unsafe { (*ctx_ptr).traps };
        MoltObject::from_bool((traps & flag as u32) != 0).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_context_set_trap(
    ctx_bits: u64,
    flag_bits: u64,
    value_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let Some(flag) = obj_from_bits(flag_bits).as_int() else {
            return raise_exception::<u64>(_py, "TypeError", "flag must be int");
        };
        let set = is_truthy(_py, obj_from_bits(value_bits));
        // SAFETY: pointer validated above.
        unsafe {
            if set {
                (*ctx_ptr).traps |= flag as u32;
            } else {
                (*ctx_ptr).traps &= !(flag as u32);
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_from_str(ctx_bits: u64, value_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let Some(text) = string_obj_to_owned(obj_from_bits(value_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "decimal value must be str");
        };
        let dec = match parse_decimal_text(text.trim()) {
            Ok(d) => d,
            Err(flag) => {
                // SAFETY: pointer validated above.
                let ctx = unsafe { &mut *ctx_ptr };
                if let Err(bits) = apply_status(_py, ctx, flag) {
                    return bits;
                }
                return raise_exception::<u64>(_py, decimal_signal_name(flag), "decimal signal");
            }
        };
        decimal_bits(dec)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_from_int(ctx_bits: u64, value_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let _ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let obj = obj_from_bits(value_bits);
        let Some(big) = to_bigint(obj) else {
            return raise_exception::<u64>(_py, "TypeError", "decimal value must be int");
        };
        decimal_bits(DecimalHandle {
            sign: big.is_negative(),
            coeff: big.abs(),
            exp: 0,
            special: DecimalSpecial::Finite,
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_clone(value_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(handle) = decimal_handle_from_bits(value_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        decimal_bits(handle.clone())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_drop(value_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let ptr = ptr_from_bits(value_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        unsafe { release_ptr(ptr) };
        // SAFETY: pointer is owned by this runtime.
        unsafe {
            drop(Box::from_raw(ptr as *mut DecimalHandle));
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_to_string(value_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(handle) = decimal_handle_from_bits(value_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let capitals = {
            let ctx_ptr = ensure_current_context();
            // SAFETY: pointer is always valid for current thread context.
            unsafe { (*ctx_ptr).capitals }
        };
        let text = decimal_to_string(handle, capitals);
        let ptr = alloc_string(_py, text.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_as_tuple(value_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(handle) = decimal_handle_from_bits(value_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        decimal_tuple_bits(_py, handle)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_to_float(value_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(handle) = decimal_handle_from_bits(value_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let parsed = match handle.special {
            DecimalSpecial::Infinity => {
                if handle.sign {
                    f64::NEG_INFINITY
                } else {
                    f64::INFINITY
                }
            }
            DecimalSpecial::Nan | DecimalSpecial::SNan => f64::NAN,
            DecimalSpecial::Finite => decimal_to_string(handle, 1)
                .parse::<f64>()
                .unwrap_or(f64::NAN),
        };
        MoltObject::from_float(parsed).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_div(ctx_bits: u64, a_bits: u64, b_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let Some(b) = decimal_handle_from_bits(b_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        // SAFETY: pointer validated above.
        let ctx = unsafe { &mut *ctx_ptr };

        if a.special != DecimalSpecial::Finite || b.special != DecimalSpecial::Finite {
            return raise_exception::<u64>(_py, "InvalidOperation", "decimal signal");
        }

        if b.coeff.is_zero() {
            if let Err(bits) = apply_status(_py, ctx, MPD_DIVISION_BY_ZERO) {
                return bits;
            }
            let result = DecimalHandle {
                sign: a.sign ^ b.sign,
                coeff: BigInt::zero(),
                exp: 0,
                special: DecimalSpecial::Infinity,
            };
            return decimal_bits(result);
        }

        if a.coeff.is_zero() {
            return decimal_bits(DecimalHandle {
                sign: a.sign ^ b.sign,
                coeff: BigInt::zero(),
                exp: 0,
                special: DecimalSpecial::Finite,
            });
        }

        let shift = a.exp - b.exp;
        let (num_base, den_base) = if shift >= 0 {
            let Some(scale) = pow10_i64(shift) else {
                return raise_exception::<u64>(_py, "InvalidContext", "decimal signal");
            };
            (&a.coeff * scale, b.coeff.clone())
        } else {
            let Some(scale) = pow10_i64(-shift) else {
                return raise_exception::<u64>(_py, "InvalidContext", "decimal signal");
            };
            (a.coeff.clone(), &b.coeff * scale)
        };

        let Some((coeff, exp, status)) = round_ratio_to_precision(
            &num_base,
            &den_base,
            ctx.prec,
            ctx.rounding,
            a.sign ^ b.sign,
        ) else {
            return raise_exception::<u64>(_py, "InvalidContext", "decimal signal");
        };

        if let Err(bits) = apply_status(_py, ctx, status) {
            return bits;
        }

        decimal_bits(DecimalHandle {
            sign: (a.sign ^ b.sign) && !coeff.is_zero(),
            coeff,
            exp,
            special: DecimalSpecial::Finite,
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_quantize(ctx_bits: u64, a_bits: u64, exp_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let Some(exp_dec) = decimal_handle_from_bits(exp_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        // SAFETY: pointer validated above.
        let ctx = unsafe { &mut *ctx_ptr };

        if a.special != DecimalSpecial::Finite || exp_dec.special != DecimalSpecial::Finite {
            return raise_exception::<u64>(_py, "InvalidOperation", "decimal signal");
        }

        let target_exp = exp_dec.exp;
        let delta = a.exp - target_exp;
        let mut status = 0u32;
        let mut coeff = a.coeff.clone();

        if delta >= 0 {
            let Some(scale) = pow10_i64(delta) else {
                return raise_exception::<u64>(_py, "InvalidContext", "decimal signal");
            };
            coeff *= scale;
        } else {
            let cut = -delta;
            let Some(divisor) = pow10_i64(cut) else {
                return raise_exception::<u64>(_py, "InvalidContext", "decimal signal");
            };
            let q = &coeff / &divisor;
            let rem = &coeff % &divisor;
            if !rem.is_zero() {
                status |= MPD_INEXACT | MPD_ROUNDED;
            }
            let mut rounded = q;
            if round_increment(ctx.rounding, a.sign, &rounded, &rem, &divisor) {
                rounded += 1u8;
            }
            coeff = rounded;
        }

        if let Err(bits) = apply_status(_py, ctx, status) {
            return bits;
        }

        decimal_bits(DecimalHandle {
            sign: a.sign && !coeff.is_zero(),
            coeff,
            exp: target_exp,
            special: DecimalSpecial::Finite,
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_compare(ctx_bits: u64, a_bits: u64, b_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let _ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let Some(b) = decimal_handle_from_bits(b_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };

        if a.special != DecimalSpecial::Finite || b.special != DecimalSpecial::Finite {
            return raise_exception::<u64>(_py, "InvalidOperation", "decimal signal");
        }

        let cmp = compare_finite(a, b);
        let v = match cmp {
            Ordering::Less => -1,
            Ordering::Equal => 0,
            Ordering::Greater => 1,
        };
        decimal_bits(decimal_from_cmp(v))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_compare_total(a_bits: u64, b_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let Some(b) = decimal_handle_from_bits(b_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };

        let cmp = if a.special == DecimalSpecial::Finite && b.special == DecimalSpecial::Finite {
            if a.sign != b.sign {
                if a.sign {
                    Ordering::Less
                } else {
                    Ordering::Greater
                }
            } else {
                let numeric = compare_finite(a, b);
                if numeric != Ordering::Equal {
                    numeric
                } else {
                    let repr_cmp = a.exp.cmp(&b.exp).then_with(|| a.coeff.cmp(&b.coeff));
                    if a.sign { repr_cmp.reverse() } else { repr_cmp }
                }
            }
        } else {
            let rank = |d: &DecimalHandle| match d.special {
                DecimalSpecial::SNan => 0i32,
                DecimalSpecial::Nan => 1,
                DecimalSpecial::Finite => 2,
                DecimalSpecial::Infinity => 3,
            };
            rank(a).cmp(&rank(b))
        };

        let v = match cmp {
            Ordering::Less => -1,
            Ordering::Equal => 0,
            Ordering::Greater => 1,
        };
        decimal_bits(decimal_from_cmp(v))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_normalize(ctx_bits: u64, a_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let _ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let mut out = a.clone();
        compact_trailing_zeros(&mut out);
        decimal_bits(out)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_exp(ctx_bits: u64, a_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        // SAFETY: pointer validated above.
        let ctx = unsafe { &mut *ctx_ptr };

        if a.special != DecimalSpecial::Finite {
            return raise_exception::<u64>(_py, "InvalidOperation", "decimal signal");
        }

        let text = decimal_to_string(a, 1);
        let base = text.parse::<f64>().unwrap_or(f64::NAN);
        let val = libm::exp(base);
        if !val.is_finite() {
            if let Err(bits) = apply_status(_py, ctx, MPD_OVERFLOW) {
                return bits;
            }
            return decimal_bits(DecimalHandle {
                sign: false,
                coeff: BigInt::zero(),
                exp: 0,
                special: DecimalSpecial::Infinity,
            });
        }

        let prec = usize::try_from(ctx.prec.max(1)).unwrap_or(28);
        let sci = format!("{:.*e}", prec + 4, val);
        let mut dec = match parse_decimal_text(&sci) {
            Ok(v) => v,
            Err(flag) => {
                if let Err(bits) = apply_status(_py, ctx, flag) {
                    return bits;
                }
                return raise_exception::<u64>(_py, decimal_signal_name(flag), "decimal signal");
            }
        };
        let mut status = MPD_INEXACT | MPD_ROUNDED;
        if let Err(flag) = apply_precision(ctx, &mut dec, &mut status) {
            if let Err(bits) = apply_status(_py, ctx, flag) {
                return bits;
            }
            return raise_exception::<u64>(_py, decimal_signal_name(flag), "decimal signal");
        }
        if let Err(bits) = apply_status(_py, ctx, status) {
            return bits;
        }
        decimal_bits(dec)
    })
}

// ── Binary arithmetic ────────────────────────────────────────────────────

fn binary_arith_setup(
    _py: &PyToken,
    ctx_bits: u64,
    a_bits: u64,
    b_bits: u64,
) -> Result<
    (
        *mut DecimalContextHandle,
        &'static mut DecimalHandle,
        &'static mut DecimalHandle,
    ),
    u64,
> {
    let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
        Some(ptr) => ptr,
        None => ensure_current_context(),
    };
    let Some(a) = decimal_handle_from_bits(a_bits) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "invalid decimal handle",
        ));
    };
    let Some(b) = decimal_handle_from_bits(b_bits) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "invalid decimal handle",
        ));
    };
    Ok((ctx_ptr, a, b))
}

fn align_add_sub(a: &DecimalHandle, b: &DecimalHandle) -> (BigInt, BigInt, i64) {
    let common_exp = a.exp.min(b.exp);
    let shift_a = u32::try_from(a.exp - common_exp).unwrap_or(0);
    let shift_b = u32::try_from(b.exp - common_exp).unwrap_or(0);
    let ca = &a.coeff * pow10_u32(shift_a);
    let cb = &b.coeff * pow10_u32(shift_b);
    (ca, cb, common_exp)
}

fn finalize_binary(
    _py: &PyToken,
    ctx: &mut DecimalContextHandle,
    sign: bool,
    coeff: BigInt,
    exp: i64,
) -> u64 {
    let mut dec = DecimalHandle {
        sign: sign && !coeff.is_zero(),
        coeff,
        exp,
        special: DecimalSpecial::Finite,
    };
    let mut status = 0u32;
    if let Err(flag) = apply_precision(ctx, &mut dec, &mut status) {
        if let Err(bits) = apply_status(_py, ctx, flag) {
            return bits;
        }
        return raise_exception::<u64>(_py, decimal_signal_name(flag), "decimal signal");
    }
    if let Err(bits) = apply_status(_py, ctx, status) {
        return bits;
    }
    decimal_bits(dec)
}

fn transcendental_via_f64(
    _py: &PyToken,
    ctx: &mut DecimalContextHandle,
    a: &DecimalHandle,
    f: impl FnOnce(f64) -> f64,
) -> u64 {
    if a.special != DecimalSpecial::Finite {
        return raise_exception::<u64>(_py, "InvalidOperation", "decimal signal");
    }
    let text = decimal_to_string(a, 1);
    let base = text.parse::<f64>().unwrap_or(f64::NAN);
    let val = f(base);
    if !val.is_finite() {
        if let Err(bits) = apply_status(_py, ctx, MPD_OVERFLOW) {
            return bits;
        }
        return decimal_bits(DecimalHandle {
            sign: val.is_sign_negative(),
            coeff: BigInt::zero(),
            exp: 0,
            special: DecimalSpecial::Infinity,
        });
    }
    let prec = usize::try_from(ctx.prec.max(1)).unwrap_or(28);
    let sci = format!("{:.*e}", prec + 4, val);
    let mut dec = match parse_decimal_text(&sci) {
        Ok(v) => v,
        Err(flag) => {
            if let Err(bits) = apply_status(_py, ctx, flag) {
                return bits;
            }
            return raise_exception::<u64>(_py, decimal_signal_name(flag), "decimal signal");
        }
    };
    let mut status = MPD_INEXACT | MPD_ROUNDED;
    if let Err(flag) = apply_precision(ctx, &mut dec, &mut status) {
        if let Err(bits) = apply_status(_py, ctx, flag) {
            return bits;
        }
        return raise_exception::<u64>(_py, decimal_signal_name(flag), "decimal signal");
    }
    if let Err(bits) = apply_status(_py, ctx, status) {
        return bits;
    }
    decimal_bits(dec)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_add(ctx_bits: u64, a_bits: u64, b_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let (ctx_ptr, a, b) = match binary_arith_setup(_py, ctx_bits, a_bits, b_bits) {
            Ok(t) => t,
            Err(bits) => return bits,
        };
        let ctx = unsafe { &mut *ctx_ptr };

        if a.special != DecimalSpecial::Finite || b.special != DecimalSpecial::Finite {
            if a.special == DecimalSpecial::Infinity && b.special == DecimalSpecial::Infinity {
                if a.sign != b.sign {
                    return raise_exception::<u64>(_py, "InvalidOperation", "decimal signal");
                }
                return decimal_bits(DecimalHandle {
                    sign: a.sign,
                    coeff: BigInt::zero(),
                    exp: 0,
                    special: DecimalSpecial::Infinity,
                });
            }
            if a.special == DecimalSpecial::Infinity {
                return decimal_bits(a.clone());
            }
            if b.special == DecimalSpecial::Infinity {
                return decimal_bits(b.clone());
            }
            return raise_exception::<u64>(_py, "InvalidOperation", "decimal signal");
        }

        let (ca, cb, common_exp) = align_add_sub(a, b);
        let sa = if a.sign { -ca } else { ca };
        let sb = if b.sign { -cb } else { cb };
        let sum = sa + sb;
        let sign = sum.is_negative();
        let coeff = sum.abs();
        finalize_binary(_py, ctx, sign, coeff, common_exp)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_sub(ctx_bits: u64, a_bits: u64, b_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let (ctx_ptr, a, b) = match binary_arith_setup(_py, ctx_bits, a_bits, b_bits) {
            Ok(t) => t,
            Err(bits) => return bits,
        };
        let ctx = unsafe { &mut *ctx_ptr };

        if a.special != DecimalSpecial::Finite || b.special != DecimalSpecial::Finite {
            if a.special == DecimalSpecial::Infinity && b.special == DecimalSpecial::Infinity {
                if a.sign == b.sign {
                    return raise_exception::<u64>(_py, "InvalidOperation", "decimal signal");
                }
                return decimal_bits(DecimalHandle {
                    sign: a.sign,
                    coeff: BigInt::zero(),
                    exp: 0,
                    special: DecimalSpecial::Infinity,
                });
            }
            if a.special == DecimalSpecial::Infinity {
                return decimal_bits(a.clone());
            }
            if b.special == DecimalSpecial::Infinity {
                return decimal_bits(DecimalHandle {
                    sign: !b.sign,
                    coeff: BigInt::zero(),
                    exp: 0,
                    special: DecimalSpecial::Infinity,
                });
            }
            return raise_exception::<u64>(_py, "InvalidOperation", "decimal signal");
        }

        let (ca, cb, common_exp) = align_add_sub(a, b);
        let sa = if a.sign { -ca } else { ca };
        let sb = if b.sign { -cb } else { cb };
        let diff = sa - sb;
        let sign = diff.is_negative();
        let coeff = diff.abs();
        finalize_binary(_py, ctx, sign, coeff, common_exp)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_mul(ctx_bits: u64, a_bits: u64, b_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let (ctx_ptr, a, b) = match binary_arith_setup(_py, ctx_bits, a_bits, b_bits) {
            Ok(t) => t,
            Err(bits) => return bits,
        };
        let ctx = unsafe { &mut *ctx_ptr };

        if a.special != DecimalSpecial::Finite || b.special != DecimalSpecial::Finite {
            if a.special == DecimalSpecial::Infinity || b.special == DecimalSpecial::Infinity {
                if (a.special == DecimalSpecial::Finite && a.coeff.is_zero())
                    || (b.special == DecimalSpecial::Finite && b.coeff.is_zero())
                {
                    return raise_exception::<u64>(_py, "InvalidOperation", "decimal signal");
                }
                return decimal_bits(DecimalHandle {
                    sign: a.sign ^ b.sign,
                    coeff: BigInt::zero(),
                    exp: 0,
                    special: DecimalSpecial::Infinity,
                });
            }
            return raise_exception::<u64>(_py, "InvalidOperation", "decimal signal");
        }

        let coeff = &a.coeff * &b.coeff;
        let exp = a.exp + b.exp;
        let sign = a.sign ^ b.sign;
        finalize_binary(_py, ctx, sign, coeff, exp)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_floordiv(ctx_bits: u64, a_bits: u64, b_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let (ctx_ptr, a, b) = match binary_arith_setup(_py, ctx_bits, a_bits, b_bits) {
            Ok(t) => t,
            Err(bits) => return bits,
        };
        let ctx = unsafe { &mut *ctx_ptr };

        if a.special != DecimalSpecial::Finite || b.special != DecimalSpecial::Finite {
            return raise_exception::<u64>(_py, "InvalidOperation", "decimal signal");
        }
        if b.coeff.is_zero() {
            if a.coeff.is_zero() {
                return raise_exception::<u64>(_py, "InvalidOperation", "decimal signal");
            }
            if let Err(bits) = apply_status(_py, ctx, MPD_DIVISION_BY_ZERO) {
                return bits;
            }
            return decimal_bits(DecimalHandle {
                sign: a.sign ^ b.sign,
                coeff: BigInt::zero(),
                exp: 0,
                special: DecimalSpecial::Infinity,
            });
        }

        let (ca, cb, _common_exp) = align_add_sub(a, b);
        let q = &ca / &cb;
        let sign = (a.sign ^ b.sign) && !q.is_zero();
        decimal_bits(DecimalHandle {
            sign,
            coeff: q,
            exp: 0,
            special: DecimalSpecial::Finite,
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_mod(ctx_bits: u64, a_bits: u64, b_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let (ctx_ptr, a, b) = match binary_arith_setup(_py, ctx_bits, a_bits, b_bits) {
            Ok(t) => t,
            Err(bits) => return bits,
        };
        let ctx = unsafe { &mut *ctx_ptr };

        if a.special != DecimalSpecial::Finite || b.special != DecimalSpecial::Finite {
            return raise_exception::<u64>(_py, "InvalidOperation", "decimal signal");
        }
        if b.coeff.is_zero() {
            if a.coeff.is_zero() {
                return raise_exception::<u64>(_py, "InvalidOperation", "decimal signal");
            }
            if let Err(bits) = apply_status(_py, ctx, MPD_DIVISION_BY_ZERO) {
                return bits;
            }
            return raise_exception::<u64>(_py, "InvalidOperation", "decimal signal");
        }

        let (ca, cb, common_exp) = align_add_sub(a, b);
        let rem = &ca % &cb;
        let sign = a.sign && !rem.is_zero();
        finalize_binary(_py, ctx, sign, rem, common_exp)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_pow(ctx_bits: u64, a_bits: u64, b_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let (ctx_ptr, a, b) = match binary_arith_setup(_py, ctx_bits, a_bits, b_bits) {
            Ok(t) => t,
            Err(bits) => return bits,
        };
        let ctx = unsafe { &mut *ctx_ptr };

        if a.special != DecimalSpecial::Finite || b.special != DecimalSpecial::Finite {
            return raise_exception::<u64>(_py, "InvalidOperation", "decimal signal");
        }

        // Integer exponent fast path
        if b.exp >= 0 {
            let Some(scale) = pow10_i64(b.exp) else {
                return raise_exception::<u64>(_py, "InvalidContext", "decimal signal");
            };
            let full_exp = &b.coeff * scale;
            if let Some(exp_i64) = full_exp.to_i64().filter(|e| (0..=999_999_999).contains(e)) {
                let exp_u32 = exp_i64 as u32;
                let coeff = num_traits::pow::Pow::pow(&a.coeff, &exp_u32);
                let new_exp = a.exp * exp_i64;
                let sign = a.sign && (exp_u32 % 2 == 1);
                return finalize_binary(_py, ctx, sign, coeff, new_exp);
            }
        }

        // Fall back to f64 for non-integer or large exponents
        transcendental_via_f64(_py, ctx, a, |base| {
            let exp_text = decimal_to_string(b, 1);
            let exp_f64 = exp_text.parse::<f64>().unwrap_or(f64::NAN);
            libm::pow(base, exp_f64)
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_abs(ctx_bits: u64, a_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let _ctx = unsafe { &mut *ctx_ptr };
        decimal_bits(DecimalHandle {
            sign: false,
            coeff: a.coeff.clone(),
            exp: a.exp,
            special: a.special,
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_neg(ctx_bits: u64, a_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let _ctx = unsafe { &mut *ctx_ptr };
        decimal_bits(DecimalHandle {
            sign: !a.sign,
            coeff: a.coeff.clone(),
            exp: a.exp,
            special: a.special,
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_pos(ctx_bits: u64, a_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let ctx = unsafe { &mut *ctx_ptr };
        if a.special != DecimalSpecial::Finite {
            return decimal_bits(a.clone());
        }
        let mut out = a.clone();
        let mut status = 0u32;
        if let Err(flag) = apply_precision(ctx, &mut out, &mut status)
            && let Err(bits) = apply_status(_py, ctx, flag)
        {
            return bits;
        }
        let _ = apply_status(_py, ctx, status);
        decimal_bits(out)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_sqrt(ctx_bits: u64, a_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let ctx = unsafe { &mut *ctx_ptr };
        if a.special == DecimalSpecial::Infinity {
            if a.sign {
                return raise_exception::<u64>(_py, "InvalidOperation", "decimal signal");
            }
            return decimal_bits(a.clone());
        }
        if a.sign && !a.coeff.is_zero() {
            return raise_exception::<u64>(_py, "InvalidOperation", "decimal signal");
        }
        transcendental_via_f64(_py, ctx, a, libm::sqrt)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_ln(ctx_bits: u64, a_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let ctx = unsafe { &mut *ctx_ptr };
        if a.sign && !a.coeff.is_zero() {
            return raise_exception::<u64>(_py, "InvalidOperation", "decimal signal");
        }
        if a.coeff.is_zero() {
            if let Err(bits) = apply_status(_py, ctx, MPD_DIVISION_BY_ZERO) {
                return bits;
            }
            return decimal_bits(DecimalHandle {
                sign: true,
                coeff: BigInt::zero(),
                exp: 0,
                special: DecimalSpecial::Infinity,
            });
        }
        transcendental_via_f64(_py, ctx, a, libm::log)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_log10(ctx_bits: u64, a_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let ctx = unsafe { &mut *ctx_ptr };
        if a.sign && !a.coeff.is_zero() {
            return raise_exception::<u64>(_py, "InvalidOperation", "decimal signal");
        }
        if a.coeff.is_zero() {
            if let Err(bits) = apply_status(_py, ctx, MPD_DIVISION_BY_ZERO) {
                return bits;
            }
            return decimal_bits(DecimalHandle {
                sign: true,
                coeff: BigInt::zero(),
                exp: 0,
                special: DecimalSpecial::Infinity,
            });
        }
        transcendental_via_f64(_py, ctx, a, libm::log10)
    })
}

// ── Predicates ────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_is_finite(a_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return MoltObject::from_bool(false).bits();
        };
        MoltObject::from_bool(a.special == DecimalSpecial::Finite).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_is_infinite(a_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return MoltObject::from_bool(false).bits();
        };
        MoltObject::from_bool(a.special == DecimalSpecial::Infinity).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_is_nan(a_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return MoltObject::from_bool(false).bits();
        };
        MoltObject::from_bool(a.special == DecimalSpecial::Nan || a.special == DecimalSpecial::SNan)
            .bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_is_zero(a_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return MoltObject::from_bool(false).bits();
        };
        MoltObject::from_bool(a.special == DecimalSpecial::Finite && a.coeff.is_zero()).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_is_signed(a_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return MoltObject::from_bool(false).bits();
        };
        MoltObject::from_bool(a.sign).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_is_normal(ctx_bits: u64, a_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return MoltObject::from_bool(false).bits();
        };
        if a.special != DecimalSpecial::Finite || a.coeff.is_zero() {
            return MoltObject::from_bool(false).bits();
        }
        let ctx = unsafe { &*ctx_ptr };
        let adjusted = a.exp + digits_len(&a.coeff) - 1;
        let emin = 1 - ctx.prec;
        MoltObject::from_bool(adjusted >= emin).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_is_subnormal(ctx_bits: u64, a_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return MoltObject::from_bool(false).bits();
        };
        if a.special != DecimalSpecial::Finite || a.coeff.is_zero() {
            return MoltObject::from_bool(false).bits();
        }
        let ctx = unsafe { &*ctx_ptr };
        let adjusted = a.exp + digits_len(&a.coeff) - 1;
        let emin = 1 - ctx.prec;
        MoltObject::from_bool(adjusted < emin).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_number_class(ctx_bits: u64, a_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let class = match a.special {
            DecimalSpecial::Nan => "NaN",
            DecimalSpecial::SNan => "sNaN",
            DecimalSpecial::Infinity => {
                if a.sign {
                    "-Infinity"
                } else {
                    "+Infinity"
                }
            }
            DecimalSpecial::Finite => {
                if a.coeff.is_zero() {
                    if a.sign { "-Zero" } else { "+Zero" }
                } else {
                    let ctx = unsafe { &*ctx_ptr };
                    let adjusted = a.exp + digits_len(&a.coeff) - 1;
                    let emin = 1 - ctx.prec;
                    if adjusted < emin {
                        if a.sign { "-Subnormal" } else { "+Subnormal" }
                    } else if a.sign {
                        "-Normal"
                    } else {
                        "+Normal"
                    }
                }
            }
        };
        let ptr = alloc_string(_py, class.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_adjusted(a_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        if a.special != DecimalSpecial::Finite {
            return raise_exception::<u64>(_py, "InvalidOperation", "decimal signal");
        }
        let adjusted = a.exp + digits_len(&a.coeff) - 1;
        int_bits_from_i64(_py, adjusted)
    })
}

// ── Min/Max/SameQuantum ──────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_max(ctx_bits: u64, a_bits: u64, b_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let (ctx_ptr, a, b) = match binary_arith_setup(_py, ctx_bits, a_bits, b_bits) {
            Ok(t) => t,
            Err(bits) => return bits,
        };
        let _ctx = unsafe { &mut *ctx_ptr };
        if a.special == DecimalSpecial::Nan {
            return decimal_bits(b.clone());
        }
        if b.special == DecimalSpecial::Nan {
            return decimal_bits(a.clone());
        }
        if a.special == DecimalSpecial::Finite && b.special == DecimalSpecial::Finite {
            match compare_finite(a, b) {
                Ordering::Less => return decimal_bits(b.clone()),
                _ => return decimal_bits(a.clone()),
            }
        }
        if a.special == DecimalSpecial::Infinity && !a.sign {
            return decimal_bits(a.clone());
        }
        if b.special == DecimalSpecial::Infinity && !b.sign {
            return decimal_bits(b.clone());
        }
        decimal_bits(a.clone())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_min(ctx_bits: u64, a_bits: u64, b_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let (ctx_ptr, a, b) = match binary_arith_setup(_py, ctx_bits, a_bits, b_bits) {
            Ok(t) => t,
            Err(bits) => return bits,
        };
        let _ctx = unsafe { &mut *ctx_ptr };
        if a.special == DecimalSpecial::Nan {
            return decimal_bits(b.clone());
        }
        if b.special == DecimalSpecial::Nan {
            return decimal_bits(a.clone());
        }
        if a.special == DecimalSpecial::Finite && b.special == DecimalSpecial::Finite {
            match compare_finite(a, b) {
                Ordering::Greater => return decimal_bits(b.clone()),
                _ => return decimal_bits(a.clone()),
            }
        }
        if a.special == DecimalSpecial::Infinity && a.sign {
            return decimal_bits(a.clone());
        }
        if b.special == DecimalSpecial::Infinity && b.sign {
            return decimal_bits(b.clone());
        }
        decimal_bits(a.clone())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_same_quantum(a_bits: u64, b_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return MoltObject::from_bool(false).bits();
        };
        let Some(b) = decimal_handle_from_bits(b_bits) else {
            return MoltObject::from_bool(false).bits();
        };
        if a.special != DecimalSpecial::Finite || b.special != DecimalSpecial::Finite {
            return MoltObject::from_bool(a.special == b.special).bits();
        }
        MoltObject::from_bool(a.exp == b.exp).bits()
    })
}

// ── Integral conversion ─────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_to_integral_value(ctx_bits: u64, a_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let ctx = unsafe { &mut *ctx_ptr };
        if a.special != DecimalSpecial::Finite {
            return decimal_bits(a.clone());
        }
        if a.exp >= 0 {
            return decimal_bits(a.clone());
        }
        let cut = -a.exp;
        let Some(divisor) = pow10_i64(cut) else {
            return raise_exception::<u64>(_py, "InvalidContext", "decimal signal");
        };
        let q = &a.coeff / &divisor;
        let rem = &a.coeff % &divisor;
        let mut rounded = q;
        if round_increment(ctx.rounding, a.sign, &rounded, &rem, &divisor) {
            rounded += 1u8;
        }
        let mut status = 0u32;
        if !rem.is_zero() {
            status |= MPD_INEXACT | MPD_ROUNDED;
        }
        if let Err(bits) = apply_status(_py, ctx, status) {
            return bits;
        }
        decimal_bits(DecimalHandle {
            sign: a.sign && !rounded.is_zero(),
            coeff: rounded,
            exp: 0,
            special: DecimalSpecial::Finite,
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_to_integral_exact(ctx_bits: u64, a_bits: u64) -> u64 {
    molt_decimal_to_integral_value(ctx_bits, a_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_to_eng_string(a_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        match a.special {
            DecimalSpecial::Infinity | DecimalSpecial::Nan | DecimalSpecial::SNan => {
                let text = decimal_to_string(a, 1);
                let ptr = alloc_string(_py, text.as_bytes());
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
            DecimalSpecial::Finite => {}
        }
        let digits = if a.coeff.is_zero() {
            "0".to_string()
        } else {
            a.coeff.to_string()
        };
        let n = i64::try_from(digits.len()).unwrap_or(1);
        let adjusted = a.exp + n - 1;
        let eng_exp = adjusted - (((adjusted % 3) + 3) % 3);
        let shift = adjusted - eng_exp;
        let left_digits = usize::try_from(shift + 1).unwrap_or(1);
        let text = if left_digits >= digits.len() {
            let zeros = left_digits - digits.len();
            let padded = format!("{}{}", digits, "0".repeat(zeros));
            if eng_exp == 0 {
                padded
            } else {
                format!("{}E{:+}", padded, eng_exp)
            }
        } else {
            let (left, right) = digits.split_at(left_digits);
            if eng_exp == 0 {
                format!("{}.{}", left, right)
            } else {
                format!("{}.{}E{:+}", left, right, eng_exp)
            }
        };
        let mut result = text;
        if a.sign {
            result.insert(0, '-');
        }
        let ptr = alloc_string(_py, result.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

// ── Unary operations ────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_next_plus(ctx_bits: u64, a_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let ctx = unsafe { &*ctx_ptr };
        if a.special == DecimalSpecial::Infinity && !a.sign {
            return decimal_bits(a.clone());
        }
        if a.special == DecimalSpecial::Infinity && a.sign {
            let emax = ctx.prec - 1;
            let coeff = pow10_i64(ctx.prec).unwrap_or_else(BigInt::zero) - BigInt::one();
            return decimal_bits(DecimalHandle {
                sign: true,
                coeff,
                exp: emax - ctx.prec + 1,
                special: DecimalSpecial::Finite,
            });
        }
        if a.special != DecimalSpecial::Finite {
            return raise_exception::<u64>(_py, "InvalidOperation", "decimal signal");
        }
        let etiny = (1 - ctx.prec) - ctx.prec + 1;
        let epsilon = DecimalHandle {
            sign: false,
            coeff: BigInt::one(),
            exp: etiny,
            special: DecimalSpecial::Finite,
        };
        let (ca, ce, common_exp) = align_add_sub(a, &epsilon);
        let sa = if a.sign { -ca } else { ca };
        let sum = sa + ce;
        let sign = sum.is_negative();
        let coeff = sum.abs();
        decimal_bits(DecimalHandle {
            sign: sign && !coeff.is_zero(),
            coeff,
            exp: common_exp,
            special: DecimalSpecial::Finite,
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_next_minus(ctx_bits: u64, a_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let ctx = unsafe { &*ctx_ptr };
        if a.special == DecimalSpecial::Infinity && a.sign {
            return decimal_bits(a.clone());
        }
        if a.special == DecimalSpecial::Infinity && !a.sign {
            let emax = ctx.prec - 1;
            let coeff = pow10_i64(ctx.prec).unwrap_or_else(BigInt::zero) - BigInt::one();
            return decimal_bits(DecimalHandle {
                sign: false,
                coeff,
                exp: emax - ctx.prec + 1,
                special: DecimalSpecial::Finite,
            });
        }
        if a.special != DecimalSpecial::Finite {
            return raise_exception::<u64>(_py, "InvalidOperation", "decimal signal");
        }
        let etiny = (1 - ctx.prec) - ctx.prec + 1;
        let epsilon = DecimalHandle {
            sign: false,
            coeff: BigInt::one(),
            exp: etiny,
            special: DecimalSpecial::Finite,
        };
        let (ca, ce, common_exp) = align_add_sub(a, &epsilon);
        let sa = if a.sign { -ca } else { ca };
        let diff = sa - ce;
        let sign = diff.is_negative();
        let coeff = diff.abs();
        decimal_bits(DecimalHandle {
            sign: sign && !coeff.is_zero(),
            coeff,
            exp: common_exp,
            special: DecimalSpecial::Finite,
        })
    })
}

// ── Copy operations ─────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_copy_abs(a_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        decimal_bits(DecimalHandle {
            sign: false,
            coeff: a.coeff.clone(),
            exp: a.exp,
            special: a.special,
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_copy_negate(a_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        decimal_bits(DecimalHandle {
            sign: !a.sign,
            coeff: a.coeff.clone(),
            exp: a.exp,
            special: a.special,
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_copy_sign(a_bits: u64, b_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let Some(b) = decimal_handle_from_bits(b_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        decimal_bits(DecimalHandle {
            sign: b.sign,
            coeff: a.coeff.clone(),
            exp: a.exp,
            special: a.special,
        })
    })
}

// ── Conversion ──────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_as_integer_ratio(a_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        if a.special != DecimalSpecial::Finite {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "cannot convert non-finite Decimal to integer ratio",
            );
        }
        let (num, den) = if a.exp >= 0 {
            let scale = pow10_i64(a.exp).unwrap_or_else(BigInt::zero);
            let n = &a.coeff * scale;
            (if a.sign { -n } else { n }, BigInt::one())
        } else {
            let scale = pow10_i64(-a.exp).unwrap_or_else(BigInt::zero);
            let n = if a.sign {
                -a.coeff.clone()
            } else {
                a.coeff.clone()
            };
            (n, scale)
        };
        let num_bits = int_bits_from_bigint(_py, num);
        let den_bits = int_bits_from_bigint(_py, den);
        let tuple_ptr = alloc_tuple(_py, &[num_bits, den_bits]);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_from_float(ctx_bits: u64, value_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let obj = obj_from_bits(value_bits);
        let Some(f) = obj.as_float() else {
            return raise_exception::<u64>(_py, "TypeError", "argument must be float");
        };
        let _ctx = unsafe { &*ctx_ptr };
        if f.is_nan() {
            return decimal_bits(DecimalHandle {
                sign: f.is_sign_negative(),
                coeff: BigInt::zero(),
                exp: 0,
                special: DecimalSpecial::Nan,
            });
        }
        if f.is_infinite() {
            return decimal_bits(DecimalHandle {
                sign: f.is_sign_negative(),
                coeff: BigInt::zero(),
                exp: 0,
                special: DecimalSpecial::Infinity,
            });
        }
        let text = format!("{}", f);
        match parse_decimal_text(&text) {
            Ok(dec) => decimal_bits(dec),
            Err(_) => raise_exception::<u64>(_py, "ValueError", "cannot convert float to Decimal"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_to_int(a_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        if a.special != DecimalSpecial::Finite {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "cannot convert non-finite Decimal to int",
            );
        }
        let value = if a.exp >= 0 {
            let scale = pow10_i64(a.exp).unwrap_or_else(BigInt::zero);
            &a.coeff * scale
        } else {
            let scale = pow10_i64(-a.exp).unwrap_or_else(BigInt::zero);
            &a.coeff / &scale
        };
        let signed = if a.sign { -value } else { value };
        int_bits_from_bigint(_py, signed)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_scaleb(ctx_bits: u64, a_bits: u64, b_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let (ctx_ptr, a, b) = match binary_arith_setup(_py, ctx_bits, a_bits, b_bits) {
            Ok(t) => t,
            Err(bits) => return bits,
        };
        let ctx = unsafe { &mut *ctx_ptr };
        if a.special != DecimalSpecial::Finite || b.special != DecimalSpecial::Finite {
            return raise_exception::<u64>(_py, "InvalidOperation", "decimal signal");
        }
        if b.exp != 0 {
            return raise_exception::<u64>(
                _py,
                "InvalidOperation",
                "second argument must be integer",
            );
        }
        let Some(shift) = b.coeff.to_i64() else {
            return raise_exception::<u64>(_py, "InvalidOperation", "exponent too large");
        };
        let shift = if b.sign { -shift } else { shift };
        let mut dec = DecimalHandle {
            sign: a.sign,
            coeff: a.coeff.clone(),
            exp: a.exp + shift,
            special: DecimalSpecial::Finite,
        };
        let mut status = 0u32;
        if let Err(flag) = apply_precision(ctx, &mut dec, &mut status)
            && let Err(bits) = apply_status(_py, ctx, flag)
        {
            return bits;
        }
        let _ = apply_status(_py, ctx, status);
        decimal_bits(dec)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_remainder_near(ctx_bits: u64, a_bits: u64, b_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let (ctx_ptr, a, b) = match binary_arith_setup(_py, ctx_bits, a_bits, b_bits) {
            Ok(t) => t,
            Err(bits) => return bits,
        };
        let ctx = unsafe { &mut *ctx_ptr };
        if a.special != DecimalSpecial::Finite || b.special != DecimalSpecial::Finite {
            return raise_exception::<u64>(_py, "InvalidOperation", "decimal signal");
        }
        if b.coeff.is_zero() {
            return raise_exception::<u64>(_py, "InvalidOperation", "decimal signal");
        }
        let (ca, cb, common_exp) = align_add_sub(a, b);
        let rem = &ca % &cb;
        let half_divisor_times_2 = &cb;
        let rem_times_2 = &rem * 2u8;
        let final_rem = if rem_times_2 > *half_divisor_times_2 {
            rem - cb
        } else {
            rem
        };
        let sign = a.sign && !final_rem.is_zero();
        let coeff = final_rem.abs();
        finalize_binary(_py, ctx, sign, coeff, common_exp)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_fma(ctx_bits: u64, a_bits: u64, b_bits: u64, c_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let Some(b) = decimal_handle_from_bits(b_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let Some(c) = decimal_handle_from_bits(c_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let ctx = unsafe { &mut *ctx_ptr };
        if a.special != DecimalSpecial::Finite
            || b.special != DecimalSpecial::Finite
            || c.special != DecimalSpecial::Finite
        {
            return raise_exception::<u64>(_py, "InvalidOperation", "decimal signal");
        }
        let product_coeff = &a.coeff * &b.coeff;
        let product_exp = a.exp + b.exp;
        let product_sign = a.sign ^ b.sign;
        let product = DecimalHandle {
            sign: product_sign,
            coeff: product_coeff,
            exp: product_exp,
            special: DecimalSpecial::Finite,
        };
        let (ca, cc, common_exp) = align_add_sub(&product, c);
        let sa = if product.sign { -ca } else { ca };
        let sc = if c.sign { -cc } else { cc };
        let sum = sa + sc;
        let sign = sum.is_negative();
        let coeff = sum.abs();
        finalize_binary(_py, ctx, sign, coeff, common_exp)
    })
}
