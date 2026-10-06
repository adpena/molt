use crate::PyToken;
use std::cmp::Ordering;
use std::mem;
use std::sync::atomic::Ordering as AtomicOrdering;

use molt_obj_model::MoltObject;
use num_bigint::{BigInt, Sign};
use num_traits::{ToPrimitive, Zero};

use crate::object::class_layout::{ScalarValueKind, scalar_value_bits};
use crate::object::ops::{as_float_extended, is_float_extended};
use crate::{
    INLINE_INT_MAX_I128, INLINE_INT_MIN_I128, MoltHeader, TYPE_ID_BIGINT, TYPE_ID_COMPLEX,
    alloc_object, call_callable0, class_name_for_error, dec_ref_bits, exception_pending,
    maybe_ptr_from_bits, obj_from_bits, object_type_id, raise_exception, runtime_state_for_gil,
    type_of_bits,
};

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct ComplexParts {
    pub(crate) re: f64,
    pub(crate) im: f64,
}

fn builtin_int_bits_for_gil() -> Option<u64> {
    let state = runtime_state_for_gil()?;
    let ptr = state.builtin_classes.load(AtomicOrdering::Acquire);
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { (*ptr).int })
    }
}

fn builtin_float_bits_for_gil() -> Option<u64> {
    let state = runtime_state_for_gil()?;
    let ptr = state.builtin_classes.load(AtomicOrdering::Acquire);
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { (*ptr).float })
    }
}

pub(crate) fn int_subclass_value_bits_raw(obj_bits: u64) -> Option<u64> {
    scalar_value_bits(obj_from_bits(obj_bits), ScalarValueKind::Int)
}

#[inline(always)]
pub(crate) fn to_i64(obj: MoltObject) -> Option<i64> {
    if obj.is_int() {
        return Some(obj.as_int_unchecked());
    }
    if obj.is_bool() {
        return Some(if (obj.bits() & 0x1) == 1 { 1 } else { 0 });
    }
    // Recover integral direct float carriers emitted by codegen. A managed
    // Float intrinsic is a Python float subtype, not an integer carrier;
    // representation-complete float extraction must not widen this policy.
    if let Some(f) = as_float_extended(obj)
        && scalar_value_bits(obj, ScalarValueKind::Float).is_none()
        && f.fract() == 0.0
        && f.abs() < (1i64 << 53) as f64
    {
        return Some(f as i64);
    }
    if let Some(bits) = int_subclass_value_bits_raw(obj.bits()) {
        let val_obj = obj_from_bits(bits);
        if let Some(i) = val_obj.as_int() {
            return Some(i);
        }
        if val_obj.is_bool() {
            return Some(if val_obj.as_bool().unwrap_or(false) {
                1
            } else {
                0
            });
        }
        if let Some(ptr) = bigint_ptr_from_bits(bits) {
            return unsafe { bigint_ref(ptr) }.to_i64();
        }
    }
    // A bare heap BigInt (a Python `int` whose magnitude exceeds the inline-int
    // tag range, ~2^46) is still a plain integer. Convert it when it fits in
    // i64 so intrinsics that accept large integers (e.g. os.utime nanosecond
    // timestamps) see the value instead of falling through to `None`.
    if let Some(ptr) = bigint_ptr_from_bits(obj.bits()) {
        return unsafe { bigint_ref(ptr) }.to_i64();
    }
    None
}

pub(crate) fn bigint_ptr_from_bits(bits: u64) -> Option<*mut u8> {
    let ptr = maybe_ptr_from_bits(bits)?;
    unsafe {
        if object_type_id(ptr) == TYPE_ID_BIGINT {
            Some(ptr)
        } else {
            None
        }
    }
}

pub(crate) fn complex_ptr_from_bits(bits: u64) -> Option<*mut u8> {
    let ptr = maybe_ptr_from_bits(bits)?;
    unsafe {
        if object_type_id(ptr) == TYPE_ID_COMPLEX {
            Some(ptr)
        } else {
            None
        }
    }
}

pub(crate) unsafe fn complex_ref(ptr: *mut u8) -> &'static ComplexParts {
    unsafe { &*(ptr as *const ComplexParts) }
}

pub(crate) fn complex_bits(_py: &PyToken<'_>, re: f64, im: f64) -> u64 {
    let total = mem::size_of::<MoltHeader>() + mem::size_of::<ComplexParts>();
    let ptr = alloc_object(_py, total, TYPE_ID_COMPLEX);
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    unsafe {
        std::ptr::write(ptr as *mut ComplexParts, ComplexParts { re, im });
    }
    MoltObject::from_ptr(ptr).bits()
}

pub(crate) fn complex_from_obj_strict(
    _py: &PyToken<'_>,
    obj: MoltObject,
) -> Result<Option<ComplexParts>, ()> {
    if let Some(ptr) = complex_ptr_from_bits(obj.bits()) {
        return Ok(Some(unsafe { *complex_ref(ptr) }));
    }
    if let Some(re) = as_float_extended(obj) {
        return Ok(Some(ComplexParts { re, im: 0.0 }));
    }
    if let Some(value) = index_i64_integral_bits(obj.bits()) {
        return Ok(Some(ComplexParts {
            re: value as f64,
            im: 0.0,
        }));
    }
    if let Some(value) = index_bigint_integral_bits(obj.bits()) {
        return checked_integer_double(&value)
            .map(|re| Some(ComplexParts { re, im: 0.0 }))
            .ok_or(());
    }
    Ok(None)
}

/// An operand of complex arithmetic: a complex number, or the real operand of
/// a mixed real/complex operation, a `float` or an `int` converted like
/// `float()`.
#[derive(Clone, Copy, Debug)]
pub(crate) enum ComplexOperand {
    Complex(ComplexParts),
    Real(f64),
}

impl ComplexOperand {
    /// The operand as the `complex(x, 0.0)` that arithmetic before 3.14 uses.
    #[inline]
    fn parts(self) -> ComplexParts {
        match self {
            Self::Complex(parts) => parts,
            Self::Real(re) => ComplexParts { re, im: 0.0 },
        }
    }
}

