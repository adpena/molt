#![allow(dead_code, unused_imports)]
use std::cell::RefCell;
use std::cmp::Ordering;
use std::ptr;

use molt_obj_model::MoltObject;
use num_bigint::BigInt;
use num_traits::{One, Signed, ToPrimitive, Zero};

use crate::object::ops::{is_truthy, string_obj_to_owned};
use crate::{
    PyToken, alloc_string, alloc_tuple, dec_ref_bits, int_bits_from_bigint, int_bits_from_i64,
    obj_from_bits, opaque_handle_bits, opaque_handle_ptr_from_bits, raise_exception, release_ptr,
    to_bigint,
};

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
// CPython decimal.DefaultContext bounds (Lib/_pydecimal.py): Emax=999999, Emin=-999999, clamp=0.
const DECIMAL_DEFAULT_EMIN: i64 = -999_999;
const DECIMAL_DEFAULT_EMAX: i64 = 999_999;
const DECIMAL_DEFAULT_CLAMP: i32 = 0;

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
    /// Smallest adjusted exponent of a normal number; CPython default -999999.
    emin: i64,
    /// Largest adjusted exponent; CPython default 999999.
    emax: i64,
    /// IEEE clamp flag (0 or 1); CPython default 0.
    clamp: i32,
    refs: usize,
}

impl DecimalContextHandle {
    /// Etiny = Emin - prec + 1 (CPython Context.Etiny).
    fn etiny(&self) -> i64 {
        self.emin - self.prec + 1
    }