/// `obj` as a complex arithmetic operand: `Ok(None)` for a type complex
/// arithmetic does not accept, `Err(())` for an int too large for a float.
pub(crate) fn complex_operand(
    _py: &PyToken<'_>,
    obj: MoltObject,
) -> Result<Option<ComplexOperand>, ()> {
    if let Some(ptr) = complex_ptr_from_bits(obj.bits()) {
        return Ok(Some(ComplexOperand::Complex(unsafe { *complex_ref(ptr) })));
    }
    Ok(complex_from_obj_strict(_py, obj)?.map(|parts| ComplexOperand::Real(parts.re)))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ComplexArith {
    Add,
    Sub,
    Mul,
    TrueDiv,
}

/// CPython's complex arithmetic (`Objects/complexobject.c`) for the target
/// Python; `None` for a zero divisor. From 3.14 a real operand combines with
/// the complex one directly (gh-69639), so no imaginary zero of its own can
/// change the sign of a zero or produce a NaN; before 3.14 it is
/// `complex(x, 0.0)`. From 3.14 a product or quotient of two complex numbers
/// that computes as `nan+nanj` also recovers the infinities and zeros of
/// C11 Annex G (gh-120010, gh-119372).
pub(crate) fn complex_arith(
    op: ComplexArith,
    lhs: ComplexOperand,
    rhs: ComplexOperand,
    py314: bool,
) -> Option<ComplexParts> {
    if py314 {
        match (lhs, rhs) {
            (ComplexOperand::Complex(a), ComplexOperand::Real(b)) => {
                return complex_real_arith(op, a, b);
            }
            (ComplexOperand::Real(a), ComplexOperand::Complex(b)) => {
                return real_complex_arith(op, a, b);
            }
            _ => {}
        }
    }
    let (a, b) = (lhs.parts(), rhs.parts());
    match op {
        ComplexArith::Add => Some(ComplexParts {
            re: a.re + b.re,
            im: a.im + b.im,
        }),
        ComplexArith::Sub => Some(ComplexParts {
            re: a.re - b.re,
            im: a.im - b.im,
        }),
        ComplexArith::Mul => Some(complex_prod(a, b, py314)),
        ComplexArith::TrueDiv => complex_quot(a, b, py314),
    }
}

/// `_Py_cr_sum`, `_Py_cr_diff`, `_Py_cr_prod`, `_Py_cr_quot`.
fn complex_real_arith(op: ComplexArith, a: ComplexParts, b: f64) -> Option<ComplexParts> {
    Some(match op {
        ComplexArith::Add => ComplexParts {
            re: a.re + b,
            im: a.im,
        },
        ComplexArith::Sub => ComplexParts {
            re: a.re - b,
            im: a.im,
        },
        ComplexArith::Mul => ComplexParts {
            re: a.re * b,
            im: a.im * b,
        },
        ComplexArith::TrueDiv => {
            if b == 0.0 {
                return None;
            }
            ComplexParts {
                re: a.re / b,
                im: a.im / b,
            }
        }
    })
}

/// `_Py_rc_sum`, `_Py_rc_diff`, `_Py_rc_prod`, `_Py_rc_quot`.
fn real_complex_arith(op: ComplexArith, a: f64, b: ComplexParts) -> Option<ComplexParts> {
    Some(match op {
        ComplexArith::Add => ComplexParts {
            re: b.re + a,
            im: b.im,
        },
        ComplexArith::Sub => ComplexParts {
            re: a - b.re,
            im: -b.im,
        },
        ComplexArith::Mul => ComplexParts {
            re: b.re * a,
            im: b.im * a,
        },
        ComplexArith::TrueDiv => return real_complex_quot(a, b),
    })
}

/// An infinity "boxed" to a signed one, and anything else to a signed zero.
#[inline]
fn annex_g_box(x: f64) -> f64 {
    (if x.is_infinite() { 1.0f64 } else { 0.0 }).copysign(x)
}

/// `_Py_c_prod`.
fn complex_prod(z: ComplexParts, w: ComplexParts, recover: bool) -> ComplexParts {
    let (mut a, mut b, mut c, mut d) = (z.re, z.im, w.re, w.im);
    let (ac, bd, ad, bc) = (a * c, b * d, a * d, b * c);
    let mut r = ComplexParts {
        re: ac - bd,
        im: ad + bc,
    };
    if recover && r.re.is_nan() && r.im.is_nan() {
        let mut recalc = false;
        if a.is_infinite() || b.is_infinite() {
            a = annex_g_box(a);
            b = annex_g_box(b);
            if c.is_nan() {
                c = 0.0f64.copysign(c);
            }
            if d.is_nan() {
                d = 0.0f64.copysign(d);
            }
            recalc = true;
        }
        if c.is_infinite() || d.is_infinite() {
            c = annex_g_box(c);
            d = annex_g_box(d);
            if a.is_nan() {
                a = 0.0f64.copysign(a);
            }
            if b.is_nan() {
                b = 0.0f64.copysign(b);
            }
            recalc = true;
        }
        if !recalc && (ac.is_infinite() || bd.is_infinite() || ad.is_infinite() || bc.is_infinite())
        {
            // An overflowed partial product: its NaN partners become zeros.
            for part in [&mut a, &mut b, &mut c, &mut d] {
                if part.is_nan() {
                    *part = 0.0f64.copysign(*part);
                }
            }
            recalc = true;
        }
        if recalc {
            r.re = f64::INFINITY * (a * c - b * d);
            r.im = f64::INFINITY * (a * d + b * c);
        }
    }
    r
}

/// `_Py_c_quot`: Smith's algorithm, dividing by the larger denominator part.
fn complex_quot(a: ComplexParts, b: ComplexParts, recover: bool) -> Option<ComplexParts> {
    let abs_breal = b.re.abs();
    let abs_bimag = b.im.abs();
    let mut r = if abs_breal >= abs_bimag {
        if abs_breal == 0.0 {
            return None;
        }
        let ratio = b.im / b.re;
        let denom = b.re + b.im * ratio;
        ComplexParts {
            re: (a.re + a.im * ratio) / denom,
            im: (a.im - a.re * ratio) / denom,
        }
    } else if abs_bimag >= abs_breal {
        let ratio = b.re / b.im;
        let denom = b.re * ratio + b.im;
        ComplexParts {
            re: (a.re * ratio + a.im) / denom,
            im: (a.im * ratio - a.re) / denom,
        }
    } else {
        // A NaN denominator part.
        ComplexParts {
            re: f64::NAN,
            im: f64::NAN,
        }
    };
    if recover && r.re.is_nan() && r.im.is_nan() {
        if (a.re.is_infinite() || a.im.is_infinite()) && b.re.is_finite() && b.im.is_finite() {
            let x = annex_g_box(a.re);
            let y = annex_g_box(a.im);
            r.re = f64::INFINITY * (x * b.re + y * b.im);
            r.im = f64::INFINITY * (y * b.re - x * b.im);
        } else if (abs_breal.is_infinite() || abs_bimag.is_infinite())
            && a.re.is_finite()
            && a.im.is_finite()
        {
            let x = annex_g_box(b.re);
            let y = annex_g_box(b.im);
            r.re = 0.0 * (a.re * x + a.im * y);
            r.im = 0.0 * (a.im * x - a.re * y);
        }
    }
    Some(r)
}

/// `_Py_rc_quot`: [`complex_quot`] for a real dividend, without the terms of
/// its (absent) imaginary zero.
fn real_complex_quot(a: f64, b: ComplexParts) -> Option<ComplexParts> {
    let abs_breal = b.re.abs();
    let abs_bimag = b.im.abs();
    let mut r = if abs_breal >= abs_bimag {
        if abs_breal == 0.0 {
            return None;
        }
        let ratio = b.im / b.re;
        let denom = b.re + b.im * ratio;
        ComplexParts {
            re: a / denom,
            im: (-a * ratio) / denom,
        }
    } else if abs_bimag >= abs_breal {
        let ratio = b.re / b.im;
        let denom = b.re * ratio + b.im;
        ComplexParts {
            re: (a * ratio) / denom,
            im: (-a) / denom,
        }
    } else {
        ComplexParts {
            re: f64::NAN,
            im: f64::NAN,
        }
    };
    if r.re.is_nan()
        && r.im.is_nan()
        && a.is_finite()
        && (abs_breal.is_infinite() || abs_bimag.is_infinite())
    {
        let x = annex_g_box(b.re);
        let y = annex_g_box(b.im);
        r.re = 0.0 * (a * x);
        r.im = 0.0 * (-a * y);
    }
    Some(r)
}

pub(crate) fn to_bigint(obj: MoltObject) -> Option<BigInt> {
    if let Some(i) = to_i64(obj) {
        return Some(BigInt::from(i));
    }
    if let Some(ptr) = bigint_ptr_from_bits(obj.bits()) {
        return Some(unsafe { bigint_ref(ptr).clone() });
    }
    if let Some(bits) = int_subclass_value_bits_raw(obj.bits()) {
        let val_obj = obj_from_bits(bits);
        if let Some(i) = val_obj.as_int() {
            return Some(BigInt::from(i));
        }
        if val_obj.is_bool() {
            return Some(BigInt::from(if val_obj.as_bool().unwrap_or(false) {
                1
            } else {
                0
            }));
        }
        if let Some(ptr) = bigint_ptr_from_bits(bits) {
            return Some(unsafe { bigint_ref(ptr).clone() });
        }
    }
    None
}

pub(crate) const INT_BYTES_OK: i32 = 0;
pub(crate) const INT_BYTES_OVERFLOW: i32 = 1;
pub(crate) const INT_BYTES_NEGATIVE_UNSIGNED: i32 = 2;
pub(crate) const INT_BYTES_INVALID: i32 = -1;

/// Shared arbitrary-width two's-complement decoder for Python `int.from_bytes`
/// and the CPython ABI `_PyLong_FromByteArray` hook.
pub(crate) fn bigint_from_bytes(data: &[u8], little_endian: bool, signed: bool) -> BigInt {
    match (little_endian, signed) {
        (true, true) => BigInt::from_signed_bytes_le(data),
        (false, true) => BigInt::from_signed_bytes_be(data),
        (true, false) => BigInt::from_bytes_le(Sign::Plus, data),
        (false, false) => BigInt::from_bytes_be(Sign::Plus, data),
    }
}

/// Shared arbitrary-width fixed-width encoder.
///
/// On magnitude overflow the low `out.len()` bytes are still written before
/// `INT_BYTES_OVERFLOW` is returned, matching `_PyLong_AsByteArray`.
pub(crate) fn bigint_to_bytes(
    value: &BigInt,
    out: &mut [u8],
    little_endian: bool,
    signed: bool,
) -> i32 {
    if !signed && value.sign() == Sign::Minus {
        return INT_BYTES_NEGATIVE_UNSIGNED;
    }

    // num-bigint already owns the minimal two's-complement/sign-magnitude
    // encoder. Use its single output buffer directly: the prior modulus,
    // remainder, normalization and padding path allocated several full-width
    // BigInts and a second Vec for every conversion.
    let bytes = if signed {
        if little_endian {
            value.to_signed_bytes_le()
        } else {
            value.to_signed_bytes_be()
        }
    } else if little_endian {
        value.to_bytes_le().1
    } else {
        value.to_bytes_be().1
    };
    let fits = value.is_zero() && out.is_empty() || bytes.len() <= out.len();
    let pad = if signed && value.sign() == Sign::Minus {
        0xff
    } else {
        0
    };
    out.fill(pad);
    let copied = bytes.len().min(out.len());
    if copied != 0 {
        if little_endian {
            out[..copied].copy_from_slice(&bytes[..copied]);
        } else {
            let out_start = out.len() - copied;
            let bytes_start = bytes.len() - copied;
            out[out_start..].copy_from_slice(&bytes[bytes_start..]);
        }
    };
    if fits {
        INT_BYTES_OK
    } else {
        INT_BYTES_OVERFLOW
    }
}

pub(crate) fn bigint_num_bits(value: &BigInt) -> Option<usize> {
    usize::try_from(value.bits()).ok()
}

pub(crate) fn bigint_to_inline(value: &BigInt) -> Option<i64> {
    let val = value.to_i64()?;
    if (val as i128) >= INLINE_INT_MIN_I128 && (val as i128) <= INLINE_INT_MAX_I128 {
        Some(val)
    } else {
        None
    }
}

pub(crate) fn int_bits_from_bigint(_py: &PyToken<'_>, value: BigInt) -> u64 {
    if let Some(i) = bigint_to_inline(&value) {
        return MoltObject::from_int(i).bits();
    }
    bigint_bits(_py, value)
}

pub(crate) fn inline_int_from_i128(val: i128) -> Option<i64> {
    if (INLINE_INT_MIN_I128..=INLINE_INT_MAX_I128).contains(&val) {
        Some(val as i64)
    } else {
        None
    }
}

pub(crate) fn int_bits_from_i64(_py: &PyToken<'_>, val: i64) -> u64 {
    if let Some(inline) = inline_int_from_i128(val as i128) {
        return MoltObject::from_int(inline).bits();
    }
    bigint_bits(_py, BigInt::from(val))
}

pub(crate) fn bigint_bits(_py: &PyToken<'_>, value: BigInt) -> u64 {
    // BigInt contains a Vec<u64> which stores digits on the heap.
    // The BigInt struct itself is fixed-size (Sign enum + Vec metadata).
    // We allocate space for MoltHeader + the BigInt struct.
    // The Vec's heap buffer is separate and managed by the Vec allocator.
    let bigint_size = mem::size_of::<BigInt>();
    let total = mem::size_of::<MoltHeader>() + bigint_size;
    let ptr = alloc_object(_py, total, TYPE_ID_BIGINT);
    if ptr.is_null() {
        crate::record_memory_error_without_allocation(_py);
        return MoltObject::none().bits();
    }
    unsafe {
        std::ptr::write(ptr as *mut BigInt, value);
    }
    MoltObject::from_ptr(ptr).bits()
}

#[inline]
pub(crate) fn int_bits_from_i128(_py: &PyToken<'_>, val: i128) -> u64 {
    if let Some(i) = inline_int_from_i128(val) {
        MoltObject::from_int(i).bits()
    } else {
        bigint_bits(_py, BigInt::from(val))
    }
}

pub(crate) unsafe fn bigint_ref(ptr: *mut u8) -> &'static BigInt {
    unsafe { &*(ptr as *const BigInt) }
}

pub(crate) fn compare_bigint_float(big: &BigInt, f: f64) -> Option<Ordering> {
    if f.is_nan() {
        return None;
    }
    if f.is_infinite() {
        if f.is_sign_positive() {
            return Some(Ordering::Less);
        }
        return Some(Ordering::Greater);
    }
    // Compare to the exact integral part, never a rounded integer-to-float
    // conversion (which equates 2**53+1 with float(2**53)). The fractional
    // remainder only matters when the integer parts are equal.
    let integer_order = big.cmp(&bigint_from_f64_trunc(f));
    if integer_order != Ordering::Equal {
        return Some(integer_order);
    }
    match f.fract().partial_cmp(&0.0) {
        Some(Ordering::Greater) => Some(Ordering::Less),
        Some(Ordering::Less) => Some(Ordering::Greater),
        _ => Some(Ordering::Equal),
    }
}

pub(crate) fn bigint_from_f64_trunc(val: f64) -> BigInt {
    if val == 0.0 {
        return BigInt::from(0);
    }
    let bits = val.to_bits();
    let sign = if (bits >> 63) != 0 { -1 } else { 1 };
    let exp_bits = ((bits >> 52) & 0x7ff) as i32;
    let frac_bits = bits & ((1u64 << 52) - 1);
    let (mantissa, exp) = if exp_bits == 0 {
        (frac_bits, 1 - 1023 - 52)
    } else {
        ((1u64 << 52) | frac_bits, exp_bits - 1023 - 52)
    };
    let mut big = BigInt::from(mantissa);
    if exp >= 0 {
        big <<= exp as usize;
    } else {
        big >>= (-exp) as usize;
    }
    if sign < 0 { -big } else { big }
}

pub(crate) fn round_half_even(val: f64) -> f64 {
    if !val.is_finite() {
        return val;
    }
    let floor = val.floor();
    let ceil = val.ceil();
    let diff_floor = (val - floor).abs();
    let diff_ceil = (ceil - val).abs();
    if diff_floor < diff_ceil {
        return floor;
    }
    if diff_ceil < diff_floor {
        return ceil;
    }
    if floor.abs() > i64::MAX as f64 {
        return floor;
    }
    let floor_int = floor as i64;
    if floor_int & 1 == 0 { floor } else { ceil }
}

/// Borrow a validated integer carrier from direct or sealed tagged Int storage.
/// At most one intrinsic word is read: nested objects and every float carrier
/// are rejected. The caller keeps the original owner alive while using these
/// bits. This projection neither allocates/retains nor dispatches protocols.
pub(crate) fn index_integral_payload_bits(bits: u64) -> Option<u64> {
    let payload = int_subclass_value_bits_raw(bits).unwrap_or(bits);
    let obj = obj_from_bits(payload);
    (obj.is_int() || obj.is_bool() || bigint_ptr_from_bits(payload).is_some()).then_some(payload)
}

pub(crate) fn index_i64_integral_bits(bits: u64) -> Option<i64> {
    let payload = index_integral_payload_bits(bits)?;
    let obj = obj_from_bits(payload);
    if let Some(value) = obj.as_int() {
        return Some(value);
    }
    if let Some(value) = obj.as_bool() {
        return Some(i64::from(value));
    }
    unsafe { bigint_ref(bigint_ptr_from_bits(payload)?) }.to_i64()
}

pub(crate) fn index_bigint_integral_bits(bits: u64) -> Option<BigInt> {
    let payload = index_integral_payload_bits(bits)?;
    let obj = obj_from_bits(payload);
    if let Some(value) = obj.as_int() {
        return Some(BigInt::from(value));
    }
    if let Some(value) = obj.as_bool() {
        return Some(BigInt::from(u8::from(value)));
    }
    Some(unsafe { bigint_ref(bigint_ptr_from_bits(payload)?) }.clone())
}

/// CPython's checked C-int argument domain, after canonical __index__ dispatch.
/// Keep public API limits independent of the target pointer width and of the
/// NaN-box inline representation. Rejected values never become runtime state.
pub(crate) fn index_c_int_from_obj(
    _py: &PyToken<'_>,
    obj_bits: u64,
) -> Option<std::os::raw::c_int> {
    let value = if let Some(value) = index_i64_integral_bits(obj_bits) {
        i32::try_from(value).ok()
    } else {
        let name = class_name_for_error(type_of_bits(_py, obj_bits));
        let err = format!("'{name}' object cannot be interpreted as an integer");
        index_bigint_from_obj(_py, obj_bits, &err)?.to_i32()
    };
    value.or_else(|| {
        raise_exception::<Option<i32>>(
            _py,
            "OverflowError",
            "Python int too large to convert to C int",
        )
    })
}

/// Index-sized conversion with CPython's no-error overflow policy. Slice/search
/// bounds and byte membership share __index__ admission and target-width clipping.
pub(crate) fn index_ssize_clamped_from_obj(
    py: &PyToken<'_>,
    bits: u64,
    err: &str,
) -> Option<isize> {
    if let Some(value) = index_i64_integral_bits(bits) {
        return Some(value.clamp(isize::MIN as i64, isize::MAX as i64) as isize);
    }
    let value = index_bigint_from_obj(py, bits, err)?;
    Some(value.to_isize().unwrap_or_else(|| {
        if value.sign() == Sign::Minus {
            isize::MIN
        } else {
            isize::MAX
        }
    }))
}

pub(crate) fn index_i64_from_obj(_py: &PyToken<'_>, obj_bits: u64, err: &str) -> i64 {
    if let Some(value) = index_i64_integral_bits(obj_bits) {
        return value;
    }
    let Some(value) = index_bigint_from_obj(_py, obj_bits, err) else {
        return 0;
    };
    value.to_i64().unwrap_or_else(|| {
        raise_exception::<i64>(
            _py,
            "OverflowError",
            "Python int too large to convert to C ssize_t",
        )
    })
}

/// Mixed float arithmetic admits numeric payloads only when one operand is
/// actually a float. Conversion failure is distinct from unsupported operands:
/// callers must return the original exception before considering another slot.
#[inline]
pub(crate) fn float_pair_from_obj(
    py: &PyToken<'_>,
    lhs: MoltObject,
    rhs: MoltObject,
) -> Result<Option<(f64, f64)>, ()> {
    if !is_float_extended(lhs) && !is_float_extended(rhs) {
        return Ok(None);
    }
    fn operand(py: &PyToken<'_>, obj: MoltObject) -> Result<Option<f64>, ()> {
        if let Some(ptr) = bigint_ptr_from_bits(obj.bits()) {
            // Reuse the constructor/C-API integer conversion authority. A
            // bigint can convert to Some(infinity), which Python rejects.
            return integer_as_double(py, unsafe { bigint_ref(ptr) })
                .map(Some)
                .ok_or(());
        }
        // Existing float infinities and NaNs are values, not overflow.
        Ok(to_f64(obj))
    }
    let Some(lf) = operand(py, lhs)? else {
        return Ok(None);
    };
    let Some(rf) = operand(py, rhs)? else {
        return Ok(None);
    };
    Ok(Some((lf, rf)))
}

pub(crate) fn compare_numbers(lhs: MoltObject, rhs: MoltObject) -> Option<Ordering> {
    if let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
        return Some(li.cmp(&ri));
    }
    if let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
        return Some(l_big.cmp(&r_big));
    }
    if let Some(ptr) = bigint_ptr_from_bits(lhs.bits())
        && let Some(f) = to_f64(rhs)
    {
        return compare_bigint_float(unsafe { bigint_ref(ptr) }, f);
    }
    if let Some(ptr) = bigint_ptr_from_bits(rhs.bits())
        && let Some(f) = to_f64(lhs)
    {
        return compare_bigint_float(unsafe { bigint_ref(ptr) }, f).map(Ordering::reverse);
    }
    if let (Some(lf), Some(rf)) = (to_f64(lhs), to_f64(rhs)) {
        return lf.partial_cmp(&rf);
    }
    None
}

pub(crate) fn split_maxsplit_from_obj(_py: &PyToken<'_>, obj_bits: u64) -> i64 {
    let obj = obj_from_bits(obj_bits);
    let msg = format!(
        "'{}' object cannot be interpreted as an integer",
        crate::type_name(_py, obj)
    );
    let Some(value) = index_bigint_from_obj(_py, obj_bits, &msg) else {
        return 0;
    };
    // The boxed transport is i64 on every backend, but Python's index-sized
    // admission follows the target pointer width (also used by sys.maxsize).
    match value.to_isize() {
        Some(value) => value as i64,
        None => raise_exception::<i64>(
            _py,
            "OverflowError",
            "Python int too large to convert to C ssize_t",
        ),
    }
}

pub(crate) fn index_i64_with_overflow(
    _py: &PyToken<'_>,
    obj_bits: u64,
    err: &str,
    overflow_err: Option<&str>,
) -> Option<i64> {
    let value = index_bigint_from_obj(_py, obj_bits, err)?;
    if let Some(i) = value.to_i64() {
        return Some(i);
    }
    let msg = match overflow_err {
        Some(msg) => msg.to_string(),
        None => format!(
            "cannot fit '{}' into an index-sized integer",
            class_name_for_error(type_of_bits(_py, obj_bits))
        ),
    };
    raise_exception::<Option<i64>>(_py, "IndexError", &msg)
}

fn sequence_index_type_error(_py: &PyToken<'_>, key_bits: u64, container_tp: &str) -> String {
    format!(
        "{} indices must be integers or slices, not {}",
        container_tp,
        crate::type_name(_py, obj_from_bits(key_bits)),
    )
}

pub(crate) fn sequence_index_i64_with_type_error(
    _py: &PyToken<'_>,
    key_bits: u64,
    type_err: &str,
) -> Option<i64> {
    if let Some(i) = index_i64_integral_bits(key_bits) {
        return Some(i);
    }
    index_i64_with_overflow(_py, key_bits, type_err, None)
}

pub(crate) fn sequence_index_bigint(
    _py: &PyToken<'_>,
    key_bits: u64,
    container_tp: &str,
) -> Option<BigInt> {
    let type_err = sequence_index_type_error(_py, key_bits, container_tp);
    index_bigint_from_obj(_py, key_bits, &type_err)
}

/// Coerce a subscript key to an `i64` sequence index under CPython `__index__`
/// semantics. This is the single authority for tuple / list / bytearray index
/// extraction (and shares its integral fast path with `range`).
///
/// Accepts `int`, `bool`, an `int` subclass, a bare `bigint`, and any object
/// implementing `__index__`. REJECTS `float` (even an integral `2.0`), `str`,
/// `None`, and every other non-index type with `TypeError: <container> indices
/// must be integers or slices, not <keytype>` — byte-identical to CPython
/// 3.12+ (`_PyIndex_Check` gates sequence subscript, and `float` has no
/// `nb_index`). An index magnitude exceeding `isize` raises
/// `IndexError: cannot fit '<type>' into an index-sized integer`.
///
/// Do NOT reintroduce a `to_i64` fast path here: `to_i64` is a *numeric*
/// coercion that silently accepts integral floats, whereas sequence indexing is
/// the integer protocol (`__index__`), which floats deliberately do not satisfy.
///
/// Returns `None` with a pending exception on any error; the caller must
/// propagate (`return MoltObject::none().bits()`).
pub(crate) fn sequence_index_i64(
    _py: &PyToken<'_>,
    key_bits: u64,
    container_tp: &str,
) -> Option<i64> {
    // Fast path: int / bool / int-subclass — no BigInt allocation, no float.
    if let Some(i) = index_i64_integral_bits(key_bits) {
        return Some(i);
    }
    // Slow path: bare bigint, `__index__` objects, overflow -> IndexError, and
    // the TypeError raise for float / other non-index keys. The message is the
    // CPython "<container> indices must be integers or slices, not <keytype>"
    // sequence family, shared by tuple / list / range / byte / bytearray.
    let type_err = sequence_index_type_error(_py, key_bits, container_tp);
    index_i64_with_overflow(_py, key_bits, &type_err, None)
}

pub(crate) fn index_bigint_from_obj(_py: &PyToken<'_>, obj_bits: u64, err: &str) -> Option<BigInt> {
    if let Some(value) = index_bigint_integral_bits(obj_bits) {
        return Some(value);
    }
    if maybe_ptr_from_bits(obj_bits).is_some() {
        unsafe {
            if let Some(call_bits) =
                crate::builtins::attr::lookup_special_method(_py, obj_bits, b"__index__")
            {
                let res_bits = call_callable0(_py, call_bits);
                dec_ref_bits(_py, call_bits);
                if exception_pending(_py) {
                    dec_ref_bits(_py, res_bits);
                    return None;
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(value) = index_bigint_integral_bits(res_bits) {
                    let exact = builtin_int_bits_for_gil() == Some(type_of_bits(_py, res_bits));
                    let accepted =
                        exact || warn_numeric_subclass_result(_py, "__index__", "int", res_bits);
                    dec_ref_bits(_py, res_bits);
                    if !accepted {
                        return None;
                    }
                    return Some(value);
                }
                let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                if res_obj.as_ptr().is_some() {
                    dec_ref_bits(_py, res_bits);
                }
                let msg = format!("__index__ returned non-int (type {res_type})");
                raise_exception::<u64>(_py, "TypeError", &msg);
                return None;
            }
            if exception_pending(_py) {
                return None;
            }
        }
    }
    raise_exception::<u64>(_py, "TypeError", err);
    None
}

pub(crate) fn warn_numeric_subclass_result(
    py: &PyToken<'_>,
    protocol: &str,
    expected: &str,
    result_bits: u64,
) -> bool {
    let actual = class_name_for_error(type_of_bits(py, result_bits));
    let message = format!(
        "{protocol} returned non-{expected} (type {actual}).  The ability to return an instance of a strict subclass of {expected} is deprecated, and may be removed in a future version of Python."
    );
    crate::builtins::warnings_ext::emit_deprecation_warning(py, &message)
}

pub(crate) fn checked_integer_double(value: &BigInt) -> Option<f64> {
    value.to_f64().filter(|value| value.is_finite())
}

fn integer_as_double(py: &PyToken<'_>, value: &BigInt) -> Option<f64> {
    // num_bigint may return Some(infinity), not just None, on overflow.
    if let Some(value) = checked_integer_double(value) {
        return Some(value);
    }
    raise_exception::<()>(py, "OverflowError", "int too large to convert to float");
    None
}

/// Exact float identity is semantic: TYPE_ID_FLOAT is also subclass storage.
pub(crate) fn is_exact_float(py: &PyToken<'_>, bits: u64) -> bool {
    let obj = obj_from_bits(bits);
    obj.is_float()
        || (as_float_extended(obj).is_some()
            && builtin_float_bits_for_gil() == Some(type_of_bits(py, bits)))
}

/// int.__float__ owns the native payload conversion inherited by int subclasses.
/// Explicit base calls read that payload without redispatching __float__/__index__.
pub(crate) extern "C" fn int_float_slot(bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(value) = index_bigint_integral_bits(bits) else {
            return raise_exception::<_>(py, "TypeError", "int.__float__ requires an int");
        };
        let Some(value) = integer_as_double(py, &value) else {
            return crate::MoltObject::none().bits();
        };
        crate::object::ops::float_result_bits(py, value)
    })
}

/// Shared numeric protocol used by the float constructor after its exact-float
/// identity fast path. No text parsing and no instance-attribute lookup occurs.
/// A missing numeric protocol returns None without an exception, permitting
/// the constructor's text parser; every protocol failure returns None with its
/// original pending exception. Int subclasses must reach __float__ before any
/// integral payload extraction. Float subclasses likewise reach their slot here.
pub(crate) fn float_from_number_protocol(py: &PyToken<'_>, bits: u64) -> Option<f64> {
    let obj = obj_from_bits(bits);
    if is_exact_float(py, bits) {
        return as_float_extended(obj);
    }
    if let Some(value) = obj.as_int() {
        return Some(value as f64);
    }
    if let Some(value) = obj.as_bool() {
        return Some(if value { 1.0 } else { 0.0 });
    }
    if let Some(ptr) = bigint_ptr_from_bits(bits)
        && builtin_int_bits_for_gil() == Some(type_of_bits(py, bits))
    {
        return integer_as_double(py, unsafe { bigint_ref(ptr) });
    }
    if let Some(call_bits) =
        unsafe { crate::builtins::attr::lookup_special_method(py, bits, b"__float__") }
    {
        let result = unsafe { call_callable0(py, call_bits) };
        dec_ref_bits(py, call_bits);
        if exception_pending(py) {
            dec_ref_bits(py, result);
            return None;
        }
        let result_obj = obj_from_bits(result);
        let owner = class_name_for_error(type_of_bits(py, bits));
        if let Some(value) = as_float_extended(result_obj) {
            let accepted = is_exact_float(py, result)
                || warn_numeric_subclass_result(py, &format!("{owner}.__float__"), "float", result);
            // Keep the returned subclass owned through warning callbacks.
            dec_ref_bits(py, result);
            return accepted.then_some(value);
        }
        let actual = class_name_for_error(type_of_bits(py, result));
        raise_exception::<()>(
            py,
            "TypeError",
            &format!("{owner}.__float__ returned non-float (type {actual})"),
        );
        dec_ref_bits(py, result);
        return None;
    }
    if exception_pending(py) {
        return None;
    }
    // Inherited int.__float__ is a real native descriptor above. Only a
    // genuinely missing float slot reaches the canonical index protocol.
    let has_index = unsafe { crate::builtins::attr::has_special_method(py, bits, b"__index__") };
    if exception_pending(py) || !has_index {
        return None;
    }
    let message = format!(
        "must be real number, not {}",
        class_name_for_error(type_of_bits(py, bits))
    );
    let value = index_bigint_from_obj(py, bits, &message)?;
    integer_as_double(py, &value)
}