    /// Etop = Emax - prec + 1 (CPython Context.Etop).
    fn etop(&self) -> i64 {
        self.emax - self.prec + 1
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
        emin: DECIMAL_DEFAULT_EMIN,
        emax: DECIMAL_DEFAULT_EMAX,
        clamp: DECIMAL_DEFAULT_CLAMP,
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
    opaque_handle_ptr_from_bits(bits).map(|ptr| ptr as *mut DecimalContextHandle)
}

fn decimal_handle_from_bits(bits: u64) -> Option<&'static mut DecimalHandle> {
    let ptr = opaque_handle_ptr_from_bits(bits)?;
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

fn apply_status(_py: &PyToken<'_>, ctx: &mut DecimalContextHandle, status: u32) -> Result<(), u64> {
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

fn decimal_tuple_bits(_py: &PyToken<'_>, dec: &DecimalHandle) -> u64 {
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

/// Largest finite value representable in `ctx` (Nmax = (10^prec - 1) * 10^Etop),
/// with the given sign. Used as the round-down overflow result.
fn nmax(ctx: &DecimalContextHandle, sign: bool) -> DecimalHandle {
    let coeff = pow10_i64(ctx.prec).unwrap_or_else(BigInt::zero) - BigInt::one();
    DecimalHandle {
        sign,
        coeff,
        exp: ctx.etop(),
        special: DecimalSpecial::Finite,
    }
}

/// CPython `Overflow.handle`: the result of an overflowing operation depends on
/// the rounding mode — either signed infinity or the largest finite number.
fn overflow_result(ctx: &DecimalContextHandle, sign: bool) -> DecimalHandle {
    match ctx.rounding {
        MPD_ROUND_HALF_UP | MPD_ROUND_HALF_EVEN | MPD_ROUND_HALF_DOWN | MPD_ROUND_UP => {
            DecimalHandle {
                sign,
                coeff: BigInt::zero(),
                exp: 0,
                special: DecimalSpecial::Infinity,
            }
        }
        MPD_ROUND_CEILING if !sign => DecimalHandle {
            sign,
            coeff: BigInt::zero(),
            exp: 0,
            special: DecimalSpecial::Infinity,
        },
        MPD_ROUND_FLOOR if sign => DecimalHandle {
            sign,
            coeff: BigInt::zero(),
            exp: 0,
            special: DecimalSpecial::Infinity,
        },
        _ => nmax(ctx, sign),
    }
}

/// Round, fix the exponent, and apply the context Emin/Emax/clamp bounds,
/// raising the appropriate signals into `status`.
///
/// Faithful port of CPython `Decimal._fix(self, context)` (Lib/_pydecimal.py).
/// Operates in place on `dec`, which must be a finite Decimal coefficient
/// representation (sign, coeff, exp). Specials are returned unchanged.
///
/// The signals collected in `status` (MPD_SUBNORMAL / MPD_UNDERFLOW /
/// MPD_OVERFLOW / MPD_CLAMPED / MPD_INEXACT / MPD_ROUNDED) follow the IEEE
/// 854 precedence the specification requires; `apply_status` is responsible
/// for turning trapped signals into raised exceptions.
fn fix_decimal(
    ctx: &DecimalContextHandle,
    dec: &mut DecimalHandle,
    status: &mut u32,
) -> Result<(), u32> {
    if dec.special != DecimalSpecial::Finite {
        // +/-Infinity and NaN are returned unaltered (sNaN payload handling is
        // performed at the call sites that can observe it).
        return Ok(());
    }
    if ctx.prec <= 0 {
        return Err(MPD_INVALID_CONTEXT);
    }

    let etiny = ctx.etiny();
    let etop = ctx.etop();

    // Zero: exponent must lie between Etiny and Emax (clamp==0) or Etop (clamp==1).
    if dec.coeff.is_zero() {
        let exp_max = if ctx.clamp == 1 { etop } else { ctx.emax };
        let new_exp = dec.exp.max(etiny).min(exp_max);
        if new_exp != dec.exp {
            *status |= MPD_CLAMPED;
            dec.exp = new_exp;
        }
        return Ok(());
    }

    // exp_min = max(self.adjusted() - prec + 1, Etiny)
    //         = (len(coeff) + exp - prec) clamped up to Etiny.
    let coeff_digits = digits_len(&dec.coeff);
    let mut exp_min = coeff_digits + dec.exp - ctx.prec;

    // Overflow: exp_min > Etop  <=>  adjusted() > Emax.
    if exp_min > etop {
        *status |= MPD_OVERFLOW | MPD_INEXACT | MPD_ROUNDED;
        *dec = overflow_result(ctx, dec.sign);
        return Ok(());
    }

    let self_is_subnormal = exp_min < etiny;
    if self_is_subnormal {
        exp_min = etiny;
    }

    // Round if the value has digits below exp_min.
    if dec.exp < exp_min {
        // If every surviving digit is below the rounding point (adjusted <
        // exp_min - 1), CPython replaces self with 1 * 10**(exp_min - 1) before
        // rounding at digit 0, so the coefficient becomes "0" and every original
        // digit is dropped (always inexact in that case).
        let below_round_point = coeff_digits + dec.exp - exp_min < 0;
        let (mut coeff, changed_nonzero, changed_increment) = if below_round_point {
            // self := _dec_from_triple(sign, '1', exp_min - 1); round at prec 0.
            let inc = round_increment(
                ctx.rounding,
                dec.sign,
                &BigInt::zero(),
                &BigInt::one(),
                &BigInt::from(10u8),
            );
            (BigInt::zero(), true, inc)
        } else {
            let drop = -dec.exp + exp_min;
            let divisor = pow10_i64(drop).ok_or(MPD_INVALID_CONTEXT)?;
            let q = &dec.coeff / &divisor;
            let rem = &dec.coeff % &divisor;
            let nonzero = !rem.is_zero();
            let inc = round_increment(ctx.rounding, dec.sign, &q, &rem, &divisor);
            (q, nonzero, inc)
        };

        if changed_increment {
            coeff += 1u8;
            // A carry can push the coefficient past prec; drop the trailing digit.
            if digits_len(&coeff) > ctx.prec {
                coeff /= 10u8;
                exp_min += 1;
            }
        }

        // Did the rounding push the exponent back above Etop?
        let overflowed = exp_min > etop;
        if overflowed {
            *status |= MPD_OVERFLOW;
            *dec = overflow_result(ctx, dec.sign);
        } else {
            dec.coeff = coeff;
            dec.exp = exp_min;
        }

        // Raise the signals in specification precedence order.
        if changed_nonzero && self_is_subnormal {
            *status |= MPD_UNDERFLOW;
        }
        if self_is_subnormal {
            *status |= MPD_SUBNORMAL;
        }
        if changed_nonzero {
            *status |= MPD_INEXACT;
        }
        *status |= MPD_ROUNDED;
        if !overflowed && dec.coeff.is_zero() {
            // Underflow to zero raises Clamped (the result is 0E-Etiny).
            *status |= MPD_CLAMPED;
        }
        return Ok(());
    }

    if self_is_subnormal {
        *status |= MPD_SUBNORMAL;
    }

    // clamp==1 fold-down: too few digits, pad up to Etop.
    if ctx.clamp == 1 && dec.exp > etop {
        *status |= MPD_CLAMPED;
        let pad = u32::try_from(dec.exp - etop).map_err(|_| MPD_INVALID_CONTEXT)?;
        dec.coeff *= pow10_u32(pad);
        dec.exp = etop;
        return Ok(());
    }

    // Representable as-is.
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
    opaque_handle_bits(Box::into_raw(Box::new(dec)) as *mut u8)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_context_new() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = Box::new(default_context());
        opaque_handle_bits(Box::into_raw(handle) as *mut u8)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_context_get_current() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ptr = ensure_current_context();
        context_inc(ptr);
        opaque_handle_bits(ptr as *mut u8)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_context_set_current(ctx_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
            return opaque_handle_bits(old_ptr as *mut u8);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_context_copy(ctx_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        // SAFETY: pointer validated above.
        let mut cloned = unsafe { (*ctx_ptr).clone() };
        cloned.refs = 1;
        opaque_handle_bits(Box::into_raw(Box::new(cloned)) as *mut u8)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_context_drop(ctx_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(ptr) = context_ptr_from_bits(ctx_bits) else {
            return MoltObject::none().bits();
        };
        context_dec(ptr);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_context_get_prec(ctx_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
pub extern "C" fn molt_decimal_context_get_emin(ctx_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        // SAFETY: pointer validated above.
        int_bits_from_i64(_py, unsafe { (*ctx_ptr).emin })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_context_set_emin(ctx_bits: u64, emin_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let Some(emin) = obj_from_bits(emin_bits).as_int() else {
            return raise_exception::<u64>(_py, "TypeError", "Emin must be an integer");
        };
        // CPython: Emin must be in [-inf, 0].
        if emin > 0 {
            return raise_exception::<u64>(_py, "ValueError", "Emin must be in [-inf, 0]");
        }
        // SAFETY: pointer validated above.
        unsafe {
            (*ctx_ptr).emin = emin;
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_context_get_emax(ctx_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        // SAFETY: pointer validated above.
        int_bits_from_i64(_py, unsafe { (*ctx_ptr).emax })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_context_set_emax(ctx_bits: u64, emax_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let Some(emax) = obj_from_bits(emax_bits).as_int() else {
            return raise_exception::<u64>(_py, "TypeError", "Emax must be an integer");
        };
        // CPython: Emax must be in [0, inf].
        if emax < 0 {
            return raise_exception::<u64>(_py, "ValueError", "Emax must be in [0, inf]");
        }
        // SAFETY: pointer validated above.
        unsafe {
            (*ctx_ptr).emax = emax;
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_context_get_clamp(ctx_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        // SAFETY: pointer validated above.
        int_bits_from_i64(_py, i64::from(unsafe { (*ctx_ptr).clamp }))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_context_set_clamp(ctx_bits: u64, clamp_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let Some(clamp) = obj_from_bits(clamp_bits).as_int() else {
            return raise_exception::<u64>(_py, "TypeError", "clamp must be an integer");
        };
        // CPython: clamp must be in [0, 1].
        if !(0..=1).contains(&clamp) {
            return raise_exception::<u64>(_py, "ValueError", "clamp must be in [0, 1]");
        }
        // SAFETY: pointer validated above.
        unsafe {
            (*ctx_ptr).clamp = clamp as i32;
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_context_get_capitals(ctx_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        // SAFETY: pointer validated above.
        int_bits_from_i64(_py, i64::from(unsafe { (*ctx_ptr).capitals }))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_context_set_capitals(ctx_bits: u64, capitals_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let Some(capitals) = obj_from_bits(capitals_bits).as_int() else {
            return raise_exception::<u64>(_py, "TypeError", "capitals must be an integer");
        };
        // CPython: capitals must be in [0, 1].
        if !(0..=1).contains(&capitals) {
            return raise_exception::<u64>(_py, "ValueError", "capitals must be in [0, 1]");
        }
        // SAFETY: pointer validated above.
        unsafe {
            (*ctx_ptr).capitals = capitals as i32;
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_context_etiny(ctx_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        // SAFETY: pointer validated above.
        int_bits_from_i64(_py, unsafe { (*ctx_ptr).etiny() })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_context_etop(ctx_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        // SAFETY: pointer validated above.
        int_bits_from_i64(_py, unsafe { (*ctx_ptr).etop() })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_context_clear_flags(ctx_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
        let Some(handle) = decimal_handle_from_bits(value_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        decimal_bits(handle.clone())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_drop(value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(ptr) = opaque_handle_ptr_from_bits(value_bits) else {
            return MoltObject::none().bits();
        };
        release_ptr(ptr);
        // SAFETY: pointer is owned by this runtime.
        unsafe {
            drop(Box::from_raw(ptr as *mut DecimalHandle));
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_to_string(value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
        let Some(handle) = decimal_handle_from_bits(value_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        decimal_tuple_bits(_py, handle)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_to_float(value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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

/// Rescale `dec` to the target exponent, padding with zeros or rounding with
/// `rounding`. Quiet: raises no flags and consults no context (CPython
/// `Decimal._rescale`). `dec` must be finite. Returns None only on an
/// impossible (overflowing) power of ten.
fn rescale_quiet(dec: &DecimalHandle, target_exp: i64, rounding: i32) -> Option<DecimalHandle> {
    if dec.coeff.is_zero() {
        return Some(DecimalHandle {
            sign: dec.sign,
            coeff: BigInt::zero(),
            exp: target_exp,
            special: DecimalSpecial::Finite,
        });
    }
    if dec.exp >= target_exp {
        let pad = u32::try_from(dec.exp - target_exp).ok()?;
        return Some(DecimalHandle {
            sign: dec.sign,
            coeff: &dec.coeff * pow10_u32(pad),
            exp: target_exp,
            special: DecimalSpecial::Finite,
        });
    }
    // Too many digits: round and lose data. If the value is below the rounding
    // point (adjusted < target_exp - 1), CPython first replaces it with
    // 1 * 10**(target_exp - 1) so the rounding direction is preserved.
    let below_round_point = digits_len(&dec.coeff) + dec.exp - target_exp < 0;
    let (coeff_src, exp_src) = if below_round_point {
        (BigInt::one(), target_exp - 1)
    } else {
        (dec.coeff.clone(), dec.exp)
    };
    // exp_src < target_exp here, so the number of low digits to drop is positive.
    let drop = target_exp - exp_src;
    let divisor = pow10_i64(drop)?;
    let mut q = &coeff_src / &divisor;
    let rem = &coeff_src % &divisor;
    if round_increment(rounding, dec.sign, &q, &rem, &divisor) {
        q += 1u8;
    }
    Some(DecimalHandle {
        sign: dec.sign,
        coeff: q,
        exp: target_exp,
        special: DecimalSpecial::Finite,
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_quantize(ctx_bits: u64, a_bits: u64, exp_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

        // Special handling: both infinite -> self; one infinite -> InvalidOperation.
        let a_inf = a.special == DecimalSpecial::Infinity;
        let exp_inf = exp_dec.special == DecimalSpecial::Infinity;
        if a.special != DecimalSpecial::Finite || exp_dec.special != DecimalSpecial::Finite {
            if a_inf && exp_inf {
                return decimal_bits(a.clone());
            }
            return raise_exception::<u64>(_py, "InvalidOperation", "decimal signal");
        }

        let target_exp = exp_dec.exp;

        // exp._exp must lie within [Etiny, Emax].
        if !(ctx.etiny() <= target_exp && target_exp <= ctx.emax) {
            return raise_exception::<u64>(_py, "InvalidOperation", "decimal signal");
        }

        // Zero short-circuit: result is 0 at the target exponent, then _fix.
        if a.coeff.is_zero() {
            let mut zero = DecimalHandle {
                sign: a.sign,
                coeff: BigInt::zero(),
                exp: target_exp,
                special: DecimalSpecial::Finite,
            };
            let mut status = 0u32;
            if let Err(flag) = fix_decimal(ctx, &mut zero, &mut status) {
                if let Err(bits) = apply_status(_py, ctx, flag) {
                    return bits;
                }
                return raise_exception::<u64>(_py, decimal_signal_name(flag), "decimal signal");
            }
            if let Err(bits) = apply_status(_py, ctx, status) {
                return bits;
            }
            return decimal_bits(zero);
        }

        let self_adjusted = a.exp + digits_len(&a.coeff) - 1;
        if self_adjusted > ctx.emax {
            return raise_exception::<u64>(_py, "InvalidOperation", "decimal signal");
        }
        if self_adjusted - target_exp + 1 > ctx.prec {
            return raise_exception::<u64>(_py, "InvalidOperation", "decimal signal");
        }

        let Some(mut ans) = rescale_quiet(a, target_exp, ctx.rounding) else {
            return raise_exception::<u64>(_py, "InvalidContext", "decimal signal");
        };
        ans.sign = ans.sign && !ans.coeff.is_zero();

        let ans_adjusted = ans.exp + digits_len(&ans.coeff) - 1;
        if !ans.coeff.is_zero() && ans_adjusted > ctx.emax {
            return raise_exception::<u64>(_py, "InvalidOperation", "decimal signal");
        }
        if digits_len(&ans.coeff) > ctx.prec {
            return raise_exception::<u64>(_py, "InvalidOperation", "decimal signal");
        }

        let mut status = 0u32;
        // Subnormal result.
        if !ans.coeff.is_zero() && ans_adjusted < ctx.emin {
            status |= MPD_SUBNORMAL;
        }
        // Inexact/Rounded if the exponent grew (digits were dropped).
        if ans.exp > a.exp {
            if compare_finite(&ans, a) != Ordering::Equal {
                status |= MPD_INEXACT;
            }
            status |= MPD_ROUNDED;
        }

        // _fix handles any folddown and the Clamped signal.
        if let Err(flag) = fix_decimal(ctx, &mut ans, &mut status) {
            if let Err(bits) = apply_status(_py, ctx, flag) {
                return bits;
            }
            return raise_exception::<u64>(_py, decimal_signal_name(flag), "decimal signal");
        }
        if let Err(bits) = apply_status(_py, ctx, status) {
            return bits;
        }
        decimal_bits(ans)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_compare(ctx_bits: u64, a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let ctx = unsafe { &mut *ctx_ptr };
        if a.special != DecimalSpecial::Finite {
            // NaN/Infinity: returned unchanged (sNaN routing handled by the shim).
            return decimal_bits(a.clone());
        }
        // CPython normalize: round to context first, then strip trailing zeros.
        let mut out = a.clone();
        let mut status = 0u32;
        if let Err(flag) = fix_decimal(ctx, &mut out, &mut status) {
            if let Err(bits) = apply_status(_py, ctx, flag) {
                return bits;
            }
            return raise_exception::<u64>(_py, decimal_signal_name(flag), "decimal signal");
        }
        if let Err(bits) = apply_status(_py, ctx, status) {
            return bits;
        }
        if out.special != DecimalSpecial::Finite {
            // _fix overflowed to Infinity.
            return decimal_bits(out);
        }
        if out.coeff.is_zero() {
            // Any zero normalizes to 0E0.
            return decimal_bits(DecimalHandle {
                sign: out.sign,
                coeff: BigInt::zero(),
                exp: 0,
                special: DecimalSpecial::Finite,
            });
        }
        // Strip trailing zeros, but never raise the exponent above exp_max.
        let exp_max = if ctx.clamp == 1 { ctx.etop() } else { ctx.emax };
        while out.exp < exp_max && (&out.coeff % 10u8).is_zero() {
            out.coeff /= 10u8;
            out.exp += 1;
        }
        decimal_bits(out)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_exp(ctx_bits: u64, a_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
        if let Err(flag) = fix_decimal(ctx, &mut dec, &mut status) {
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

/// Align two finite operands for addition/subtraction with CPython's `_normalize`
/// capping (Lib/_pydecimal.py): when the smaller operand is more than ~prec
/// orders of magnitude below the larger, it is replaced by a single sticky digit.
/// This keeps the aligned coefficients bounded (a result rounded to `prec` digits
/// is identical) instead of materializing a 10**(Emax-Emin)-sized integer.
///
/// Returns the aligned `(coeff_a, coeff_b, common_exp)` triple, where the sign of
/// each operand is NOT applied (callers attach signs, as with `align_add_sub`).
fn normalize_add_operands(
    a: &DecimalHandle,
    b: &DecimalHandle,
    prec: i64,
) -> (BigInt, BigInt, i64) {
    // `tmp` is the operand with the larger exponent; `other` the smaller.
    let (tmp, other, tmp_is_a) = if a.exp < b.exp {
        (b, a, false)
    } else {
        (a, b, true)
    };

    let tmp_len = digits_len(&tmp.coeff);
    // exp = tmp.exp + min(-1, tmp_len - prec - 2)
    let cap_exp = tmp.exp + (-1).min(tmp_len - prec - 2);

    // If `other` is entirely below the sticky-digit threshold, collapse it to
    // 1 * 10**cap_exp (its exact value rounds identically at `prec` digits).
    let (other_coeff, other_exp) = {
        let other_len = digits_len(&other.coeff);
        if !other.coeff.is_zero() && other_len + other.exp - 1 < cap_exp {
            (BigInt::one(), cap_exp)
        } else {
            (other.coeff.clone(), other.exp)
        }
    };

    let common_exp = other_exp;
    // tmp scaled up to other_exp; the gap is now bounded by ~prec digits.
    let tmp_shift = u32::try_from(tmp.exp - common_exp).unwrap_or(0);
    let tmp_coeff = &tmp.coeff * pow10_u32(tmp_shift);

    if tmp_is_a {
        (tmp_coeff, other_coeff, common_exp)
    } else {
        (other_coeff, tmp_coeff, common_exp)
    }
}

fn finalize_binary(
    _py: &PyToken<'_>,
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
    if let Err(flag) = fix_decimal(ctx, &mut dec, &mut status) {
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
    _py: &PyToken<'_>,
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
    if let Err(flag) = fix_decimal(ctx, &mut dec, &mut status) {
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
    crate::with_gil_entry_nopanic!(_py, {
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

        let (ca, cb, common_exp) = normalize_add_operands(a, b, ctx.prec);
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
    crate::with_gil_entry_nopanic!(_py, {
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

        let (ca, cb, common_exp) = normalize_add_operands(a, b, ctx.prec);
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
        if let Err(flag) = fix_decimal(ctx, &mut out, &mut status)
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return MoltObject::from_bool(false).bits();
        };
        MoltObject::from_bool(a.special == DecimalSpecial::Finite).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_is_infinite(a_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return MoltObject::from_bool(false).bits();
        };
        MoltObject::from_bool(a.special == DecimalSpecial::Infinity).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_is_nan(a_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return MoltObject::from_bool(false).bits();
        };
        MoltObject::from_bool(a.special == DecimalSpecial::Nan || a.special == DecimalSpecial::SNan)
            .bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_is_zero(a_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return MoltObject::from_bool(false).bits();
        };
        MoltObject::from_bool(a.special == DecimalSpecial::Finite && a.coeff.is_zero()).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_is_signed(a_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return MoltObject::from_bool(false).bits();
        };
        MoltObject::from_bool(a.sign).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_is_normal(ctx_bits: u64, a_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
        // CPython: is_normal iff context.Emin <= self.adjusted().
        MoltObject::from_bool(ctx.emin <= adjusted).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_is_subnormal(ctx_bits: u64, a_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
        // CPython: is_subnormal iff self.adjusted() < context.Emin.
        MoltObject::from_bool(adjusted < ctx.emin).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_number_class(ctx_bits: u64, a_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
                    // CPython number_class: subnormal iff adjusted() < Emin.
                    if adjusted < ctx.emin {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let ctx = unsafe { &*ctx_ptr };
        if a.special != DecimalSpecial::Finite || a.exp >= 0 {
            // Specials and already-integral values are returned unchanged.
            return decimal_bits(a.clone());
        }
        // CPython to_integral_value: quiet _rescale(0) — NO Inexact/Rounded.
        let Some(mut ans) = rescale_quiet(a, 0, ctx.rounding) else {
            return raise_exception::<u64>(_py, "InvalidContext", "decimal signal");
        };
        ans.sign = ans.sign && !ans.coeff.is_zero();
        decimal_bits(ans)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_to_integral_exact(ctx_bits: u64, a_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let ctx = unsafe { &mut *ctx_ptr };
        if a.special != DecimalSpecial::Finite || a.exp >= 0 {
            return decimal_bits(a.clone());
        }
        if a.coeff.is_zero() {
            // Zero -> 0E0 with no signals.
            return decimal_bits(DecimalHandle {
                sign: a.sign,
                coeff: BigInt::zero(),
                exp: 0,
                special: DecimalSpecial::Finite,
            });
        }
        // CPython to_integral_exact: _rescale(0), then Inexact (if changed) + Rounded.
        let Some(mut ans) = rescale_quiet(a, 0, ctx.rounding) else {
            return raise_exception::<u64>(_py, "InvalidContext", "decimal signal");
        };
        ans.sign = ans.sign && !ans.coeff.is_zero();
        let mut status = MPD_ROUNDED;
        if compare_finite(&ans, a) != Ordering::Equal {
            status |= MPD_INEXACT;
        }
        if let Err(bits) = apply_status(_py, ctx, status) {
            return bits;
        }
        decimal_bits(ans)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_to_eng_string(a_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

/// Largest finite Decimal representable in `ctx` (`9...9 * 10^Etop`, prec nines)
/// with the given sign — CPython's `_dec_from_triple(sign, '9'*prec, Etop())`,
/// the next_plus/next_minus result for an infinite operand.
fn prec_nines(ctx: &DecimalContextHandle, sign: bool) -> DecimalHandle {
    let coeff = pow10_i64(ctx.prec).unwrap_or_else(BigInt::zero) - BigInt::one();
    DecimalHandle {
        sign,
        coeff,
        exp: ctx.etop(),
        special: DecimalSpecial::Finite,
    }
}

/// Shared core of next_plus (`toward_pos = true`) and next_minus.
///
/// Faithful port of CPython `Decimal.next_plus` / `Decimal.next_minus`:
/// round the value toward +/-Infinity under a flag-suppressed copy of the
/// context; if that rounding changes the numeric value, it IS the neighbour;
/// otherwise step by one unit in the last place, `1 * 10**(Etiny - 1)`.
fn decimal_next(_py: &PyToken<'_>, ctx: &DecimalContextHandle, a: &DecimalHandle, toward_pos: bool) -> u64 {
    // Infinity handling.
    if a.special == DecimalSpecial::Infinity {
        if a.sign != toward_pos {
            // +Inf for next_plus, -Inf for next_minus: unchanged.
            return decimal_bits(a.clone());
        }
        // -Inf.next_plus() -> -Nmax(nines); +Inf.next_minus() -> +Nmax(nines).
        // The result keeps the operand's own sign (CPython _dec_from_triple sign).
        return decimal_bits(prec_nines(ctx, a.sign));
    }
    if a.special != DecimalSpecial::Finite {
        return raise_exception::<u64>(_py, "InvalidOperation", "decimal signal");
    }

    // Round toward the requested infinity under a flag-suppressed context copy.
    let mut local = ctx.clone();
    local.rounding = if toward_pos {
        MPD_ROUND_CEILING
    } else {
        MPD_ROUND_FLOOR
    };
    let mut fixed = a.clone();
    let mut scratch = 0u32; // _ignore_all_flags(): rounding signals are discarded.
    if fix_decimal(&local, &mut fixed, &mut scratch).is_err() {
        return raise_exception::<u64>(_py, "InvalidContext", "decimal signal");
    }

    // If _fix produced a different numeric value, that is the neighbour.
    let changed = if fixed.special != DecimalSpecial::Finite {
        true
    } else {
        compare_finite(&fixed, a) != Ordering::Equal
    };
    if changed {
        return decimal_bits(fixed);
    }

    // Otherwise step by one ULP: self +/- 1e(Etiny-1), rounded under `local`.
    let mut result = if a.coeff.is_zero() {
        // 0 +/- 1e(Etiny-1): the sum is exactly the epsilon (CPython's __add__
        // zero-operand branch returns the other operand unchanged), with the sign
        // set by the step direction. _fix then rounds it to the Etiny boundary.
        DecimalHandle {
            sign: !toward_pos,
            coeff: BigInt::one(),
            exp: local.etiny() - 1,
            special: DecimalSpecial::Finite,
        }
    } else {
        // Use the _normalize-capped alignment so the (possibly enormous) exponent
        // gap between `a` and the Etiny-1 epsilon does not blow up the coefficient.
        let epsilon = DecimalHandle {
            sign: false,
            coeff: BigInt::one(),
            exp: local.etiny() - 1,
            special: DecimalSpecial::Finite,
        };
        let (ca, ce, common_exp) = normalize_add_operands(a, &epsilon, local.prec);
        let sa = if a.sign { -ca } else { ca };
        let combined = if toward_pos { sa + ce } else { sa - ce };
        let sign = combined.is_negative();
        let coeff = combined.abs();
        DecimalHandle {
            sign: sign && !coeff.is_zero(),
            coeff,
            exp: common_exp,
            special: DecimalSpecial::Finite,
        }
    };
    let mut scratch2 = 0u32;
    if fix_decimal(&local, &mut result, &mut scratch2).is_err() {
        return raise_exception::<u64>(_py, "InvalidContext", "decimal signal");
    }
    decimal_bits(result)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_next_plus(ctx_bits: u64, a_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let ctx = unsafe { &*ctx_ptr };
        decimal_next(_py, ctx, a, true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_next_minus(ctx_bits: u64, a_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ctx_ptr = match context_ptr_from_bits(ctx_bits) {
            Some(ptr) => ptr,
            None => ensure_current_context(),
        };
        let Some(a) = decimal_handle_from_bits(a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid decimal handle");
        };
        let ctx = unsafe { &*ctx_ptr };
        decimal_next(_py, ctx, a, false)
    })
}

// ── Copy operations ─────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_copy_abs(a_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
        let (ctx_ptr, a, b) = match binary_arith_setup(_py, ctx_bits, a_bits, b_bits) {
            Ok(t) => t,
            Err(bits) => return bits,
        };
        let ctx = unsafe { &mut *ctx_ptr };
        // NaN second argument is an InvalidOperation; NaN first propagates (handled
        // by the Python shim's NaN routing). The second argument must be integral.
        if b.special != DecimalSpecial::Finite || b.exp != 0 {
            return raise_exception::<u64>(_py, "InvalidOperation", "decimal signal");
        }
        // Shift must satisfy |shift| <= 2*(Emax + prec) (CPython scaleb bounds).
        let limit = 2i64.saturating_mul(ctx.emax.saturating_add(ctx.prec));
        let Some(mut shift) = b.coeff.to_i64() else {
            return raise_exception::<u64>(_py, "InvalidOperation", "decimal signal");
        };
        if b.sign {
            shift = -shift;
        }
        if !(-limit <= shift && shift <= limit) {
            return raise_exception::<u64>(_py, "InvalidOperation", "decimal signal");
        }
        // Infinite first argument is returned unchanged.
        if a.special == DecimalSpecial::Infinity {
            return decimal_bits(a.clone());
        }
        if a.special != DecimalSpecial::Finite {
            return raise_exception::<u64>(_py, "InvalidOperation", "decimal signal");
        }
        let mut dec = DecimalHandle {
            sign: a.sign,
            coeff: a.coeff.clone(),
            exp: a.exp + shift,
            special: DecimalSpecial::Finite,
        };
        let mut status = 0u32;
        if let Err(flag) = fix_decimal(ctx, &mut dec, &mut status) {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_decimal_remainder_near(ctx_bits: u64, a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
        let (ca, cc, common_exp) = normalize_add_operands(&product, c, ctx.prec);
        let sa = if product.sign { -ca } else { ca };
        let sc = if c.sign { -cc } else { cc };
        let sum = sa + sc;
        let sign = sum.is_negative();
        let coeff = sum.abs();
        finalize_binary(_py, ctx, sign, coeff, common_exp)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(prec: i64, emin: i64, emax: i64, clamp: i32, rounding: i32) -> DecimalContextHandle {
        DecimalContextHandle {
            prec,
            traps: 0,
            status: 0,
            rounding,
            capitals: 1,
            emin,
            emax,
            clamp,
            refs: 1,
        }
    }

    fn fin(sign: bool, coeff: &str, exp: i64) -> DecimalHandle {
        DecimalHandle {
            sign,
            coeff: coeff.parse::<BigInt>().unwrap(),
            exp,
            special: DecimalSpecial::Finite,
        }
    }

    fn adjusted(d: &DecimalHandle) -> i64 {
        d.exp + digits_len(&d.coeff) - 1
    }

    #[test]
    fn default_context_has_cpython_bounds() {
        let c = default_context();
        assert_eq!(c.emin, -999_999);
        assert_eq!(c.emax, 999_999);
        assert_eq!(c.clamp, 0);
        assert_eq!(c.prec, 28);
        // Etiny = Emin - prec + 1 = -1000026; Etop = Emax - prec + 1 = 999972.
        assert_eq!(c.etiny(), -1_000_026);
        assert_eq!(c.etop(), 999_972);
    }

    #[test]
    fn is_normal_subnormal_at_default_emin() {
        // Decimal('1e-100') has adjusted exponent -100, which is >> default Emin
        // (-999999), so it is NORMAL, not subnormal. This is the headline P0:
        // the phantom emin=1-prec=-27 made this misclassified.
        let c = default_context();
        let d = fin(false, "1", -100);
        assert_eq!(adjusted(&d), -100);
        assert!(c.emin <= adjusted(&d), "1e-100 must be normal at default Emin");
        assert!(!(adjusted(&d) < c.emin), "1e-100 must NOT be subnormal");
    }

    #[test]
    fn is_subnormal_below_custom_emin() {
        // With Emin=-50, 1e-100 (adjusted -100) IS subnormal.
        let c = ctx(28, -50, 999_999, 0, MPD_ROUND_HALF_EVEN);
        let d = fin(false, "1", -100);
        assert!(adjusted(&d) < c.emin);
        assert!(!(c.emin <= adjusted(&d)));
    }

    #[test]
    fn fix_overflow_above_emax_traps_and_signals() {
        // prec=3, Emax=2 (Etop=0). 1.23e5 has adjusted=5 > Emax -> Overflow.
        let c = ctx(3, -2, 2, 0, MPD_ROUND_HALF_EVEN);
        let mut d = fin(false, "123", 3); // 1.23e5, adjusted = 5
        let mut status = 0u32;
        fix_decimal(&c, &mut d, &mut status).unwrap();
        assert!(status & MPD_OVERFLOW != 0);
        assert!(status & MPD_INEXACT != 0);
        assert!(status & MPD_ROUNDED != 0);
        // ROUND_HALF_EVEN overflow result is +Infinity.
        assert_eq!(d.special, DecimalSpecial::Infinity);
    }

    #[test]
    fn fix_overflow_round_down_yields_nmax() {
        // ROUND_DOWN overflow result is the largest finite (Nmax).
        let c = ctx(3, -2, 2, 0, MPD_ROUND_DOWN);
        let mut d = fin(false, "123", 3);
        let mut status = 0u32;
        fix_decimal(&c, &mut d, &mut status).unwrap();
        assert!(status & MPD_OVERFLOW != 0);
        assert_eq!(d.special, DecimalSpecial::Finite);
        // Nmax = 999 * 10^Etop, Etop = Emax - prec + 1 = 0.
        assert_eq!(d.coeff, BigInt::from(999));
        assert_eq!(d.exp, 0);
    }

    #[test]
    fn fix_subnormal_underflow_signals() {
        // prec=3, Emin=-2 => Etiny = -4. A value at 1e-5 (adjusted -5 < Emin)
        // rounds into the subnormal range and underflows.
        let c = ctx(3, -2, 999_999, 0, MPD_ROUND_HALF_EVEN);
        let mut d = fin(false, "15", -6); // 1.5e-5, adjusted = -5
        let mut status = 0u32;
        fix_decimal(&c, &mut d, &mut status).unwrap();
        assert!(status & MPD_SUBNORMAL != 0, "must signal Subnormal");
        // It was inexact (rounded to Etiny=-4), so Underflow + Inexact + Rounded.
        assert!(status & MPD_UNDERFLOW != 0, "must signal Underflow");
        assert!(status & MPD_INEXACT != 0);
        assert!(status & MPD_ROUNDED != 0);
        assert_eq!(d.exp, c.etiny());
    }

    #[test]
    fn fix_representable_value_is_unchanged_and_silent() {
        let c = default_context();
        let mut d = fin(false, "12345", -2); // 123.45, well within bounds
        let mut status = 0u32;
        fix_decimal(&c, &mut d, &mut status).unwrap();
        assert_eq!(status, 0, "in-range value must raise no signals");
        assert_eq!(d.coeff, BigInt::from(12345));
        assert_eq!(d.exp, -2);
    }

    #[test]
    fn fix_zero_clamps_exponent_into_range() {
        // prec=3, Emin=-2 => Etiny=-4. Zero with exp=-10 clamps up to Etiny.
        let c = ctx(3, -2, 2, 0, MPD_ROUND_HALF_EVEN);
        let mut d = fin(false, "0", -10);
        let mut status = 0u32;
        fix_decimal(&c, &mut d, &mut status).unwrap();
        assert!(status & MPD_CLAMPED != 0);
        assert_eq!(d.exp, c.etiny());
        assert!(d.coeff.is_zero());
    }

    #[test]
    fn fix_clamp_one_folds_down_exponent() {
        // clamp=1: a value whose exponent exceeds Etop is folded down (padded).
        // prec=3, Emax=5 => Etop = 3. 1e4 (coeff '1', exp 4) > Etop -> fold to Etop.
        let c = ctx(3, -5, 5, 1, MPD_ROUND_HALF_EVEN);
        let mut d = fin(false, "1", 4);
        let mut status = 0u32;
        fix_decimal(&c, &mut d, &mut status).unwrap();
        assert!(status & MPD_CLAMPED != 0);
        assert_eq!(d.exp, c.etop());
        // Coefficient padded: 1 * 10^(4-3) = 10.
        assert_eq!(d.coeff, BigInt::from(10));
    }

    #[test]
    fn rescale_quiet_rounds_to_integer() {
        // _rescale(0) of 1.2345 with HALF_EVEN -> 1, no flags consulted here.
        let d = fin(false, "12345", -4); // 1.2345
        let r = rescale_quiet(&d, 0, MPD_ROUND_HALF_EVEN).unwrap();
        assert_eq!(r.coeff, BigInt::from(1));
        assert_eq!(r.exp, 0);
    }

    #[test]
    fn etiny_etop_match_cpython_formula() {
        let c = ctx(9, -10, 10, 0, MPD_ROUND_HALF_EVEN);
        assert_eq!(c.etiny(), -10 - 9 + 1); // -18
        assert_eq!(c.etop(), 10 - 9 + 1); // 2
    }
}