/// PyFloat_AsDouble-style numeric conversion. An existing float or float
/// subclass is read directly, ignoring an overridden __float__. Other values
/// use type-level __float__, then __index__; text is never parsed. None always
/// means a pending exception, including callback/warning errors and integer
/// overflow. Float infinity and NaN remain valid values.
pub(crate) fn float_as_double(py: &PyToken<'_>, bits: u64) -> Option<f64> {
    if let Some(value) = as_float_extended(obj_from_bits(bits)) {
        return Some(value);
    }
    if let Some(value) = float_from_number_protocol(py, bits) {
        return Some(value);
    }
    if !exception_pending(py) {
        raise_exception::<()>(
            py,
            "TypeError",
            &format!(
                "must be real number, not {}",
                class_name_for_error(type_of_bits(py, bits))
            ),
        );
    }
    None
}

#[inline]
pub(crate) fn to_f64(obj: MoltObject) -> Option<f64> {
    // Shared float storage projection includes managed scalar subclasses.
    if let Some(val) = as_float_extended(obj) {
        return Some(val);
    }
    if let Some(i) = to_i64(obj) {
        return Some(i as f64);
    }
    if let Some(ptr) = bigint_ptr_from_bits(obj.bits()) {
        return unsafe { bigint_ref(ptr) }.to_f64();
    }
    None
}

#[cfg(test)]
mod float_conversion_tests {
    use super::*;
    use crate::builtins::functions::{alloc_runtime_function_obj, runtime_fn_addr};
    use crate::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static FLOAT_CALLS: AtomicU64 = AtomicU64::new(0);
    static INDEX_CALLS: AtomicU64 = AtomicU64::new(0);
    static RESULT: AtomicU64 = AtomicU64::new(0);
    static RAISE: AtomicU64 = AtomicU64::new(0);

    extern "C" fn float_conversion_callback(_self: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            FLOAT_CALLS.fetch_add(1, Ordering::SeqCst);
            if RAISE.load(Ordering::SeqCst) != 0 {
                return raise_exception::<_>(py, "RuntimeError", "float protocol failure");
            }
            let result = RESULT.load(Ordering::SeqCst);
            inc_ref_bits(py, result);
            result
        })
    }

    extern "C" fn index_conversion_callback(_self: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            INDEX_CALLS.fetch_add(1, Ordering::SeqCst);
            if RAISE.load(Ordering::SeqCst) != 0 {
                return raise_exception::<_>(py, "LookupError", "index protocol failure");
            }
            let result = RESULT.load(Ordering::SeqCst);
            inc_ref_bits(py, result);
            result
        })
    }

    fn conversion_class(py: &PyToken<'_>, base: u64, float: bool, index: bool) -> u64 {
        let name = attr_name_bits_from_bytes(py, b"FloatConversionContract").unwrap();
        let class = crate::molt_class_new(name);
        dec_ref_bits(py, name);
        crate::molt_class_set_base(class, base);
        let methods: [(&[u8], bool, extern "C" fn(u64) -> u64, &str); 2] = [
            (
                b"__float__",
                float,
                float_conversion_callback,
                "float_conversion_callback",
            ),
            (
                b"__index__",
                index,
                index_conversion_callback,
                "index_conversion_callback",
            ),
        ];
        for (name, enabled, method, symbol) in methods {
            if !enabled {
                continue;
            }
            let name = attr_name_bits_from_bytes(py, name).unwrap();
            let function =
                alloc_runtime_function_obj(py, runtime_fn_addr(symbol, method as *const ()), 1);
            assert!(!function.is_null());
            let function = MoltObject::from_ptr(function).bits();
            crate::molt_set_attr_name(class, name, function);
            dec_ref_bits(py, name);
            dec_ref_bits(py, function);
        }
        unsafe {
            crate::object::class_finish_definition(py, obj_from_bits(class).as_ptr().unwrap())
                .unwrap();
        }
        assert!(!exception_pending(py));
        class
    }

    fn plain_instance(py: &PyToken<'_>, class: u64) -> u64 {
        let ptr = obj_from_bits(class).as_ptr().unwrap();
        let size = unsafe { crate::object::layout::class_cached_layout_size(ptr).unwrap() };
        let instance = crate::object::builders::alloc_class_instance(py, size, class);
        unsafe {
            crate::object::gc::gc_publish_initialized(
                py,
                obj_from_bits(instance).as_ptr().unwrap(),
            );
        }
        instance
    }

    fn reset_callbacks(result: u64, raise: bool) {
        RESULT.store(result, Ordering::SeqCst);
        RAISE.store(u64::from(raise), Ordering::SeqCst);
        FLOAT_CALLS.store(0, Ordering::SeqCst);
        INDEX_CALLS.store(0, Ordering::SeqCst);
    }

    fn assert_error(py: &PyToken<'_>, kind: &str, message: &str) {
        assert!(exception_pending(py));
        let error = crate::builtins::exceptions::molt_exception_last_pending();
        assert!(crate::builtins::exceptions::exception_matches_builtin_name(
            py, error, kind
        ));
        let text = crate::builtins::exceptions::format_exception_message(
            py,
            obj_from_bits(error).as_ptr().unwrap(),
        );
        assert_eq!(text, message);
        clear_exception(py);
        dec_ref_bits(py, error);
    }

    fn install_scalar_test_method(
        py: &PyToken<'_>,
        class: u64,
        name: &[u8],
        symbol: &str,
        callback: *const (),
        arity: u64,
    ) {
        let name = attr_name_bits_from_bytes(py, name).unwrap();
        let function = alloc_runtime_function_obj(py, runtime_fn_addr(symbol, callback), arity);
        assert!(!function.is_null());
        let function = MoltObject::from_ptr(function).bits();
        let result = crate::molt_set_attr_name(class, name, function);
        for value in [result, function, name] {
            dec_ref_bits(py, value);
        }
        assert!(!exception_pending(py));
    }

    extern "C" fn scalar_binary_override(value: u64, _other: u64) -> u64 {
        float_conversion_callback(value)
    }

    #[test]
    fn float_conversion_contract_typed_storage_and_exact_identity() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let parent = conversion_class(py, builtin_classes(py).float, true, false);
            let class = conversion_class(py, parent, false, false);
            for input in [
                0.0,
                -0.0,
                3.0,
                1.25,
                f64::from_bits(1),
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::NAN,
            ] {
                let carrier = crate::object::ops::float_result_bits(py, input);
                let value = crate::molt_float_new(class, carrier);
                assert!(!exception_pending(py));
                let object = obj_from_bits(value);
                assert_eq!(
                    scalar_value_bits(object, ScalarValueKind::Float),
                    Some(carrier)
                );
                assert_eq!(scalar_value_bits(object, ScalarValueKind::Int), None);
                assert!(is_float_extended(object));
                assert!(!is_exact_float(py, value));
                assert_eq!(
                    to_i64(object),
                    None,
                    "tagged float is not an integer carrier"
                );
                reset_callbacks(MoltObject::from_int(99).bits(), true);
                for actual in [
                    as_float_extended(object).unwrap(),
                    float_as_double(py, value).unwrap(),
                ] {
                    if input.is_nan() {
                        assert!(actual.is_nan());
                    } else {
                        assert_eq!(actual.to_bits(), input.to_bits());
                    }
                }
                assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 0);
                let hash = crate::object::ops_hash::molt_float_hash_method(value);
                assert!(!exception_pending(py));
                for value in [hash, value, carrier] {
                    dec_ref_bits(py, value);
                }
            }
            let integer_class = conversion_class(py, builtin_classes(py).int, false, false);
            let large = bigint_bits(py, BigInt::from(1u8) << 80usize);
            for payload in [MoltObject::from_int(42).bits(), large] {
                let value = crate::molt_int_new(integer_class, payload, missing_bits(py));
                assert_eq!(
                    scalar_value_bits(obj_from_bits(value), ScalarValueKind::Int),
                    Some(payload)
                );
                assert_eq!(
                    scalar_value_bits(obj_from_bits(value), ScalarValueKind::Float),
                    None
                );
                assert_eq!(as_float_extended(obj_from_bits(value)), None);
                assert_eq!(
                    to_bigint(obj_from_bits(value)),
                    to_bigint(obj_from_bits(payload))
                );
                dec_ref_bits(py, value);
            }
            reset_callbacks(0, false);
            for value in [large, integer_class, class, parent] {
                dec_ref_bits(py, value);
            }
        });
    }

    #[test]
    fn float_conversion_contract_ordinary_words_and_unsealed_classes_are_not_scalar_storage() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let name = attr_name_bits_from_bytes(py, b"ScalarStorageLookalike").unwrap();
            let class = crate::molt_class_new(name);
            let class_ptr = obj_from_bits(class).as_ptr().unwrap();
            assert_eq!(
                unsafe {
                    crate::object::class_layout::scalar_value_offset(
                        py,
                        class_ptr,
                        ScalarValueKind::Float,
                    )
                },
                None
            );
            assert_error(
                py,
                "SystemError",
                "scalar class has no matching intrinsic value word",
            );
            let base_result = crate::molt_class_set_base(class, builtin_classes(py).object);
            let slots = attr_name_bits_from_bytes(py, b"__slots__").unwrap();
            let field = attr_name_bits_from_bytes(py, b"payload").unwrap();
            let declaration = MoltObject::from_ptr(alloc_tuple(py, &[field])).bits();
            unsafe {
                let namespace = obj_from_bits(class_dict_bits(class_ptr)).as_ptr().unwrap();
                dict_set_in_place(py, namespace, slots, declaration);
                crate::object::class_finish_definition(py, class_ptr).unwrap();
                assert_eq!(
                    crate::object::class_layout::field_at_offset(class_ptr, 0)
                        .unwrap()
                        .kind,
                    crate::object::class_layout::ClassFieldKind::DeclaredSlot,
                );
            }
            let value = plain_instance(py, class);
            let assigned =
                crate::molt_set_attr_name(value, field, MoltObject::from_float(1.25).bits());
            assert!(!exception_pending(py));
            assert_eq!(
                scalar_value_bits(obj_from_bits(value), ScalarValueKind::Float),
                None
            );
            assert_eq!(
                scalar_value_bits(obj_from_bits(value), ScalarValueKind::Int),
                None
            );
            assert!(!is_float_extended(obj_from_bits(value)));
            assert_eq!(as_float_extended(obj_from_bits(value)), None);
            for value in [
                assigned,
                value,
                declaration,
                field,
                slots,
                base_result,
                class,
                name,
            ] {
                dec_ref_bits(py, value);
            }
        });
    }

    #[test]
    fn float_conversion_contract_unary_entrypoints_share_payload_and_override_policy() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let operations: [(extern "C" fn(u64) -> u64, bool); 4] = [
                (crate::molt_neg, true),
                (crate::molt_operator_neg, true),
                (crate::molt_pos, false),
                (crate::molt_operator_pos, false),
            ];
            let inherited = conversion_class(py, builtin_classes(py).float, false, false);
            for input in [
                0.0,
                -0.0,
                3.0,
                1.25,
                f64::from_bits(1),
                f64::INFINITY,
                f64::NAN,
            ] {
                let carrier = crate::object::ops::float_result_bits(py, input);
                let value = crate::molt_float_new(inherited, carrier);
                for receiver in [carrier, value] {
                    for (operation, negate) in operations {
                        let result = operation(receiver);
                        assert!(!exception_pending(py));
                        assert!(is_exact_float(py, result));
                        let actual = as_float_extended(obj_from_bits(result)).unwrap();
                        let expected = if negate { -input } else { input };
                        if expected.is_nan() {
                            assert!(actual.is_nan());
                        } else {
                            assert_eq!(actual.to_bits(), expected.to_bits());
                        }
                        dec_ref_bits(py, result);
                    }
                }
                dec_ref_bits(py, value);
                dec_ref_bits(py, carrier);
            }
            dec_ref_bits(py, inherited);
            for base in [builtin_classes(py).int, builtin_classes(py).float] {
                let class = conversion_class(py, base, false, false);
                for name in [b"__neg__".as_slice(), b"__pos__".as_slice()] {
                    install_scalar_test_method(
                        py,
                        class,
                        name,
                        "float_conversion_callback",
                        float_conversion_callback as *const (),
                        1,
                    );
                }
                let value = if base == builtin_classes(py).int {
                    crate::molt_int_new(class, MoltObject::from_int(42).bits(), missing_bits(py))
                } else {
                    crate::molt_float_new(class, MoltObject::from_float(3.0).bits())
                };
                for (operation, _) in operations {
                    reset_callbacks(MoltObject::from_int(99).bits(), false);
                    let result = operation(value);
                    assert_eq!(obj_from_bits(result).as_int(), Some(99));
                    assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 1);
                    dec_ref_bits(py, result);
                    reset_callbacks(MoltObject::none().bits(), true);
                    let result = operation(value);
                    assert_error(py, "RuntimeError", "float protocol failure");
                    assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 1);
                    dec_ref_bits(py, result);
                }
                dec_ref_bits(py, value);
                dec_ref_bits(py, class);
            }
            reset_callbacks(0, false);
        });
    }

    #[test]
    fn float_conversion_contract_sum_range_and_decimal_preserve_subtype_protocols() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let class = conversion_class(py, builtin_classes(py).float, false, false);
            let value = crate::molt_float_new(class, MoltObject::from_float(1.0).bits());
            for name in [
                b"__add__".as_slice(),
                b"__radd__".as_slice(),
                b"__eq__".as_slice(),
            ] {
                install_scalar_test_method(
                    py,
                    class,
                    name,
                    "scalar_binary_override",
                    scalar_binary_override as *const (),
                    2,
                );
            }
            let singleton = MoltObject::from_ptr(alloc_tuple(py, &[value])).bits();
            let plain =
                MoltObject::from_ptr(alloc_tuple(py, &[MoltObject::from_float(2.0).bits()])).bits();
            let empty = MoltObject::from_ptr(alloc_tuple(py, &[])).bits();
            for (items, start) in [
                (singleton, MoltObject::from_int(0).bits()),
                (singleton, MoltObject::from_float(2.0).bits()),
                (plain, value),
            ] {
                reset_callbacks(MoltObject::from_int(99).bits(), false);
                let result = crate::molt_sum_builtin(items, start);
                assert!(!exception_pending(py));
                assert_eq!(obj_from_bits(result).as_int(), Some(99));
                assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 1);
                dec_ref_bits(py, result);
            }
            reset_callbacks(MoltObject::from_int(99).bits(), false);
            let result = crate::molt_sum_builtin(empty, value);
            assert_eq!(result, value);
            assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 0);
            dec_ref_bits(py, result);

            let range = crate::molt_range_new(
                MoltObject::from_int(0).bits(),
                MoltObject::from_int(3).bits(),
                MoltObject::from_int(1).bits(),
            );
            reset_callbacks(MoltObject::from_bool(false).bits(), false);
            assert_eq!(
                obj_from_bits(crate::molt_contains(range, value)).as_bool(),
                Some(false)
            );
            assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 3);
            reset_callbacks(MoltObject::from_bool(true).bits(), false);
            assert_eq!(
                obj_from_bits(crate::molt_range_count(range, value)).as_int(),
                Some(3)
            );
            assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 3);
            reset_callbacks(MoltObject::from_bool(true).bits(), false);
            assert_eq!(
                obj_from_bits(crate::molt_range_index(range, value)).as_int(),
                Some(0)
            );
            assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 1);

            let format = attr_name_bits_from_bytes(py, b"%d").unwrap();
            let rendered = crate::molt_mod(format, value);
            assert_eq!(
                string_obj_to_owned(obj_from_bits(rendered)).as_deref(),
                Some("1")
            );
            dec_ref_bits(py, rendered);
            install_scalar_test_method(
                py,
                class,
                b"__int__",
                "float_conversion_callback",
                float_conversion_callback as *const (),
                1,
            );
            reset_callbacks(MoltObject::from_int(99).bits(), false);
            let rendered = crate::molt_mod(format, value);
            assert_eq!(
                string_obj_to_owned(obj_from_bits(rendered)).as_deref(),
                Some("99")
            );
            assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 1);
            assert!(!exception_pending(py));
            reset_callbacks(0, false);
            for value in [
                rendered, format, range, empty, plain, singleton, value, class,
            ] {
                dec_ref_bits(py, value);
            }
        });
    }

    #[test]
    fn float_conversion_contract_binary_subtype_dispatch_and_declined_slots() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            type BinaryOperation = (&'static [u8], &'static [u8], extern "C" fn(u64, u64) -> u64);
            let operations: [BinaryOperation; 8] = [
                (b"__add__", b"__radd__", crate::molt_add),
                (b"__sub__", b"__rsub__", crate::molt_sub),
                (b"__mul__", b"__rmul__", crate::molt_mul),
                (b"__truediv__", b"__rtruediv__", crate::molt_div),
                (b"__floordiv__", b"__rfloordiv__", crate::molt_floordiv),
                (b"__mod__", b"__rmod__", crate::molt_mod),
                (b"__divmod__", b"__rdivmod__", crate::molt_divmod_builtin),
                (b"__pow__", b"__rpow__", crate::molt_pow),
            ];
            let base = conversion_class(py, builtin_classes(py).float, false, false);
            let base_value = crate::molt_float_new(base, MoltObject::from_float(3.0).bits());
            let heap_value = crate::object::ops::alloc_heap_float(py, 3.0);
            for (forward, reflected, operation) in operations {
                let class = conversion_class(py, base, false, false);
                let value = crate::molt_float_new(class, MoltObject::from_float(2.0).bits());
                let inherited = operation(base_value, value);
                assert!(
                    !exception_pending(py),
                    "inherited float slot must terminate"
                );
                dec_ref_bits(py, inherited);
                let native = operation(heap_value, MoltObject::from_int(2).bits());
                assert!(!exception_pending(py));
                if forward == b"__divmod__" {
                    unsafe {
                        crate::object::seq_access::with_immutable_tuple_slice(
                            obj_from_bits(native).as_ptr().unwrap(),
                            |items| assert!(items.iter().all(|bits| is_exact_float(py, *bits))),
                        )
                        .unwrap();
                    }
                } else {
                    assert!(
                        is_exact_float(py, native),
                        "integral heap float remains float arithmetic"
                    );
                }
                dec_ref_bits(py, native);
                install_scalar_test_method(
                    py,
                    class,
                    forward,
                    "scalar_binary_override",
                    scalar_binary_override as *const (),
                    2,
                );
                install_scalar_test_method(
                    py,
                    class,
                    reflected,
                    "scalar_binary_override",
                    scalar_binary_override as *const (),
                    2,
                );
                if forward == b"__add__" {
                    reset_callbacks(MoltObject::from_int(99).bits(), false);
                    let result = crate::object::ops_arith::native_slots::float_add_slot(
                        value,
                        MoltObject::from_int(1).bits(),
                    );
                    assert!(is_exact_float(py, result));
                    assert_eq!(as_float_extended(obj_from_bits(result)), Some(3.0));
                    assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 0);
                    dec_ref_bits(py, result);
                }
                // Covers the direct slot, unrelated-type reflection, and the
                // derived reflected slot's priority over an inherited base slot.
                for (left, right) in [
                    (value, MoltObject::from_int(1).bits()),
                    (MoltObject::from_int(1).bits(), value),
                    (base_value, value),
                ] {
                    reset_callbacks(MoltObject::from_int(99).bits(), false);
                    let result = operation(left, right);
                    assert_eq!(obj_from_bits(result).as_int(), Some(99));
                    assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 1);
                    assert!(!exception_pending(py));
                    dec_ref_bits(py, result);
                }
                reset_callbacks(not_implemented_bits(py), false);
                let result = operation(value, value);
                assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 1);
                assert!(
                    exception_pending(py),
                    "declined same-type slot cannot read payload instead"
                );
                let error = crate::builtins::exceptions::molt_exception_last_pending();
                assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                    py,
                    error,
                    "TypeError"
                ));
                clear_exception(py);
                for value in [error, result, value, class] {
                    dec_ref_bits(py, value);
                }
            }
            for (name, operation) in [
                (
                    b"__isub__".as_slice(),
                    crate::molt_inplace_sub as extern "C" fn(u64, u64) -> u64,
                ),
                (
                    b"__imul__".as_slice(),
                    crate::molt_inplace_mul as extern "C" fn(u64, u64) -> u64,
                ),
            ] {
                install_scalar_test_method(
                    py,
                    base,
                    name,
                    "scalar_binary_override",
                    scalar_binary_override as *const (),
                    2,
                );
                reset_callbacks(MoltObject::from_int(99).bits(), false);
                let result = operation(base_value, MoltObject::from_int(1).bits());
                assert_eq!(obj_from_bits(result).as_int(), Some(99));
                assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 1);
                assert!(!exception_pending(py));
                dec_ref_bits(py, result);
            }
            reset_callbacks(0, false);
            dec_ref_bits(py, heap_value);
            dec_ref_bits(py, base_value);
            dec_ref_bits(py, base);
        });
    }

    #[test]
    fn float_conversion_contract_integer_admission_is_independent_of_float_carrier() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let class = conversion_class(py, builtin_classes(py).float, false, false);
            let inline = MoltObject::from_float(3.0).bits();
            let heap = crate::object::ops::alloc_heap_float(py, 3.0);
            let tagged = crate::molt_float_new(class, inline);
            for value in [inline, heap, tagged] {
                assert_eq!(index_i64_integral_bits(value), None);
                assert_eq!(index_bigint_integral_bits(value), None);
                let result = crate::molt_inplace_floordiv(value, MoltObject::from_int(2).bits());
                assert!(is_exact_float(py, result));
                assert_eq!(as_float_extended(obj_from_bits(result)), Some(1.0));
                dec_ref_bits(py, result);
                for operation in [crate::molt_floordiv, crate::molt_inplace_floordiv] {
                    let failed = operation(value, MoltObject::from_int(0).bits());
                    assert_error(py, "ZeroDivisionError", "float floor division by zero");
                    dec_ref_bits(py, failed);
                }
                let absolute = crate::molt_abs_builtin(value);
                assert!(is_exact_float(py, absolute));
                dec_ref_bits(py, absolute);
                let rounded = crate::molt_round(
                    value,
                    MoltObject::from_int(0).bits(),
                    MoltObject::from_int(1).bits(),
                );
                assert!(is_exact_float(py, rounded));
                dec_ref_bits(py, rounded);
                let complex = complex_from_obj_strict(py, obj_from_bits(value))
                    .unwrap()
                    .unwrap();
                assert_eq!(complex.re, 3.0);
                for operation in [crate::molt_bit_or, crate::molt_bit_and, crate::molt_bit_xor] {
                    let failed = operation(value, MoltObject::from_int(1).bits());
                    assert!(exception_pending(py));
                    let error = crate::builtins::exceptions::molt_exception_last_pending();
                    assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                        py,
                        error,
                        "TypeError"
                    ));
                    clear_exception(py);
                    dec_ref_bits(py, error);
                    dec_ref_bits(py, failed);
                }
            }
            install_scalar_test_method(
                py,
                class,
                b"__floordiv__",
                "scalar_binary_override",
                scalar_binary_override as *const (),
                2,
            );
            reset_callbacks(not_implemented_bits(py), false);
            let failed = crate::molt_inplace_floordiv(tagged, tagged);
            assert_error(
                py,
                "TypeError",
                "unsupported operand type(s) for //=: 'FloatConversionContract' and 'FloatConversionContract'",
            );
            assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 1);
            reset_callbacks(0, false);
            for value in [failed, tagged, heap, class] {
                dec_ref_bits(py, value);
            }
        });
    }

    #[test]
    fn float_conversion_contract_percent_integer_results_reject_all_float_carriers() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let float_class = conversion_class(py, builtin_classes(py).float, false, false);
            let inline = MoltObject::from_float(1.0).bits();
            let heap = crate::object::ops::alloc_heap_float(py, 1.0);
            let tagged = crate::molt_float_new(float_class, inline);
            install_scalar_test_method(
                py,
                float_class,
                b"__int__",
                "float_conversion_callback",
                float_conversion_callback as *const (),
                1,
            );
            let index_class = conversion_class(py, builtin_classes(py).object, false, true);
            let index_value = plain_instance(py, index_class);
            for (receiver, format) in [
                (tagged, b"%d".as_slice()),
                (index_value, b"%d".as_slice()),
                (index_value, b"%x".as_slice()),
                (index_value, b"%c".as_slice()),
            ] {
                let format = attr_name_bits_from_bytes(py, format).unwrap();
                for returned in [inline, heap, tagged] {
                    reset_callbacks(returned, false);
                    let failed = crate::molt_mod(format, receiver);
                    assert!(exception_pending(py));
                    let error = crate::builtins::exceptions::molt_exception_last_pending();
                    assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                        py,
                        error,
                        "TypeError"
                    ));
                    assert_eq!(
                        FLOAT_CALLS.load(Ordering::SeqCst) + INDEX_CALLS.load(Ordering::SeqCst),
                        1
                    );
                    clear_exception(py);
                    dec_ref_bits(py, error);
                    dec_ref_bits(py, failed);
                }
                dec_ref_bits(py, format);
            }
            reset_callbacks(0, false);
            for value in [index_value, index_class, tagged, heap, float_class] {
                dec_ref_bits(py, value);
            }
        });
    }

    #[test]
    fn inherited_float_descriptors_read_the_payload_without_conversion_callbacks() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let class = conversion_class(py, builtin_classes(py).float, true, false);
            for (input, integer, hex) in [
                (0.0, true, "0x0.0p+0"),
                (-0.0, true, "-0x0.0p+0"),
                (1.25, false, "0x1.4000000000000p+0"),
                (f64::from_bits(1), false, "0x0.0000000000001p-1022"),
                (f64::INFINITY, false, "inf"),
                (f64::NAN, false, "nan"),
            ] {
                let input_bits = crate::object::ops::float_result_bits(py, input);
                let value = crate::molt_float_new(class, input_bits);
                assert!(!exception_pending(py));
                reset_callbacks(MoltObject::from_int(99).bits(), true);
                for descriptor in [crate::molt_float_float, crate::molt_float_conjugate] {
                    let result = descriptor(value);
                    let actual = as_float_extended(obj_from_bits(result)).unwrap();
                    assert!(is_exact_float(py, result));
                    if input.is_nan() {
                        assert!(actual.is_nan());
                    } else {
                        assert_eq!(actual.to_bits(), input.to_bits());
                    }
                    dec_ref_bits(py, result);
                }
                assert_eq!(
                    crate::molt_float_is_integer(value),
                    MoltObject::from_bool(integer).bits(),
                );
                let rendered = crate::molt_float_hex(value);
                assert_eq!(
                    string_obj_to_owned(obj_from_bits(rendered)).as_deref(),
                    Some(hex)
                );
                dec_ref_bits(py, rendered);
                let ratio = crate::molt_float_as_integer_ratio(value);
                if input.is_nan() {
                    assert_error(py, "ValueError", "cannot convert NaN to integer ratio");
                } else if input.is_infinite() {
                    assert_error(
                        py,
                        "OverflowError",
                        "cannot convert Infinity to integer ratio",
                    );
                } else if input == 0.0 || input == 1.25 {
                    let expected = if input == 0.0 { [0, 1] } else { [5, 4] };
                    let ptr = obj_from_bits(ratio).as_ptr().unwrap();
                    let actual = unsafe {
                        crate::object::seq_access::with_immutable_tuple_slice(ptr, |items| {
                            items
                                .iter()
                                .map(|bits| obj_from_bits(*bits).as_int().unwrap())
                                .collect::<Vec<_>>()
                        })
                    }
                    .unwrap();
                    assert_eq!(actual, expected);
                }
                dec_ref_bits(py, ratio);
                assert!(!exception_pending(py));
                assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 0);
                dec_ref_bits(py, value);
                dec_ref_bits(py, input_bits);
            }
            // A conversion protocol alone never admits an unrelated receiver
            // to the float descriptor's storage contract.
            let unrelated = conversion_class(py, builtin_classes(py).object, true, false);
            let value = plain_instance(py, unrelated);
            reset_callbacks(MoltObject::from_float(1.25).bits(), false);
            for descriptor in [
                crate::molt_float_float,
                crate::molt_float_conjugate,
                crate::molt_float_is_integer,
                crate::molt_float_as_integer_ratio,
                crate::molt_float_hex,
            ] {
                let result = descriptor(value);
                assert!(exception_pending(py));
                let error = crate::builtins::exceptions::molt_exception_last_pending();
                assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                    py,
                    error,
                    "TypeError"
                ));
                clear_exception(py);
                dec_ref_bits(py, error);
                dec_ref_bits(py, result);
            }
            assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 0);
            reset_callbacks(0, false);
            for value in [value, unrelated, class] {
                dec_ref_bits(py, value);
            }
        });
    }

    fn assert_scalar_rendered_text(py: &PyToken<'_>, rendered: u64, expected: &str) {
        assert!(!exception_pending(py));
        assert_eq!(
            string_obj_to_owned(obj_from_bits(rendered)).as_deref(),
            Some(expected),
        );
        dec_ref_bits(py, rendered);
    }

    #[test]
    fn float_conversion_contract_integral_projection_borrows_one_validated_carrier() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let class = conversion_class(py, builtin_classes(py).int, true, true);
            let small = MoltObject::from_int(42).bits();
            let boolean = MoltObject::from_bool(true).bits();
            let large = bigint_bits(py, BigInt::from(1u8) << 80usize);
            for bits in [small, boolean, large] {
                assert_eq!(index_integral_payload_bits(bits), Some(bits));
            }
            assert_eq!(index_i64_integral_bits(boolean), Some(1));
            assert_eq!(index_bigint_integral_bits(boolean), Some(BigInt::from(1u8)));
            let wrapped = crate::molt_int_new(class, large, missing_bits(py));
            let nested = crate::molt_int_new(class, small, missing_bits(py));
            let heap_float = crate::object::ops::alloc_heap_float(py, 1.0);
            reset_callbacks(0, true);
            assert_eq!(index_integral_payload_bits(wrapped), Some(large));
            assert_eq!(index_i64_integral_bits(wrapped), None);
            assert_eq!(
                index_bigint_integral_bits(wrapped),
                Some(BigInt::from(1u8) << 80usize)
            );
            assert_eq!(index_integral_payload_bits(nested), Some(small));
            assert_eq!(index_i64_integral_bits(nested), Some(42));
            let offset = unsafe {
                crate::object::class_layout::scalar_value_offset(
                    py,
                    obj_from_bits(class).as_ptr().unwrap(),
                    ScalarValueKind::Int,
                )
            }
            .unwrap();
            let ptr = obj_from_bits(wrapped).as_ptr().unwrap();
            for invalid in [
                MoltObject::from_float(1.0).bits(),
                heap_float,
                nested,
                MoltObject::none().bits(),
            ] {
                let assigned = unsafe {
                    crate::object::accessors::object_field_set_ptr_raw(py, ptr, offset, invalid)
                };
                assert!(!exception_pending(py));
                assert_eq!(
                    scalar_value_bits(obj_from_bits(wrapped), ScalarValueKind::Int),
                    Some(invalid)
                );
                assert_eq!(index_integral_payload_bits(wrapped), None);
                assert_eq!(index_i64_integral_bits(wrapped), None);
                assert_eq!(index_bigint_integral_bits(wrapped), None);
                dec_ref_bits(py, assigned);
            }
            let assigned = unsafe {
                crate::object::accessors::object_field_set_ptr_raw(py, ptr, offset, large)
            };
            assert_eq!(index_integral_payload_bits(wrapped), Some(large));
            assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 0);
            assert_eq!(INDEX_CALLS.load(Ordering::SeqCst), 0);
            assert!(!exception_pending(py));
            reset_callbacks(0, false);
            for value in [assigned, wrapped, nested, heap_float, large, class] {
                dec_ref_bits(py, value);
            }
        });
    }

    #[test]
    fn float_conversion_contract_integer_formatting_shares_payload_admission() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let class = conversion_class(py, builtin_classes(py).int, true, true);
            let small = MoltObject::from_int(42).bits();
            let large = bigint_bits(py, BigInt::from(1u8) << 80usize);
            let renderers: [extern "C" fn(u64, u64) -> u64; 2] =
                [crate::molt_string_format, crate::molt_format_builtin];
            for (payload, spec, expected) in [
                (small, "d", "42"),
                (small, "b", "101010"),
                (small, "o", "52"),
                (small, "x", "2a"),
                (small, "X", "2A"),
                (small, "c", "*"),
                (small, "n", "42"),
                (small, ">6", "    42"),
                (large, "d", "1208925819614629174706176"),
                (large, "x", "100000000000000000000"),
                (large, ">26", " 1208925819614629174706176"),
            ] {
                let value = crate::molt_int_new(class, payload, missing_bits(py));
                let spec = attr_name_bits_from_bytes(py, spec.as_bytes()).unwrap();
                for receiver in [payload, value] {
                    for render in renderers {
                        reset_callbacks(0, true);
                        assert_scalar_rendered_text(py, render(receiver, spec), expected);
                        assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 0);
                        assert_eq!(INDEX_CALLS.load(Ordering::SeqCst), 0);
                    }
                }
                dec_ref_bits(py, spec);
                dec_ref_bits(py, value);
            }
            // A conversion protocol alone does not admit arbitrary objects to
            // a native numeric __format__ implementation.
            reset_callbacks(0, false);
            let unrelated = conversion_class(py, builtin_classes(py).object, true, false);
            let value = plain_instance(py, unrelated);
            let spec = attr_name_bits_from_bytes(py, b".2f").unwrap();
            for render in renderers {
                reset_callbacks(MoltObject::from_float(2.5).bits(), false);
                let failed = render(value, spec);
                assert_error(
                    py,
                    "TypeError",
                    "unsupported format string passed to FloatConversionContract.__format__",
                );
                assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 0);
                dec_ref_bits(py, failed);
            }
            reset_callbacks(0, false);
            for value in [spec, value, unrelated, large, class] {
                dec_ref_bits(py, value);
            }
        });
    }

    #[test]
    fn float_conversion_contract_scalar_default_formatting_preserves_overrides() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let int_class = conversion_class(py, builtin_classes(py).int, true, true);
            let float_class = conversion_class(py, builtin_classes(py).float, true, true);
            let large = bigint_bits(py, BigInt::from(1u8) << 80usize);
            let integer = crate::molt_int_new(int_class, large, missing_bits(py));
            let floating = crate::molt_float_new(float_class, MoltObject::from_float(-0.0).bits());
            let empty = attr_name_bits_from_bytes(py, b"").unwrap();
            let custom = attr_name_bits_from_bytes(py, b"scalar-rendered").unwrap();
            let unary: [extern "C" fn(u64) -> u64; 2] =
                [crate::molt_repr_builtin, crate::molt_str_from_obj];
            let renderers: [extern "C" fn(u64, u64) -> u64; 2] =
                [crate::molt_string_format, crate::molt_format_builtin];
            for (class, value, spec, expected) in [
                (int_class, integer, "d", "1208925819614629174706176"),
                (float_class, floating, ".1f", "-0.0"),
            ] {
                for render in unary {
                    reset_callbacks(0, true);
                    assert_scalar_rendered_text(py, render(value), expected);
                    assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 0);
                    assert_eq!(INDEX_CALLS.load(Ordering::SeqCst), 0);
                }
                for render in renderers {
                    reset_callbacks(0, true);
                    assert_scalar_rendered_text(py, render(value, empty), expected);
                    assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 0);
                    assert_eq!(INDEX_CALLS.load(Ordering::SeqCst), 0);
                }
                if class == float_class {
                    for code in ["d", "x"] {
                        let spec = attr_name_bits_from_bytes(py, code.as_bytes()).unwrap();
                        for render in renderers {
                            reset_callbacks(0, true);
                            let failed = render(value, spec);
                            let message = format!(
                                "Unknown format code '{code}' for object of type 'FloatConversionContract'"
                            );
                            assert_error(py, "ValueError", &message);
                            assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 0);
                            assert_eq!(INDEX_CALLS.load(Ordering::SeqCst), 0);
                            dec_ref_bits(py, failed);
                        }
                        dec_ref_bits(py, spec);
                    }
                }
                install_scalar_test_method(
                    py,
                    class,
                    b"__repr__",
                    "float_conversion_callback",
                    float_conversion_callback as *const (),
                    1,
                );
                for render in unary {
                    reset_callbacks(custom, false);
                    assert_scalar_rendered_text(py, render(value), "scalar-rendered");
                    assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 1);
                }
                for render in renderers {
                    reset_callbacks(custom, false);
                    assert_scalar_rendered_text(py, render(value, empty), "scalar-rendered");
                    assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 1);
                }
                // Generic formatting consults __format__. Its declaring base
                // implementation keeps the payload receiver for nonempty specs.
                install_scalar_test_method(
                    py,
                    class,
                    b"__format__",
                    "scalar_binary_override",
                    scalar_binary_override as *const (),
                    2,
                );
                let spec = attr_name_bits_from_bytes(py, spec.as_bytes()).unwrap();
                reset_callbacks(custom, false);
                assert_scalar_rendered_text(
                    py,
                    crate::molt_format_builtin(value, spec),
                    "scalar-rendered",
                );
                assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 1);
                reset_callbacks(0, true);
                assert_scalar_rendered_text(py, crate::molt_string_format(value, spec), expected);
                assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 0);
                assert_eq!(INDEX_CALLS.load(Ordering::SeqCst), 0);
                dec_ref_bits(py, spec);
                install_scalar_test_method(
                    py,
                    class,
                    b"__str__",
                    "index_conversion_callback",
                    index_conversion_callback as *const (),
                    1,
                );
                reset_callbacks(custom, false);
                assert_scalar_rendered_text(
                    py,
                    crate::molt_string_format(value, empty),
                    "scalar-rendered",
                );
                assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 0);
                assert_eq!(INDEX_CALLS.load(Ordering::SeqCst), 1);
                reset_callbacks(0, true);
                let failed = crate::molt_string_format(value, empty);
                assert_error(py, "LookupError", "index protocol failure");
                assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 0);
                assert_eq!(INDEX_CALLS.load(Ordering::SeqCst), 1);
                dec_ref_bits(py, failed);
            }
            reset_callbacks(0, false);
            for value in [
                custom,
                empty,
                floating,
                integer,
                large,
                float_class,
                int_class,
            ] {
                dec_ref_bits(py, value);
            }
        });
    }

    #[test]
    fn float_conversion_contract_format_slots_and_pending_errors() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let class = conversion_class(py, builtin_classes(py).int, true, true);
            let huge = bigint_bits(py, BigInt::from(1u8) << 4096usize);
            let spec = attr_name_bits_from_bytes(py, b".2f").unwrap();
            let percent = attr_name_bits_from_bytes(py, b"%.2f").unwrap();
            for payload in [MoltObject::from_int(42).bits(), huge] {
                let value = crate::molt_int_new(class, payload, missing_bits(py));
                assert_eq!(
                    scalar_value_bits(obj_from_bits(value), ScalarValueKind::Int),
                    Some(payload)
                );
                assert_eq!(type_of_bits(py, value), class);
                let renderers: [extern "C" fn(u64, u64) -> u64; 2] =
                    [crate::molt_string_format, crate::molt_format_builtin];
                for render in renderers {
                    reset_callbacks(MoltObject::from_float(2.5).bits(), false);
                    let rendered = render(value, spec);
                    assert_eq!(
                        string_obj_to_owned(obj_from_bits(rendered)).as_deref(),
                        Some("2.50")
                    );
                    assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 1);
                    assert_eq!(INDEX_CALLS.load(Ordering::SeqCst), 0);
                    dec_ref_bits(py, rendered);
                    reset_callbacks(MoltObject::from_float(2.5).bits(), true);
                    let failed = render(value, spec);
                    assert_error(py, "RuntimeError", "float protocol failure");
                    assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 1);
                    assert_eq!(INDEX_CALLS.load(Ordering::SeqCst), 0);
                    dec_ref_bits(py, failed);
                }
                reset_callbacks(MoltObject::from_float(2.5).bits(), false);
                let rendered = crate::molt_mod(percent, value);
                assert_eq!(
                    string_obj_to_owned(obj_from_bits(rendered)).as_deref(),
                    Some("2.50")
                );
                assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 1);
                dec_ref_bits(py, rendered);
                dec_ref_bits(py, value);
            }
            let inherited = conversion_class(py, builtin_classes(py).int, false, true);
            let value =
                crate::molt_int_new(inherited, MoltObject::from_int(42).bits(), missing_bits(py));
            reset_callbacks(MoltObject::from_int(99).bits(), true);
            let method =
                unsafe { crate::builtins::attr::lookup_special_method(py, value, b"__float__") }
                    .unwrap();
            let converted = unsafe { call_callable0(py, method) };
            assert_eq!(as_float_extended(obj_from_bits(converted)), Some(42.0));
            assert!(is_exact_float(py, converted));
            assert!(!exception_pending(py));
            assert_eq!(INDEX_CALLS.load(Ordering::SeqCst), 0);
            for value in [
                converted, method, value, inherited, huge, class, spec, percent,
            ] {
                dec_ref_bits(py, value);
            }
        });
    }

    #[test]
    fn float_conversion_contract_c_api_managed_subclass_identity_and_policy() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::cpython_abi_hooks::register_cpython_hooks();
        crate::with_gil_entry_nopanic!(py, {
            use molt_cpython_abi::{
                OwnedHandleResult,
                api::{abstract_number, errors, numbers, refcount},
                bridge::GLOBAL_BRIDGE,
            };
            for base in [builtin_classes(py).int, builtin_classes(py).float] {
                let class = conversion_class(py, base, true, true);
                let value = if base == builtin_classes(py).int {
                    crate::molt_int_new(class, MoltObject::from_int(42).bits(), missing_bits(py))
                } else {
                    crate::molt_float_new(class, MoltObject::from_float(1.25).bits())
                };
                inc_ref_bits(py, value);
                let op =
                    unsafe { GLOBAL_BRIDGE.owned_result_to_pyobj(OwnedHandleResult::ok(value)) };
                assert!(!op.is_null());
                reset_callbacks(MoltObject::from_float(6.25).bits(), false);
                let result = unsafe { abstract_number::PyNumber_Float(op) };
                assert!(!result.is_null());
                assert_eq!(unsafe { numbers::PyFloat_AsDouble(result) }, 6.25);
                assert_eq!(unsafe { numbers::PyFloat_CheckExact(result) }, 1);
                assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 1);
                unsafe { refcount::Py_DECREF(result) };
                reset_callbacks(MoltObject::from_float(6.25).bits(), false);
                let expected = if base == builtin_classes(py).float {
                    1.25
                } else {
                    6.25
                };
                assert_eq!(unsafe { numbers::PyFloat_AsDouble(op) }, expected);
                assert_eq!(
                    FLOAT_CALLS.load(Ordering::SeqCst),
                    u64::from(base != builtin_classes(py).float)
                );
                assert_eq!(INDEX_CALLS.load(Ordering::SeqCst), 0);
                assert!(unsafe { errors::PyErr_Occurred() }.is_null());
                unsafe { refcount::Py_DECREF(op) };
                dec_ref_bits(py, value);
                dec_ref_bits(py, class);
            }
            let exact = crate::object::ops::float_result_bits(py, f64::NAN);
            let copied = crate::molt_float_from_obj(exact);
            assert_eq!(copied, exact);
            let subclass = conversion_class(py, builtin_classes(py).float, false, false);
            let value = crate::molt_float_new(subclass, exact);
            let copied_subclass = crate::molt_float_float(value);
            assert!(
                as_float_extended(obj_from_bits(copied_subclass))
                    .unwrap()
                    .is_nan()
            );
            assert!(is_exact_float(py, copied_subclass));
            assert_ne!(copied_subclass, value);
            for value in [copied_subclass, value, subclass, copied, exact] {
                dec_ref_bits(py, value);
            }
        });
    }

    static NATIVE_FLOAT_FAILURE: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);

    unsafe extern "C" fn native_float_failure(
        _op: *mut molt_cpython_abi::abi_types::PyObject,
    ) -> *mut molt_cpython_abi::abi_types::PyObject {
        let error = NATIVE_FLOAT_FAILURE.load(Ordering::SeqCst)
            as *mut molt_cpython_abi::abi_types::PyObject;
        unsafe {
            molt_cpython_abi::api::refcount::Py_INCREF(error);
            molt_cpython_abi::api::errors::PyErr_SetRaisedException(error);
        }
        std::ptr::null_mut()
    }

    unsafe extern "C" fn native_index_forbidden(
        _op: *mut molt_cpython_abi::abi_types::PyObject,
    ) -> *mut molt_cpython_abi::abi_types::PyObject {
        INDEX_CALLS.fetch_add(1, Ordering::SeqCst);
        std::ptr::null_mut()
    }

    #[test]
    fn float_conversion_contract_c_api_foreign_callback_error_identity() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::cpython_abi_hooks::register_cpython_hooks();
        crate::with_gil_entry_nopanic!(_py, {
            use molt_cpython_abi::{
                abi_types::*,
                api::{abstract_number, errors, numbers, refcount},
            };
            unsafe {
                errors::PyErr_SetString(
                    (&raw mut PyExc_TypeError).cast(),
                    c"preserve native float failure".as_ptr(),
                );
                let failure = errors::PyErr_GetRaisedException();
                assert!(!failure.is_null());
                NATIVE_FLOAT_FAILURE.store(failure as usize, Ordering::SeqCst);
                INDEX_CALLS.store(0, Ordering::SeqCst);
                let mut methods: PyNumberMethods = std::mem::zeroed();
                methods.nb_float = native_float_failure as *const () as *mut std::ffi::c_void;
                methods.nb_index = native_index_forbidden as *const () as *mut std::ffi::c_void;
                let mut ty: PyTypeObject = std::mem::zeroed();
                ty.ob_base.ob_base.ob_refcnt = IMMORTAL_REFCNT;
                ty.ob_base.ob_base.ob_type = &raw mut PyType_Type;
                ty.tp_name = c"NativeFloatFailure".as_ptr();
                ty.tp_as_number = (&mut methods as *mut PyNumberMethods).cast();
                let mut op = PyObject {
                    ob_refcnt: 1,
                    ob_type: &mut ty,
                };
                assert_eq!(numbers::PyFloat_AsDouble(&mut op), -1.0);
                let raised = errors::PyErr_GetRaisedException();
                assert_eq!(raised, failure);
                refcount::Py_DECREF(raised);
                assert!(abstract_number::PyNumber_Float(&mut op).is_null());
                let raised = errors::PyErr_GetRaisedException();
                assert_eq!(raised, failure);
                refcount::Py_DECREF(raised);
                assert_eq!(INDEX_CALLS.load(Ordering::SeqCst), 0);
                NATIVE_FLOAT_FAILURE.store(0, Ordering::SeqCst);
                refcount::Py_DECREF(failure);
            }
        });
    }

    #[test]
    fn float_conversion_contract_mixed_arithmetic_preserves_overflow() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let operations: &[extern "C" fn(u64, u64) -> u64] = &[
                crate::molt_add,
                crate::molt_sub,
                crate::molt_inplace_sub,
                crate::molt_mul,
                crate::molt_inplace_mul,
                crate::molt_div,
                crate::molt_floordiv,
                crate::molt_mod,
                crate::molt_divmod_builtin,
            ];
            for sign in [1, -1] {
                let huge = bigint_bits(py, BigInt::from(sign) << 4096usize);
                for number in [1.5, 0.0, f64::INFINITY, f64::NAN] {
                    let float = crate::object::ops::float_result_bits(py, number);
                    for operation in operations {
                        for (lhs, rhs) in [(huge, float), (float, huge)] {
                            let result = operation(lhs, rhs);
                            assert_error(py, "OverflowError", "int too large to convert to float");
                            dec_ref_bits(py, result);
                        }
                    }
                    dec_ref_bits(py, float);
                }
                dec_ref_bits(py, huge);
            }
            let finite = bigint_bits(py, BigInt::from(10u8).pow(300));
            for number in [1.5, f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
                let float = crate::object::ops::float_result_bits(py, number);
                for (lhs, rhs) in [(finite, float), (float, finite)] {
                    let result = crate::molt_add(lhs, rhs);
                    assert!(!exception_pending(py));
                    let value = to_f64(obj_from_bits(result)).unwrap();
                    assert!(value == number + 1e300 || (value.is_nan() && number.is_nan()));
                    dec_ref_bits(py, result);
                }
                dec_ref_bits(py, float);
            }
            dec_ref_bits(py, finite);
        });
    }

    #[test]
    fn float_conversion_contract_constructor_and_as_double_subclass_policies() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let class = conversion_class(py, builtin_classes(py).float, true, false);
            let value = crate::molt_float_new(class, MoltObject::from_float(1.25).bits());
            assert!(!exception_pending(py));
            reset_callbacks(MoltObject::from_float(2.5).bits(), false);
            assert_eq!(float_as_double(py, value), Some(1.25));
            assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 0);
            let constructed = crate::molt_float_from_obj(value);
            assert_eq!(as_float_extended(obj_from_bits(constructed)), Some(2.5));
            assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 1);
            let from_number = crate::molt_float_from_number(builtin_classes(py).float, value);
            assert_eq!(as_float_extended(obj_from_bits(from_number)), Some(1.25));
            assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 1);
            for bits in [constructed, from_number, value, class] {
                dec_ref_bits(py, bits);
            }

            let class = conversion_class(py, builtin_classes(py).int, true, true);
            let value =
                crate::molt_int_new(class, MoltObject::from_int(1).bits(), missing_bits(py));
            assert!(!exception_pending(py));
            reset_callbacks(MoltObject::from_float(3.5).bits(), false);
            assert_eq!(float_as_double(py, value), Some(3.5));
            assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 1);
            assert_eq!(INDEX_CALLS.load(Ordering::SeqCst), 0);
            dec_ref_bits(py, value);
            dec_ref_bits(py, class);
        });
    }

    #[test]
    fn float_conversion_contract_protocol_order_errors_and_integer_overflow() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let class = conversion_class(py, builtin_classes(py).object, true, true);
            let value = plain_instance(py, class);
            reset_callbacks(MoltObject::from_int(4).bits(), true);
            assert_eq!(float_as_double(py, value), None);
            assert_error(py, "RuntimeError", "float protocol failure");
            assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 1);
            assert_eq!(INDEX_CALLS.load(Ordering::SeqCst), 0);
            reset_callbacks(MoltObject::from_int(4).bits(), false);
            assert_eq!(float_as_double(py, value), None);
            assert_error(
                py,
                "TypeError",
                "FloatConversionContract.__float__ returned non-float (type int)",
            );
            assert_eq!(INDEX_CALLS.load(Ordering::SeqCst), 0);
            dec_ref_bits(py, value);
            dec_ref_bits(py, class);

            let class = conversion_class(py, builtin_classes(py).object, false, true);
            let value = plain_instance(py, class);
            reset_callbacks(MoltObject::from_float(1.5).bits(), false);
            assert_eq!(float_as_double(py, value), None);
            assert_error(py, "TypeError", "__index__ returned non-int (type float)");
            reset_callbacks(MoltObject::from_int(4).bits(), true);
            assert_eq!(float_as_double(py, value), None);
            assert_error(py, "LookupError", "index protocol failure");
            let huge = bigint_bits(py, BigInt::from(1u8) << 4096usize);
            reset_callbacks(huge, false);
            for input in [huge, value] {
                assert_eq!(float_as_double(py, input), None);
                assert_error(py, "OverflowError", "int too large to convert to float");
            }
            for bits in [huge, value, class] {
                dec_ref_bits(py, bits);
            }
            for number in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
                let value = crate::object::ops::float_result_bits(py, number);
                let converted = float_as_double(py, value).unwrap();
                assert!(converted == number || (converted.is_nan() && number.is_nan()));
                dec_ref_bits(py, value);
            }
        });
    }

    #[test]
    fn float_conversion_contract_strict_subclass_warnings_can_raise() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            struct ResetWarnings;
            impl Drop for ResetWarnings {
                fn drop(&mut self) {
                    crate::molt_warnings_resetwarnings();
                }
            }
            let _reset = ResetWarnings;
            crate::molt_warnings_resetwarnings();
            let category = crate::builtins::exceptions::exception_type_bits_from_name(
                py,
                "DeprecationWarning",
            );
            let float_class = conversion_class(py, builtin_classes(py).float, false, false);
            let returned = crate::molt_float_new(float_class, MoltObject::from_float(4.25).bits());
            let class = conversion_class(py, builtin_classes(py).object, true, false);
            let value = plain_instance(py, class);
            let ignore = attr_name_bits_from_bytes(py, b"ignore").unwrap();
            crate::molt_warnings_simplefilter(
                ignore,
                category,
                MoltObject::from_int(0).bits(),
                MoltObject::from_bool(false).bits(),
            );
            reset_callbacks(returned, false);
            assert_eq!(float_as_double(py, value), Some(4.25));
            assert!(!exception_pending(py));
            let error = attr_name_bits_from_bytes(py, b"error").unwrap();
            crate::molt_warnings_simplefilter(
                error,
                category,
                MoltObject::from_int(0).bits(),
                MoltObject::from_bool(false).bits(),
            );
            assert_eq!(float_as_double(py, value), None);
            assert_error(
                py,
                "DeprecationWarning",
                "FloatConversionContract.__float__ returned non-float (type FloatConversionContract).  The ability to return an instance of a strict subclass of float is deprecated, and may be removed in a future version of Python.",
            );
            for bits in [returned, float_class, value, class, ignore, error] {
                dec_ref_bits(py, bits);
            }

            let class = conversion_class(py, builtin_classes(py).object, false, true);
            let value = plain_instance(py, class);
            reset_callbacks(MoltObject::from_bool(true).bits(), false);
            assert_eq!(float_as_double(py, value), None);
            assert_error(
                py,
                "DeprecationWarning",
                "__index__ returned non-int (type bool).  The ability to return an instance of a strict subclass of int is deprecated, and may be removed in a future version of Python.",
            );
            dec_ref_bits(py, value);
            dec_ref_bits(py, class);
        });
    }
}

#[cfg(test)]
mod complex_arith_tests {
    use super::*;

    const INF: f64 = f64::INFINITY;
    const NAN: f64 = f64::NAN;

    fn c(re: f64, im: f64) -> ComplexOperand {
        ComplexOperand::Complex(ComplexParts { re, im })
    }

    fn r(value: f64) -> ComplexOperand {
        ComplexOperand::Real(value)
    }

    /// `assertComplexesAreIdentical`: NaNs match NaNs, zeros match by sign.
    fn identical(actual: Option<ComplexParts>, re: f64, im: f64) {
        let actual = actual.expect("a quotient, not a zero division");
        for (got, want) in [(actual.re, re), (actual.im, im)] {
            if want.is_nan() {
                assert!(got.is_nan(), "{actual:?} != ({re}, {im})");
            } else {
                assert_eq!(got.to_bits(), want.to_bits(), "{actual:?} != ({re}, {im})");
            }
        }
    }

    // Expected values: CPython 3.14 Lib/test/test_complex.py.
    #[test]
    fn mixed_mode_keeps_signed_zeros_from_3_14() {
        let (add, sub) = (ComplexArith::Add, ComplexArith::Sub);
        let (mul, div) = (ComplexArith::Mul, ComplexArith::TrueDiv);
        identical(complex_arith(add, c(-0.0, -0.0), r(-0.0), true), -0.0, -0.0);
        identical(complex_arith(add, r(-0.0), c(-0.0, -0.0), true), -0.0, -0.0);
        identical(complex_arith(sub, c(-0.0, -0.0), r(0.0), true), -0.0, -0.0);
        identical(complex_arith(sub, r(-0.0), c(0.0, 0.0), true), -0.0, -0.0);
        for (k, re, im) in [
            (2.0, INF, 2.0),
            (INF, INF, INF),
            (0.0, NAN, 0.0),
            (-0.0, NAN, -0.0),
        ] {
            identical(complex_arith(mul, c(INF, 1.0), r(k), true), re, im);
            identical(complex_arith(mul, r(k), c(INF, 1.0), true), re, im);
        }
        identical(complex_arith(div, c(INF, NAN), r(2.0), true), INF, NAN);
        identical(complex_arith(div, r(1.0), c(1.0, 2.0), true), 0.2, -0.4);
        identical(complex_arith(div, r(1.0), c(2.0, -1.0), true), 0.4, 0.2);
        identical(complex_arith(div, r(INF), c(1.0, 0.0), true), INF, NAN);
        identical(complex_arith(div, r(INF), c(0.0, 1.0), true), NAN, -INF);
        identical(complex_arith(div, r(1.0), c(INF, INF), true), 0.0, -0.0);
        identical(complex_arith(div, r(1.0), c(-INF, INF), true), -0.0, -0.0);
        identical(complex_arith(div, r(1.0), c(INF, NAN), true), 0.0, -0.0);
        identical(complex_arith(div, r(1.0), c(NAN, INF), true), 0.0, -0.0);
        identical(complex_arith(div, r(INF), c(NAN, INF), true), NAN, NAN);
    }

    // Before 3.14 the real operand is `complex(x, 0.0)` (IEEE-754: -0.0 + 0.0
    // is 0.0, and INF * 0.0 is NaN).
    #[test]
    fn real_operand_is_a_complex_with_zero_imaginary_before_3_14() {
        identical(
            complex_arith(ComplexArith::Add, c(-0.0, -0.0), r(-0.0), false),
            -0.0,
            0.0,
        );
        identical(
            complex_arith(ComplexArith::Add, r(1.0), c(1.0, -0.0), false),
            2.0,
            0.0,
        );
        identical(
            complex_arith(ComplexArith::Mul, c(INF, 1.0), r(2.0), false),
            INF,
            NAN,
        );
    }

    #[test]
    fn annex_g_recovery_from_3_14_only() {
        let mul = ComplexArith::Mul;
        for (z, w, re, im) in [
            ((1e300, 1.0), (INF, INF), NAN, INF),
            ((1e300, 1.0), (NAN, INF), -INF, INF),
            ((1e300, 1.0), (INF, NAN), INF, INF),
            ((INF, 1.0), (NAN, INF), NAN, INF),
            ((NAN, 1.0), (1.0, INF), -INF, NAN),
            ((1e200, NAN), (1e200, NAN), INF, NAN),
            ((NAN, 1e200), (NAN, 1e200), -INF, NAN),
            ((NAN, NAN), (NAN, NAN), NAN, NAN),
        ] {
            identical(complex_arith(mul, c(z.0, z.1), c(w.0, w.1), true), re, im);
            identical(complex_arith(mul, c(w.0, w.1), c(z.0, z.1), true), re, im);
        }
        identical(
            complex_arith(mul, c(1e300, 1.0), c(NAN, INF), false),
            NAN,
            NAN,
        );
        let div = ComplexArith::TrueDiv;
        for (z, w, re, im) in [
            ((INF, 1.0), (0.0, 1.0), NAN, -INF),
            ((INF, -INF), (1.0, 0.0), INF, -INF),
            ((INF, INF), (0.0, 1.0), INF, -INF),
            ((1.0, 1.0), (INF, INF), 0.0, 0.0),
            ((1.0, 1.0), (-INF, INF), 0.0, -0.0),
            ((1.0, 1.0), (-INF, -INF), -0.0, 0.0),
            ((INF, 1.0), (INF, INF), NAN, NAN),
            ((0.0, 0.0), (NAN, 0.0), NAN, NAN),
        ] {
            identical(complex_arith(div, c(z.0, z.1), c(w.0, w.1), true), re, im);
        }
        identical(
            complex_arith(div, c(INF, -INF), c(1.0, 0.0), false),
            NAN,
            NAN,
        );
    }

    #[test]
    fn smith_division_avoids_spurious_overflow() {
        // CPython's check_div(complex(1e200, 1e200), 1+0j): the textbook
        // formula overflows its denominator.
        identical(
            complex_arith(ComplexArith::TrueDiv, c(1e200, 1e200), c(1.0, 0.0), false),
            1e200,
            1e200,
        );
        identical(
            complex_arith(
                ComplexArith::TrueDiv,
                c(1e300, 1e300),
                c(1e300, 1e300),
                false,
            ),
            1.0,
            0.0,
        );
    }

    #[test]
    fn zero_divisors() {
        for py314 in [false, true] {
            assert!(
                complex_arith(ComplexArith::TrueDiv, c(1.0, 1.0), c(0.0, 0.0), py314).is_none()
            );
            assert!(complex_arith(ComplexArith::TrueDiv, r(1.0), c(0.0, -0.0), py314).is_none());
            assert!(complex_arith(ComplexArith::TrueDiv, c(1.0, 1.0), r(0.0), py314).is_none());
        }
    }
}
