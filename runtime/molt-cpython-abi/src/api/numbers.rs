//! Numeric type bridge — PyLong_*, PyFloat_*, PyBool_*.

use crate::abi_types::{
    IMMORTAL_REFCNT, Py_False, Py_True, Py_complex, Py_ssize_t, PyComplexObject, PyFloatObject,
    PyLongObject, PyLongValue, PyObject,
};
use crate::bridge::{GLOBAL_BRIDGE, ResolvedPyObject, resolve_pyobject, resolved_molt_handle};
use crate::hooks::hooks_or_stubs;
use molt_lang_obj_model::MoltObject;
use molt_lang_obj_model::float_bits::{
    FloatNarrowError, f16_bits_to_f64, f32_bits_to_f64, f64_to_f16_bits, f64_to_f32_bits,
};
use molt_lang_obj_model::int_literal::{
    IntLiteralErrorKind, ScannedIntLiteral, scan_int_literal_with_limit,
};
use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::os::raw::{c_char, c_double, c_int, c_long, c_longlong, c_ulong, c_ulonglong};
use std::ptr;

// ─── PyLong ──────────────────────────────────────────────────────────────────

fn py_long_from_i64(v: i64) -> *mut PyObject {
    if let Some(ptr) = cached_small_int_ptr(v) {
        return ptr;
    }
    let bits = MoltObject::try_from_int(v)
        .map(MoltObject::bits)
        .unwrap_or_else(|| unsafe { (hooks_or_stubs().int_from_i64)(v) });
    if bits == 0 {
        return ptr::null_mut();
    }
    let (ptr, _) = unsafe { materialize_numeric_owned_handle(bits) };
    ptr
}

fn py_long_from_u64(v: u64) -> *mut PyObject {
    if v <= SMALL_INT_MAX as u64
        && let Some(ptr) = cached_small_int_ptr(v as i64)
    {
        return ptr;
    }
    let bits = MoltObject::try_from_uint(v)
        .map(MoltObject::bits)
        .unwrap_or_else(|| unsafe { (hooks_or_stubs().int_from_u64)(v) });
    if bits == 0 {
        return ptr::null_mut();
    }
    let (ptr, _) = unsafe { materialize_numeric_owned_handle(bits) };
    ptr
}

// ─── Checked PyLong_As* conversion core ──────────────────────────────────────
//
// CPython's `PyLong_As*` family has an exact three-part contract
// (Objects/longobject.c):
//   1. non-int input either dispatches `__index__` (`AsLong`/`AsLongLong`/
//      `*AndOverflow`, via `_PyNumber_Index`) or raises TypeError
//      "an integer is required" (`AsSsize_t`/`AsUnsigned*`, `PyLong_Check` only);
//   2. an int outside the C target range raises OverflowError with a
//      width-specific message ("Python int too large to convert to C <type>");
//   3. `-1` is returned ONLY with an exception set — never as a bare sentinel a
//      caller could mistake for the value -1.
// The pre-fix `py_long_as_i64` violated all three (silent -1, silent
// truncation, no `__index__`), which numpy consumed as real shape/index values.

/// A successfully resolved integer value.
enum LongValue {
    Signed(i64),
    /// In `(i64::MAX, u64::MAX]` — representable only as unsigned 64-bit.
    Big(u64),
    Wide {
        bits: u64,
        low_u64: Option<u64>,
        sign: i8,
    },
}

/// Why a resolution failed. `Raised` means a CPython-shaped exception is
/// already pending (set by `__index__` dispatch or the NULL guard).
enum LongError {
    /// Not an integer, and `__index__` was not consulted (strict mode).
    NotInt,
    /// A genuine int whose magnitude exceeds the 64-bit hook envelope
    /// (`< i64::MIN` or `> u64::MAX`). For every C target of ≤ 64 bits this is
    /// exactly CPython's OverflowError case.
    /// Exception already set; propagate the sentinel.
    Raised,
}

/// Resolve `op` to a 64-bit integer through the verified paths: inline int,
/// bool (an int subtype), heap BigInt via the checked runtime hooks, then —
/// when `use_index` (the `AsLong`/`AsLongLong` family) — the `__index__`
/// protocol via `PyNumber_Index`. NULL raises SystemError (PyErr_BadInternalCall)
/// exactly like CPython's NULL guard.
fn py_long_value(op: *mut PyObject, use_index: bool) -> Result<LongValue, LongError> {
    if op.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return Err(LongError::Raised);
    }
    if let Some(tag) = unsafe { layout_long_tag(op) } {
        return unsafe { layout_long_value(op, tag) };
    }
    let resolution = resolve_pyobject(op);
    let op_handle = match resolution {
        Some(ResolvedPyObject::ManagedMolt(handle)) => Some(handle),
        Some(ResolvedPyObject::Foreign) | None => None,
    };
    if let Some(value) = op_handle {
        let bits = value.bits();
        let obj = value.decode();
        if let Some(v) = obj.as_int() {
            return Ok(LongValue::Signed(v));
        }
        if obj.is_bool() {
            return Ok(LongValue::Signed(obj.as_bool().unwrap_or(false) as i64));
        }
        if obj.is_ptr() {
            let h = hooks_or_stubs();
            if unsafe { (h.classify_heap)(bits) } == crate::abi_types::MoltTypeTag::Int as u8 {
                let mut sv = 0i64;
                if unsafe { (h.int_as_i64_checked)(bits, &raw mut sv) } == 0 {
                    return Ok(LongValue::Signed(sv));
                }
                let mut uv = 0u64;
                if unsafe { (h.int_as_u64_checked)(bits, &raw mut uv) } == 0 {
                    return Ok(LongValue::Big(uv));
                }
                // A genuine int beyond ±2^64: overflow for any ≤64-bit target.
                let Some(sign) = big_int_sign(bits).map(|sign| if sign < 0 { -1 } else { 1 })
                else {
                    if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
                        set_long_overflow_msg(c"runtime integer sign authority unavailable");
                    }
                    return Err(LongError::Raised);
                };
                return Ok(LongValue::Wide {
                    bits,
                    low_u64: None,
                    sign,
                });
            }
        }
    }
    if use_index {
        let protocol_op = op;
        // `_PyNumber_Index` dispatch: raises the CPython-shaped TypeError
        // ("'X' object cannot be interpreted as an integer") on failure.
        let index = unsafe { crate::api::abstract_number::PyNumber_Index(protocol_op) };
        if index.is_null() {
            return Err(LongError::Raised);
        }
        // The result is a real int; re-resolve WITHOUT __index__ (no loops).
        let mut result = py_long_value(index, false);
        if let Ok(LongValue::Wide {
            bits,
            low_u64: None,
            sign,
        }) = &result
        {
            let mut low_u64 = 0u64;
            if unsafe { (hooks_or_stubs().int_as_u64_mask)(*bits, 64, &raw mut low_u64) } == 0 {
                result = Ok(LongValue::Wide {
                    bits: *bits,
                    low_u64: Some(low_u64),
                    sign: *sign,
                });
            }
        }
        unsafe { crate::api::refcount::Py_DECREF(index) };
        return match result {
            Err(LongError::NotInt) => {
                // An `__index__` that produced a non-int: fail loud.
                set_long_type_error();
                Err(LongError::Raised)
            }
            other => other,
        };
    }
    Err(LongError::NotInt)
}

/// TypeError "an integer is required" — the strict (`PyLong_Check`-only)
/// non-int rejection shared by `AsSsize_t`/`AsUnsigned*` (Objects/longobject.c).
fn set_long_type_error() {
    unsafe {
        crate::api::errors::PyErr_SetString(
            (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
            c"an integer is required".as_ptr(),
        );
    }
}

/// OverflowError with the exact CPython width message.
fn set_long_overflow_msg(message: &'static std::ffi::CStr) {
    unsafe {
        crate::api::errors::PyErr_SetString(
            (&raw mut crate::abi_types::PyExc_OverflowError).cast::<crate::abi_types::PyObject>(),
            message.as_ptr(),
        );
    }
}

enum CheckedLongError {
    NotInt,
    Raised,
    Negative,
    Overflow(i8),
}

fn checked_signed_value(
    op: *mut PyObject,
    use_index: bool,
    min: i64,
    max: i64,
) -> Result<i64, CheckedLongError> {
    match py_long_value(op, use_index) {
        Ok(LongValue::Signed(value)) if value >= min && value <= max => Ok(value),
        Ok(LongValue::Signed(value)) => {
            Err(CheckedLongError::Overflow(if value < 0 { -1 } else { 1 }))
        }
        Ok(LongValue::Big(_)) => Err(CheckedLongError::Overflow(1)),
        Ok(LongValue::Wide { sign, .. }) => Err(CheckedLongError::Overflow(sign)),
        Err(LongError::NotInt) => Err(CheckedLongError::NotInt),
        Err(LongError::Raised) => Err(CheckedLongError::Raised),
    }
}

fn checked_unsigned_value(op: *mut PyObject, max: u64) -> Result<u64, CheckedLongError> {
    match py_long_value(op, false) {
        Ok(LongValue::Signed(value)) if value < 0 => Err(CheckedLongError::Negative),
        Ok(LongValue::Signed(value)) if value as u64 <= max => Ok(value as u64),
        Ok(LongValue::Signed(_)) => Err(CheckedLongError::Overflow(1)),
        Ok(LongValue::Big(value)) if value <= max => Ok(value),
        Ok(LongValue::Big(_)) | Ok(LongValue::Wide { .. }) => Err(CheckedLongError::Overflow(1)),
        Err(LongError::NotInt) => Err(CheckedLongError::NotInt),
        Err(LongError::Raised) => Err(CheckedLongError::Raised),
    }
}

fn masked_unsigned_value(op: *mut PyObject, width: u32) -> Result<u64, CheckedLongError> {
    let mask = if width == 64 {
        u64::MAX
    } else {
        (1u64 << width) - 1
    };
    match py_long_value(op, true) {
        Ok(LongValue::Signed(value)) => Ok((value as u64) & mask),
        Ok(LongValue::Big(value)) => Ok(value & mask),
        Ok(LongValue::Wide { bits, low_u64, .. }) => {
            let low_u64 = if let Some(low_u64) = low_u64 {
                low_u64
            } else {
                let mut low_u64 = 0u64;
                if unsafe { (hooks_or_stubs().int_as_u64_mask)(bits, 64, &raw mut low_u64) } != 0 {
                    if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
                        set_long_overflow_msg(c"runtime integer mask authority unavailable");
                    }
                    return Err(CheckedLongError::Raised);
                }
                low_u64
            };
            Ok(low_u64 & mask)
        }
        Err(LongError::NotInt) => Err(CheckedLongError::NotInt),
        Err(LongError::Raised) => Err(CheckedLongError::Raised),
    }
}

/// The runtime handle bits for the small int `v`, or 0 when unavailable.
fn small_int_bits(v: i64) -> u64 {
    MoltObject::try_from_int(v)
        .map(MoltObject::bits)
        .unwrap_or(0)
}

/// Sign of a heap bignum beyond the ±2^64 hook envelope, via the runtime
/// numeric authority: `v >> 128` is `-1` for negatives and `0` otherwise.
/// `None` when the authority is unavailable (stub hooks) or errored.
fn big_int_sign(bits: u64) -> Option<i64> {
    match unsafe { (hooks_or_stubs().int_sign)(bits) } {
        -1 => Some(-1),
        0 => Some(0),
        1 => Some(1),
        _ => None,
    }
}

const PYLONG_BITS_IN_DIGIT: usize = 30;
const PYLONG_SIGN_MASK: usize = 3;
const PYLONG_ZERO_TAG: usize = 1;
const PYLONG_NEGATIVE_TAG: usize = 2;

// CPython's one canonical small-integer family. These objects are built in
// static storage, carry the public compact-PyLong layout, and are immortal, so
// all threads observe one deterministic pointer without initialization locks,
// refcount writes, bridge-ledger entries, or heap traffic. The range is part of
// CPython's public implementation behavior (`_PY_NSMALLNEGINTS == 5`,
// `_PY_NSMALLPOSINTS == 257`); values outside it retain ordinary object
// identity and must never be interned by value.
const SMALL_INT_MIN: i64 = -5;
const SMALL_INT_MAX: i64 = 256;
const SMALL_INT_COUNT: usize = (SMALL_INT_MAX - SMALL_INT_MIN + 1) as usize;

const fn build_small_int_cache() -> [PyLongObject; SMALL_INT_COUNT] {
    let mut values = [const {
        PyLongObject {
            ob_base: PyObject {
                ob_refcnt: IMMORTAL_REFCNT,
                ob_type: std::ptr::null_mut(),
            },
            long_value: PyLongValue {
                lv_tag: PYLONG_ZERO_TAG,
                ob_digit: [0],
            },
        }
    }; SMALL_INT_COUNT];
    let mut index = 0;
    while index < SMALL_INT_COUNT {
        let value = SMALL_INT_MIN + index as i64;
        let (tag, digit) = if value == 0 {
            (PYLONG_ZERO_TAG, 0)
        } else if value < 0 {
            ((1 << 3) | PYLONG_NEGATIVE_TAG, (-value) as u32)
        } else {
            (1 << 3, value as u32)
        };
        values[index] = PyLongObject {
            ob_base: PyObject {
                ob_refcnt: IMMORTAL_REFCNT,
                ob_type: &raw mut crate::abi_types::PyLong_Type,
            },
            long_value: PyLongValue {
                lv_tag: tag,
                ob_digit: [digit],
            },
        };
        index += 1;
    }
    values
}

#[repr(transparent)]
struct SmallIntCache(UnsafeCell<[PyLongObject; SMALL_INT_COUNT]>);

// The only semantically mutable public field is `ob_refcnt`, and the sole
// refcount authority makes every operation on an immortal object a no-op.
// All value/type fields are initialized at compile time and never change.
unsafe impl Sync for SmallIntCache {}

static SMALL_INT_CACHE: SmallIntCache = SmallIntCache(UnsafeCell::new(build_small_int_cache()));

#[inline]
pub(crate) fn cached_small_int_ptr(value: i64) -> Option<*mut PyObject> {
    if !(SMALL_INT_MIN..=SMALL_INT_MAX).contains(&value) {
        return None;
    }
    let index = (value - SMALL_INT_MIN) as usize;
    let base = SMALL_INT_CACHE.0.get().cast::<PyLongObject>();
    Some(unsafe { base.add(index).cast::<PyObject>() })
}

/// Reverse-map only exact pointers into the static cache; no dereference is
/// performed, so arbitrary or stale foreign pointers are safe to classify.
pub(crate) fn cached_small_int_bits_from_ptr(ptr: *mut PyObject) -> Option<u64> {
    if ptr.is_null() {
        return None;
    }
    let stride = std::mem::size_of::<PyLongObject>();
    let base = SMALL_INT_CACHE.0.get().cast::<PyLongObject>() as usize;
    let offset = (ptr as usize).checked_sub(base)?;
    if offset % stride != 0 {
        return None;
    }
    let index = offset / stride;
    if index >= SMALL_INT_COUNT {
        return None;
    }
    let value = SMALL_INT_MIN + index as i64;
    MoltObject::try_from_int(value).map(MoltObject::bits)
}

#[inline]
pub(crate) fn is_cached_small_int_handle(bits: u64) -> bool {
    MoltObject::from_bits(bits)
        .as_int()
        .is_some_and(|value| (SMALL_INT_MIN..=SMALL_INT_MAX).contains(&value))
}

/// True when `op` carries the public CPython long layout directly.
///
/// Molt numeric carriers, bool singletons, and foreign int subclasses all own
/// a complete `PyLongObject` prefix. Resolve that physical authority before
/// consulting the bridge maps; generic managed views use `MoltManaged_Type`
/// and therefore cannot enter this path.
unsafe fn has_layout_long(op: *mut PyObject) -> bool {
    if op.is_null() {
        return false;
    }
    let ob_type = unsafe { (*op).ob_type };
    if ob_type.is_null() {
        return false;
    }
    std::ptr::eq(ob_type, &raw const crate::abi_types::PyLong_Type)
        || std::ptr::eq(ob_type, &raw const crate::abi_types::PyBool_Type)
        || unsafe {
            crate::api::typeobj::PyType_IsSubtype(ob_type, &raw mut crate::abi_types::PyLong_Type)
                != 0
        }
}

unsafe fn has_layout_float(op: *mut PyObject) -> bool {
    if op.is_null() {
        return false;
    }
    let ob_type = unsafe { (*op).ob_type };
    if ob_type.is_null() {
        return false;
    }
    std::ptr::eq(ob_type, &raw const crate::abi_types::PyFloat_Type)
        || unsafe {
            crate::api::typeobj::PyType_IsSubtype(ob_type, &raw mut crate::abi_types::PyFloat_Type)
                != 0
        }
}

unsafe fn layout_float_value(op: *mut PyObject) -> Option<f64> {
    if !unsafe { has_layout_float(op) } {
        return None;
    }
    // C-minted objects may be only 4-byte aligned on wasm32. A raw field
    // reference plus an unaligned read preserves the public layout without UB.
    let field = unsafe { &raw const (*op.cast::<PyFloatObject>()).ob_fval };
    Some(unsafe { std::ptr::read_unaligned(field) })
}

unsafe fn has_layout_complex(op: *mut PyObject) -> bool {
    if op.is_null() {
        return false;
    }
    let ob_type = unsafe { (*op).ob_type };
    if ob_type.is_null() {
        return false;
    }
    std::ptr::eq(ob_type, &raw const crate::abi_types::PyComplex_Type)
        || unsafe {
            crate::api::typeobj::PyType_IsSubtype(
                ob_type,
                &raw mut crate::abi_types::PyComplex_Type,
            ) != 0
        }
}

unsafe fn layout_complex_value(op: *mut PyObject) -> Option<Py_complex> {
    if !unsafe { has_layout_complex(op) } {
        return None;
    }
    // Same wasm32 alignment rule as the float carrier: never form an aligned
    // Rust reference to a C allocation whose public ABI permits 4-byte align.
    let field = unsafe { &raw const (*op.cast::<PyComplexObject>()).cval };
    Some(unsafe { std::ptr::read_unaligned(field) })
}

unsafe fn layout_long_tag(op: *mut PyObject) -> Option<usize> {
    if !unsafe { has_layout_long(op) } {
        return None;
    }
    Some(unsafe {
        std::ptr::read_unaligned(
            &raw const (*op.cast::<crate::abi_types::PyLongObject>())
                .long_value
                .lv_tag,
        )
    })
}

#[inline]
fn layout_long_sign(tag: usize) -> i8 {
    match tag & PYLONG_SIGN_MASK {
        PYLONG_ZERO_TAG => 0,
        PYLONG_NEGATIVE_TAG => -1,
        _ => 1,
    }
}

unsafe fn layout_long_digit(op: *mut PyObject, index: usize) -> u32 {
    let first = unsafe {
        &raw const (*op.cast::<crate::abi_types::PyLongObject>())
            .long_value
            .ob_digit
    }
    .cast::<u32>();
    unsafe { std::ptr::read_unaligned(first.add(index)) }
}

unsafe fn layout_long_value(op: *mut PyObject, tag: usize) -> Result<LongValue, LongError> {
    let sign = layout_long_sign(tag);
    let digits = tag >> 3;
    if sign == 0 || digits == 0 {
        return Ok(LongValue::Signed(0));
    }
    let mut magnitude = 0u128;
    for index in (0..digits.min(3)).rev() {
        magnitude =
            (magnitude << PYLONG_BITS_IN_DIGIT) | unsafe { layout_long_digit(op, index) } as u128;
    }
    if digits > 3 || magnitude > u64::MAX as u128 {
        let low = magnitude as u64;
        return Ok(LongValue::Wide {
            bits: 0,
            low_u64: Some(if sign < 0 {
                0u64.wrapping_sub(low)
            } else {
                low
            }),
            sign,
        });
    }
    if sign < 0 {
        if magnitude == i64::MAX as u128 + 1 {
            return Ok(LongValue::Signed(i64::MIN));
        }
        if magnitude <= i64::MAX as u128 {
            return Ok(LongValue::Signed(-(magnitude as i64)));
        }
        return Ok(LongValue::Wide {
            bits: 0,
            low_u64: Some((0u64).wrapping_sub(magnitude as u64)),
            sign,
        });
    }
    if magnitude <= i64::MAX as u128 {
        Ok(LongValue::Signed(magnitude as i64))
    } else {
        Ok(LongValue::Big(magnitude as u64))
    }
}

unsafe fn layout_long_num_bits(op: *mut PyObject, tag: usize) -> usize {
    let digits = tag >> 3;
    if digits == 0 {
        return 0;
    }
    let top = unsafe { layout_long_digit(op, digits - 1) };
    (digits - 1)
        .saturating_mul(PYLONG_BITS_IN_DIGIT)
        .saturating_add((u32::BITS - top.leading_zeros()) as usize)
}

unsafe fn layout_long_magnitude_is_power_of_two(op: *mut PyObject, tag: usize) -> bool {
    let digits = tag >> 3;
    let mut seen = false;
    for index in 0..digits {
        let digit = unsafe { layout_long_digit(op, index) };
        if digit == 0 {
            continue;
        }
        if seen || !digit.is_power_of_two() {
            return false;
        }
        seen = true;
    }
    seen
}

unsafe fn layout_long_as_byte_array(
    op: *mut PyObject,
    tag: usize,
    bytes: *mut u8,
    n: usize,
    little_endian: c_int,
    is_signed: c_int,
) -> c_int {
    let sign = layout_long_sign(tag);
    if sign < 0 && is_signed == 0 {
        set_long_overflow_msg(c"can't convert negative int to unsigned");
        return -1;
    }
    let width = n.saturating_mul(8);
    if n != 0 {
        let out = unsafe { std::slice::from_raw_parts_mut(bytes, n) };
        for (output_index, output) in out.iter_mut().enumerate() {
            let low_index = if little_endian != 0 {
                output_index
            } else {
                n - 1 - output_index
            };
            let bit = low_index * 8;
            let digit_index = bit / PYLONG_BITS_IN_DIGIT;
            let digit_shift = bit % PYLONG_BITS_IN_DIGIT;
            let mut chunk = if digit_index < (tag >> 3) {
                (unsafe { layout_long_digit(op, digit_index) } as u64) >> digit_shift
            } else {
                0
            };
            if digit_shift > PYLONG_BITS_IN_DIGIT - 8 && digit_index + 1 < (tag >> 3) {
                chunk |= (unsafe { layout_long_digit(op, digit_index + 1) } as u64)
                    << (PYLONG_BITS_IN_DIGIT - digit_shift);
            }
            *output = chunk as u8;
        }
        if sign < 0 {
            let mut carry = 1u16;
            if little_endian != 0 {
                for byte in out.iter_mut() {
                    let value = (!*byte as u16 & 0xff) + carry;
                    *byte = value as u8;
                    carry = value >> 8;
                }
            } else {
                for byte in out.iter_mut().rev() {
                    let value = (!*byte as u16 & 0xff) + carry;
                    *byte = value as u8;
                    carry = value >> 8;
                }
            }
        }
    }

    let num_bits = unsafe { layout_long_num_bits(op, tag) };
    let fits = if sign == 0 {
        true
    } else if is_signed == 0 {
        num_bits <= width
    } else if sign > 0 {
        num_bits < width
    } else {
        num_bits < width
            || (num_bits == width && unsafe { layout_long_magnitude_is_power_of_two(op, tag) })
    };
    if fits {
        0
    } else {
        set_long_overflow();
        -1
    }
}

pub(crate) unsafe fn copy_layout_long_to_exact(op: *mut PyObject) -> *mut PyObject {
    let Some(tag) = (unsafe { layout_long_tag(op) }) else {
        return ptr::null_mut();
    };
    let sign = layout_long_sign(tag);
    let bits = unsafe { layout_long_num_bits(op, tag) };
    let width = if sign < 0 {
        let base = bits.div_ceil(8).max(1);
        if bits != 0 && bits % 8 == 0 && !unsafe { layout_long_magnitude_is_power_of_two(op, tag) }
        {
            base + 1
        } else {
            base
        }
    } else {
        bits.saturating_add(1).div_ceil(8).max(1)
    };
    let mut bytes = Vec::new();
    if bytes.try_reserve_exact(width).is_err() {
        unsafe { crate::api::errors::PyErr_NoMemory() };
        return ptr::null_mut();
    }
    bytes.resize(width, 0);
    if unsafe { layout_long_as_byte_array(op, tag, bytes.as_mut_ptr(), width, 1, 1) } != 0 {
        return ptr::null_mut();
    }
    unsafe { _PyLong_FromByteArray(bytes.as_ptr(), width, 1, 1) }
}

/// True when `op` resolves to a genuine Molt int (inline int, bool, or heap
/// BigInt) — the objects `py_long_as_ssize_clamped` reads directly without an
/// `__index__` round-trip. Zero-alloc; the slice fast path depends on it.
pub(crate) fn is_int_like(op: *mut PyObject) -> bool {
    if op.is_null() {
        return false;
    }
    if unsafe { has_layout_long(op) } {
        return true;
    }
    let Some(bits) = resolved_molt_handle(op) else {
        return false;
    };
    let obj = bits.decode();
    obj.is_int()
        || obj.is_bool()
        || (obj.is_ptr()
            && unsafe { (hooks_or_stubs().classify_heap)(bits.bits()) }
                == crate::abi_types::MoltTypeTag::Int as u8)
}

/// `PyNumber_AsSsize_t(v, NULL)`-style clamped read of an *integer* object
/// (Objects/abstract.c): an out-of-`Py_ssize_t` value CLAMPS to
/// `PY_SSIZE_T_MAX`/`PY_SSIZE_T_MIN` by the value's sign instead of raising —
/// the `err == NULL` contract the slice machinery relies on
/// (`_PyEval_SliceIndex`, Python/ceval.c). Returns `None` with a pending
/// exception when `op` is not an int or the sign of a beyond-±2^64 bignum
/// cannot be resolved (runtime authority absent — loud, never a wrong-direction
/// clamp).
pub(crate) fn py_long_as_ssize_clamped(op: *mut PyObject) -> Option<isize> {
    match py_long_value(op, false) {
        Ok(LongValue::Signed(v)) => {
            // On 32-bit targets (wasm32) clamp rather than truncate.
            Some(v.clamp(isize::MIN as i64, isize::MAX as i64) as isize)
        }
        Ok(LongValue::Big(_)) => Some(isize::MAX),
        Ok(LongValue::Wide { sign, .. }) if sign < 0 => Some(isize::MIN),
        Ok(LongValue::Wide { .. }) => Some(isize::MAX),
        Err(LongError::NotInt) => {
            set_long_type_error();
            None
        }
        Err(LongError::Raised) => None,
    }
}

/// Exact int→f64 conversion for a heap bignum beyond the 64-bit hook envelope,
/// via the runtime's own numeric authority: `v / 1` (TrueDivide) is the exact
/// CPython `nb_float`-equivalent conversion, raising the runtime's OverflowError
/// past f64 range. Returns `None` when the authority is unavailable (stub hooks)
/// or errored (pending exception).
fn big_int_as_f64(bits: u64) -> Option<f64> {
    let h = hooks_or_stubs();
    let one = small_int_bits(1);
    if one == 0 {
        return None;
    }
    let result =
        unsafe { (h.number_binary_op)(crate::hooks::NumberBinaryOp::TrueDivide as u32, bits, one) };
    match result.decode() {
        crate::hooks::DecodedHandleResult::Ok(result_bits) => {
            let value = MoltObject::from_bits(result_bits).as_float();
            if MoltObject::from_bits(result_bits).is_ptr() {
                unsafe { (h.dec_ref)(result_bits) };
            }
            value
        }
        crate::hooks::DecodedHandleResult::Missing | crate::hooks::DecodedHandleResult::Error => {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NumericParseError {
    InvalidBase,
    InvalidLiteral,
    Overflow,
    Type,
    TooManyDigits { limit: usize, digits: usize },
}

fn scan_python_int_literal(
    bytes: &[u8],
    base_arg: c_int,
) -> Result<ScannedIntLiteral, NumericParseError> {
    let max_digits = unsafe { (hooks_or_stubs().int_max_str_digits)() };
    scan_int_literal_with_limit(bytes, base_arg, max_digits).map_err(|error| match error.kind {
        IntLiteralErrorKind::InvalidBase => NumericParseError::InvalidBase,
        IntLiteralErrorKind::InvalidLiteral => NumericParseError::InvalidLiteral,
        IntLiteralErrorKind::TooManyDigits => NumericParseError::TooManyDigits {
            limit: max_digits,
            digits: error.offset,
        },
    })
}

#[cfg(test)]
fn parse_python_int_literal(bytes: &[u8], base_arg: c_int) -> Result<i128, NumericParseError> {
    let scanned = scan_python_int_literal(bytes, base_arg)?;
    let limit: u128 = if scanned.negative {
        (i64::MAX as u128) + 1
    } else {
        u64::MAX as u128
    };
    let mut value = 0u128;
    for &digit in &scanned.digits {
        value = value
            .checked_mul(scanned.base as u128)
            .and_then(|acc| acc.checked_add(digit as u128))
            .ok_or(NumericParseError::Overflow)?;
        if value > limit {
            return Err(NumericParseError::Overflow);
        }
    }
    if scanned.negative {
        if value == (i64::MAX as u128) + 1 {
            Ok(i64::MIN as i128)
        } else {
            Ok(-((value as i64) as i128))
        }
    } else {
        Ok(value as i128)
    }
}

fn py_long_from_scanned(scanned: &ScannedIntLiteral) -> *mut PyObject {
    let limit = if scanned.negative {
        i64::MAX as u128 + 1
    } else {
        u64::MAX as u128
    };
    let mut magnitude = 0u128;
    let mut fast = true;
    for digit in &scanned.digits {
        let Some(next) = magnitude
            .checked_mul(scanned.base as u128)
            .and_then(|value| value.checked_add(*digit as u128))
        else {
            fast = false;
            break;
        };
        magnitude = next;
        if magnitude > limit {
            fast = false;
            break;
        }
    }
    if fast {
        if scanned.negative {
            if magnitude == i64::MAX as u128 + 1 {
                return py_long_from_i64(i64::MIN);
            }
            return py_long_from_i64(-(magnitude as i64));
        }
        return py_long_from_u64(magnitude as u64);
    }
    let bits = unsafe {
        (hooks_or_stubs().int_from_digits)(
            scanned.digits.as_ptr(),
            scanned.digits.len(),
            scanned.base,
            scanned.negative as c_int,
        )
    };
    if bits == 0 {
        unsafe {
            set_numeric_parse_error(
                NumericParseError::Overflow,
                c"int literal exceeds the Molt int authority (runtime hooks absent)",
            )
        };
        return ptr::null_mut();
    }
    unsafe { materialize_numeric_owned_handle(bits).0 }
}

fn normalize_float_literal(bytes: &[u8]) -> Result<String, NumericParseError> {
    let text = std::str::from_utf8(bytes).map_err(|_| NumericParseError::InvalidLiteral)?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(NumericParseError::InvalidLiteral);
    }
    let raw = trimmed.as_bytes();
    let mut out = String::with_capacity(raw.len());
    for (index, &byte) in raw.iter().enumerate() {
        if byte == b'_' {
            let prev_is_digit = index > 0 && raw[index - 1].is_ascii_digit();
            let next_is_digit = index + 1 < raw.len() && raw[index + 1].is_ascii_digit();
            if !prev_is_digit || !next_is_digit {
                return Err(NumericParseError::InvalidLiteral);
            }
            continue;
        }
        if !byte.is_ascii() {
            return Err(NumericParseError::InvalidLiteral);
        }
        out.push(byte as char);
    }
    Ok(out)
}

fn parse_python_float_literal(bytes: &[u8]) -> Result<f64, NumericParseError> {
    let normalized = normalize_float_literal(bytes)?;
    let lower = normalized.to_ascii_lowercase();
    match lower.as_str() {
        "nan" | "+nan" | "-nan" => return Ok(f64::NAN),
        "inf" | "+inf" | "infinity" | "+infinity" => return Ok(f64::INFINITY),
        "-inf" | "-infinity" => return Ok(f64::NEG_INFINITY),
        _ => {}
    }
    normalized
        .parse::<f64>()
        .map_err(|_| NumericParseError::InvalidLiteral)
}

unsafe fn py_textlike_bytes(op: *mut PyObject) -> Result<Vec<u8>, NumericParseError> {
    if op.is_null() {
        return Err(NumericParseError::Type);
    }
    // Overlay inline constructors return raw NaN-box handles. Decode those
    // through the same runtime hook authority as ABI proxies, without
    // materializing a bridge proxy solely to parse its bytes.
    if let Some(handle) = resolved_molt_handle(op) {
        let hooks = hooks_or_stubs();
        let tag = unsafe { (hooks.classify_heap)(handle.bits()) };
        let mut len = 0usize;
        let data = if tag == crate::abi_types::MoltTypeTag::Str as u8 {
            unsafe { (hooks.str_data)(handle.bits(), &raw mut len) }
        } else if tag == crate::abi_types::MoltTypeTag::Bytes as u8 {
            unsafe { (hooks.bytes_data)(handle.bits(), &raw mut len) }
        } else {
            return Err(NumericParseError::Type);
        };
        if data.is_null() {
            return Err(NumericParseError::InvalidLiteral);
        }
        return Ok(unsafe { std::slice::from_raw_parts(data, len) }.to_vec());
    }
    if unsafe { crate::api::strings::PyUnicode_Check(op) } != 0 {
        let mut len: Py_ssize_t = 0;
        let ptr = unsafe { crate::api::strings::PyUnicode_AsUTF8AndSize(op, &raw mut len) };
        if ptr.is_null() || len < 0 {
            return Err(NumericParseError::InvalidLiteral);
        }
        let bytes = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), len as usize) };
        return Ok(bytes.to_vec());
    }
    if unsafe { crate::api::strings::PyBytes_Check(op) } != 0 {
        let mut ptr_out: *mut c_char = ptr::null_mut();
        let mut len: Py_ssize_t = 0;
        if unsafe {
            crate::api::strings::PyBytes_AsStringAndSize(op, &raw mut ptr_out, &raw mut len)
        } != 0
            || ptr_out.is_null()
            || len < 0
        {
            return Err(NumericParseError::InvalidLiteral);
        }
        let bytes = unsafe { std::slice::from_raw_parts(ptr_out.cast::<u8>(), len as usize) };
        return Ok(bytes.to_vec());
    }
    Err(NumericParseError::Type)
}

unsafe fn set_numeric_parse_error(kind: NumericParseError, message: &'static std::ffi::CStr) {
    let exc = match kind {
        NumericParseError::InvalidBase
        | NumericParseError::InvalidLiteral
        | NumericParseError::TooManyDigits { .. } => {
            (&raw mut crate::abi_types::PyExc_ValueError).cast::<crate::abi_types::PyObject>()
        }
        NumericParseError::Overflow => {
            (&raw mut crate::abi_types::PyExc_OverflowError).cast::<crate::abi_types::PyObject>()
        }
        NumericParseError::Type => {
            (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>()
        }
    };
    unsafe { crate::api::errors::PyErr_SetString(exc, message.as_ptr()) };
}

unsafe fn set_digit_limit_error(limit: usize, digits: usize) {
    let message = std::ffi::CString::new(format!(
        "Exceeds the limit ({limit} digits) for integer string conversion: value has {digits} digits; use sys.set_int_max_str_digits() to increase the limit"
    ))
    .expect("digit-limit message has no NUL");
    unsafe {
        crate::api::errors::PyErr_SetString(
            (&raw mut crate::abi_types::PyExc_ValueError).cast::<crate::abi_types::PyObject>(),
            message.as_ptr(),
        )
    };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromLong(v: c_long) -> *mut PyObject {
    #[allow(clippy::unnecessary_cast)]
    py_long_from_i64(v as i64)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromSsize_t(v: isize) -> *mut PyObject {
    py_long_from_i64(v as i64)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromSize_t(v: usize) -> *mut PyObject {
    py_long_from_u64(v as u64)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromLongLong(v: c_longlong) -> *mut PyObject {
    #[allow(clippy::unnecessary_cast)]
    py_long_from_i64(v as i64)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromUnsignedLong(v: c_ulong) -> *mut PyObject {
    #[allow(clippy::unnecessary_cast)]
    py_long_from_u64(v as u64)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromUnsignedLongLong(v: c_ulonglong) -> *mut PyObject {
    #[allow(clippy::unnecessary_cast)]
    py_long_from_u64(v as u64)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromVoidPtr(p: *mut c_void) -> *mut PyObject {
    py_long_from_u64(p as usize as u64)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromDouble(v: c_double) -> *mut PyObject {
    if v.is_nan() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_ValueError).cast::<crate::abi_types::PyObject>(),
                c"cannot convert float NaN to integer".as_ptr(),
            );
        }
        return ptr::null_mut();
    }
    if !v.is_finite() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_OverflowError)
                    .cast::<crate::abi_types::PyObject>(),
                c"cannot convert float infinity to integer".as_ptr(),
            );
        }
        return ptr::null_mut();
    }

    let truncated = v.trunc();
    if (-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&truncated) {
        return py_long_from_i64(truncated as i64);
    }
    // |value| >= 2^63: CPython builds the exact arbitrary-precision integer
    // (Objects/longobject.c PyLong_FromDouble never overflows for a finite
    // double). Construct it directly through the runtime authority: the old
    // mantissa/shift/negate pipeline allocated and retained owned BigInt
    // intermediates instead of producing one final integer.
    let result = unsafe { (hooks_or_stubs().int_from_f64_trunc)(truncated) };
    if result == 0 {
        // Runtime numeric authority unavailable (stub hooks) or errored: fail
        // loudly, never silently — but only set our message when the authority
        // did not already raise its own.
        let h2 = hooks_or_stubs();
        if unsafe { crate::api::errors::PyErr_Occurred() }.is_null()
            && unsafe { (h2.exception_pending)() } == 0
        {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_OverflowError)
                        .cast::<crate::abi_types::PyObject>(),
                    c"float too large for the Molt int authority (runtime hooks absent)".as_ptr(),
                );
            }
        }
        return ptr::null_mut();
    }
    unsafe { materialize_numeric_owned_handle(result).0 }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyLong_Sign(op: *mut PyObject) -> c_int {
    if let Some(tag) = unsafe { layout_long_tag(op) } {
        return layout_long_sign(tag) as c_int;
    }
    match py_long_value(op, false) {
        Ok(LongValue::Signed(value)) => value.signum() as c_int,
        Ok(LongValue::Big(_)) => 1,
        Ok(LongValue::Wide { sign, .. }) => sign.signum() as c_int,
        Err(LongError::NotInt | LongError::Raised) => 0,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnstable_Long_IsCompact(
    op: *const crate::abi_types::PyLongObject,
) -> c_int {
    let obj = op.cast_mut().cast::<PyObject>();
    if let Some(tag) = unsafe { layout_long_tag(obj) } {
        return ((tag >> 3) < 2) as c_int;
    }
    match py_long_value(obj, false) {
        Ok(LongValue::Signed(value)) => (value.unsigned_abs() < (1u64 << 30)) as c_int,
        Ok(LongValue::Big(value)) => (value < (1u64 << 30)) as c_int,
        Ok(LongValue::Wide { .. }) | Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnstable_Long_CompactValue(
    op: *const crate::abi_types::PyLongObject,
) -> Py_ssize_t {
    let obj = op.cast_mut().cast::<PyObject>();
    if let Some(tag) = unsafe { layout_long_tag(obj) } {
        if (tag >> 3) == 0 {
            return 0;
        }
        let digit = unsafe { layout_long_digit(obj, 0) } as Py_ssize_t;
        return if layout_long_sign(tag) < 0 {
            -digit
        } else {
            digit
        };
    }
    match py_long_value(obj, false) {
        Ok(LongValue::Signed(value)) if value.unsigned_abs() < (1u64 << 30) => value as Py_ssize_t,
        Ok(LongValue::Big(value)) if value < (1u64 << 30) => value as Py_ssize_t,
        _ => 0,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyLong_NumBits(op: *mut PyObject) -> usize {
    if let Some(tag) = unsafe { layout_long_tag(op) } {
        return unsafe { layout_long_num_bits(op, tag) };
    }
    match py_long_value(op, false) {
        Ok(LongValue::Signed(value)) => (u64::BITS - value.unsigned_abs().leading_zeros()) as usize,
        Ok(LongValue::Big(value)) => (u64::BITS - value.leading_zeros()) as usize,
        Ok(LongValue::Wide { bits, .. }) => {
            let mut out = 0usize;
            if unsafe { (hooks_or_stubs().int_num_bits)(bits, &raw mut out) } == 0 {
                out
            } else {
                if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
                    set_long_overflow_msg(c"int has too many bits to fit size_t");
                }
                usize::MAX
            }
        }
        Err(LongError::NotInt) => {
            set_long_type_error();
            usize::MAX
        }
        Err(LongError::Raised) => usize::MAX,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromUnicodeObject(u: *mut PyObject, base: c_int) -> *mut PyObject {
    if u.is_null() || unsafe { crate::api::strings::PyUnicode_Check(u) } == 0 {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    let mut len = 0isize;
    let data = unsafe { crate::api::strings::PyUnicode_AsUTF8AndSize(u, &raw mut len) };
    if data.is_null() || len < 0 {
        return ptr::null_mut();
    }
    let raw = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), len as usize) };
    let text = match std::str::from_utf8(raw) {
        Ok(text) => text,
        Err(_) => {
            unsafe {
                set_numeric_parse_error(
                    NumericParseError::InvalidLiteral,
                    c"invalid literal for int()",
                )
            };
            return ptr::null_mut();
        }
    };
    let mut bytes = Vec::with_capacity(raw.len());
    for ch in text.chars() {
        if ch.is_ascii() {
            bytes.push(ch as u8);
        } else if let Some(digit) = crate::api::strings::unicode_decimal_digit_value(ch as u32) {
            bytes.push(b'0' + digit);
        } else if crate::api::strings::unicode_is_space(ch as u32) {
            bytes.push(b' ');
        } else {
            bytes.push(b'?');
        }
    }
    match scan_python_int_literal(&bytes, base) {
        Ok(scanned) => py_long_from_scanned(&scanned),
        Err(NumericParseError::TooManyDigits { limit, digits }) => {
            unsafe { set_digit_limit_error(limit, digits) };
            ptr::null_mut()
        }
        Err(kind) => {
            unsafe { set_numeric_parse_error(kind, c"invalid literal for int()") };
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromString(
    str_: *const c_char,
    pend: *mut *mut c_char,
    base: c_int,
) -> *mut PyObject {
    if str_.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    if base != 0 && !(2..=36).contains(&base) {
        unsafe {
            set_numeric_parse_error(
                NumericParseError::InvalidBase,
                c"int() base must be >= 2 and <= 36, or 0",
            )
        };
        return ptr::null_mut();
    }
    let bytes = unsafe { std::ffi::CStr::from_ptr(str_) }.to_bytes();
    let max_digits = unsafe { (hooks_or_stubs().int_max_str_digits)() };
    match scan_int_literal_with_limit(bytes, base, max_digits) {
        Ok(scanned) => {
            if !pend.is_null() {
                unsafe { *pend = str_.add(scanned.end).cast_mut() };
            }
            py_long_from_scanned(&scanned)
        }
        Err(error) => {
            if !pend.is_null() && error.kind == IntLiteralErrorKind::InvalidLiteral {
                unsafe { *pend = str_.add(error.offset).cast_mut() };
            }
            let kind = match error.kind {
                IntLiteralErrorKind::InvalidBase => NumericParseError::InvalidBase,
                IntLiteralErrorKind::InvalidLiteral => NumericParseError::InvalidLiteral,
                IntLiteralErrorKind::TooManyDigits => {
                    unsafe { set_digit_limit_error(max_digits, error.offset) };
                    return ptr::null_mut();
                }
            };
            unsafe { set_numeric_parse_error(kind, c"invalid literal for int()") };
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyLong_FromByteArray(
    bytes: *const u8,
    n: usize,
    little_endian: c_int,
    is_signed: c_int,
) -> *mut PyObject {
    if bytes.is_null() && n != 0 {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    let bits = unsafe { (hooks_or_stubs().int_from_bytes)(bytes, n, little_endian, is_signed) };
    if bits == 0 {
        // Preserve a runtime exception when present; otherwise the hook table
        // is absent and this operation cannot be represented honestly.
        if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
            set_long_overflow_msg(c"runtime arbitrary-width integer authority unavailable");
        }
        return ptr::null_mut();
    }
    unsafe { materialize_numeric_owned_handle(bits).0 }
}

/// CPython `PyLong_AsLong` (Objects/longobject.c): accepts int and any object
/// with `__index__` (via `_PyNumber_Index`); raises OverflowError
/// "Python int too large to convert to C long" when the value does not fit the
/// platform `long` (32-bit on Windows/wasm32); returns -1 only with an
/// exception set. The pre-fix body silently truncated (`as c_long`) and
/// returned bare -1 sentinels.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsLong(op: *mut PyObject) -> c_long {
    match checked_signed_value(op, true, c_long::MIN as i64, c_long::MAX as i64) {
        Ok(value) => value as c_long,
        Err(CheckedLongError::Overflow(_)) => {
            set_long_overflow_msg(c"Python int too large to convert to C long");
            -1
        }
        Err(CheckedLongError::NotInt) => {
            set_long_type_error();
            -1
        }
        Err(CheckedLongError::Raised | CheckedLongError::Negative) => -1,
    }
}

/// CPython 3.14 `PyLong_IsZero`: return 1 for a zero int, 0 for a non-zero
/// int, and -1 with TypeError for any non-int object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_IsZero(op: *mut PyObject) -> c_int {
    match py_long_value(op, false) {
        Ok(LongValue::Signed(value)) => (value == 0) as c_int,
        Ok(LongValue::Big(value)) => (value == 0) as c_int,
        Ok(LongValue::Wide { .. }) => 0,
        Err(LongError::NotInt) => {
            set_long_type_error();
            -1
        }
        Err(LongError::Raised) => -1,
    }
}

/// CPython ``PyLong_AsDouble`` (Objects/longobject.c): converts any Python
/// int — including heap bignums — to ``double``, raising OverflowError only
/// past f64 range and TypeError "an integer is required" for a non-int; -1.0
/// is returned only with an exception set. Bignums beyond the 64-bit hook
/// envelope convert exactly through the runtime numeric authority
/// (``v / 1`` TrueDivide), which raises its own OverflowError past 1e308.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsDouble(op: *mut PyObject) -> c_double {
    match py_long_value(op, false) {
        Ok(LongValue::Signed(v)) => v as c_double,
        Ok(LongValue::Big(v)) => v as c_double,
        Err(LongError::NotInt) => {
            set_long_type_error();
            -1.0
        }
        Ok(LongValue::Wide { bits, .. }) => {
            // A genuine int beyond ±2^64: exact conversion via the runtime
            // authority when available; honest OverflowError otherwise.
            if bits != 0
                && let Some(v) = big_int_as_f64(bits)
            {
                return v;
            }
            if bits == 0
                && let Some(tag) = unsafe { layout_long_tag(op) }
            {
                let value = unsafe { layout_long_to_f64_rounded(op, tag) };
                if value.is_finite() {
                    return value;
                }
            }
            if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
                set_long_overflow_msg(c"int too large to convert to float");
            }
            -1.0
        }
        Err(LongError::Raised) => -1.0,
    }
}

unsafe fn layout_long_to_f64_rounded(op: *mut PyObject, tag: usize) -> f64 {
    let bit_len = unsafe { layout_long_num_bits(op, tag) };
    if bit_len == 0 {
        return 0.0;
    }
    let bit = |index: usize| unsafe {
        let digit = layout_long_digit(op, index / PYLONG_BITS_IN_DIGIT);
        (digit >> (index % PYLONG_BITS_IN_DIGIT)) & 1
    };
    let mut shift = bit_len.saturating_sub(53);
    let mut significand = 0u64;
    for source in (shift..bit_len).rev() {
        significand = (significand << 1) | u64::from(bit(source));
    }
    if shift != 0 {
        let halfway = bit(shift - 1) != 0;
        let sticky = (0..shift - 1).any(|index| bit(index) != 0);
        if halfway && (sticky || significand & 1 != 0) {
            significand += 1;
            if significand == 1u64 << 53 {
                significand >>= 1;
                shift += 1;
            }
        }
    }
    let mut value = if shift > 1023 {
        f64::INFINITY
    } else {
        (significand as f64) * 2f64.powi(shift as i32)
    };
    if layout_long_sign(tag) < 0 {
        value = -value;
    }
    value
}

/// CPython `PyLong_AsLongAndOverflow`: on out-of-range returns **-1** with
/// `*overflow = ±1` and NO exception (the caller handles the overflow); a
/// non-int dispatches `__index__` and raises TypeError with `*overflow = 0`.
/// The pre-fix body returned a clamped MAX/MIN (divergent) and a silent -1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsLongAndOverflow(
    op: *mut PyObject,
    overflow: *mut c_int,
) -> c_long {
    let mut ov: c_int = 0;
    let result = match checked_signed_value(op, true, c_long::MIN as i64, c_long::MAX as i64) {
        Ok(value) => value as c_long,
        Err(CheckedLongError::Overflow(sign)) => {
            ov = sign as c_int;
            -1
        }
        Err(CheckedLongError::NotInt) => {
            set_long_type_error();
            -1
        }
        Err(CheckedLongError::Raised | CheckedLongError::Negative) => -1,
    };
    if !overflow.is_null() {
        unsafe { *overflow = ov };
    }
    result
}

/// CPython `PyLong_AsSsize_t` (Objects/longobject.c): `PyLong_Check` only — NO
/// `__index__` dispatch; TypeError "an integer is required" for a non-int;
/// OverflowError "Python int too large to convert to C ssize_t" out of range
/// (`isize` is 32-bit on the wasm32 witness — the pre-fix `as isize` silently
/// truncated there, feeding numpy wrong shapes/strides; and its silent -1 was
/// numpy's reshape-"infer" sentinel).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsSsize_t(op: *mut PyObject) -> isize {
    match checked_signed_value(op, false, isize::MIN as i64, isize::MAX as i64) {
        Ok(value) => value as isize,
        Err(CheckedLongError::Overflow(_)) => {
            set_long_overflow_msg(c"Python int too large to convert to C ssize_t");
            -1
        }
        Err(CheckedLongError::NotInt) => {
            set_long_type_error();
            -1
        }
        Err(CheckedLongError::Raised | CheckedLongError::Negative) => -1,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsSize_t(op: *mut PyObject) -> usize {
    match checked_unsigned_value(op, usize::MAX as u64) {
        Ok(value) => value as usize,
        Err(CheckedLongError::Negative) => {
            set_long_overflow_msg(c"can't convert negative value to size_t");
            usize::MAX
        }
        Err(CheckedLongError::Overflow(_)) => {
            set_long_overflow_msg(c"Python int too large to convert to C size_t");
            usize::MAX
        }
        Err(CheckedLongError::NotInt) => {
            set_long_type_error();
            usize::MAX
        }
        Err(CheckedLongError::Raised) => usize::MAX,
    }
}

/// CPython `PyLong_AsLongLong`: `__index__` dispatch, OverflowError
/// "Python int too large to convert to C long long" beyond i64, -1 only with
/// an exception set (was: silent truncation through the unchecked hook).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsLongLong(op: *mut PyObject) -> c_longlong {
    match checked_signed_value(op, true, i64::MIN, i64::MAX) {
        Ok(value) => value as c_longlong,
        Err(CheckedLongError::Overflow(_)) => {
            set_long_overflow_msg(c"Python int too large to convert to C long long");
            -1
        }
        Err(CheckedLongError::NotInt) => {
            set_long_type_error();
            -1
        }
        Err(CheckedLongError::Raised | CheckedLongError::Negative) => -1,
    }
}

/// CPython `PyLong_AsLongLongAndOverflow`: same contract as the `long` variant
/// — **-1** with `*overflow = ±1` (no exception) on out-of-range, `__index__`
/// dispatch for non-int. The pre-fix body returned `c_longlong::MAX` on
/// positive overflow (divergent clamp).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsLongLongAndOverflow(
    op: *mut PyObject,
    overflow: *mut c_int,
) -> c_longlong {
    let mut ov: c_int = 0;
    let result = match checked_signed_value(op, true, i64::MIN, i64::MAX) {
        Ok(value) => value as c_longlong,
        Err(CheckedLongError::Overflow(sign)) => {
            ov = sign as c_int;
            -1
        }
        Err(CheckedLongError::NotInt) => {
            set_long_type_error();
            -1
        }
        Err(CheckedLongError::Raised | CheckedLongError::Negative) => -1,
    };
    if !overflow.is_null() {
        unsafe { *overflow = ov };
    }
    result
}

/// CPython `PyLong_AsUnsignedLong` (Objects/longobject.c): `PyLong_Check` only;
/// TypeError "an integer is required" for non-int; OverflowError
/// "can't convert negative value to unsigned int" for negatives (CPython's
/// historical message says "int" even for `unsigned long`); OverflowError
/// "Python int too large to convert to C unsigned long" past the width. The
/// pre-fix body wrapped negatives/non-ints to huge values with no exception.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsUnsignedLong(op: *mut PyObject) -> c_ulong {
    const SENTINEL: c_ulong = c_ulong::MAX; // (unsigned long)-1
    match checked_unsigned_value(op, c_ulong::MAX as u64) {
        Ok(value) => value as c_ulong,
        Err(CheckedLongError::Negative) => {
            set_long_overflow_msg(c"can't convert negative value to unsigned int");
            SENTINEL
        }
        Err(CheckedLongError::Overflow(_)) => {
            set_long_overflow_msg(c"Python int too large to convert to C unsigned long");
            SENTINEL
        }
        Err(CheckedLongError::NotInt) => {
            set_long_type_error();
            SENTINEL
        }
        Err(CheckedLongError::Raised) => SENTINEL,
    }
}

/// CPython `PyLong_AsUnsignedLongLong`: same strict contract at 64-bit width.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsUnsignedLongLong(op: *mut PyObject) -> c_ulonglong {
    const SENTINEL: c_ulonglong = c_ulonglong::MAX; // (unsigned long long)-1
    match checked_unsigned_value(op, u64::MAX) {
        Ok(value) => value as c_ulonglong,
        Err(CheckedLongError::Negative) => {
            set_long_overflow_msg(c"can't convert negative value to unsigned int");
            SENTINEL
        }
        Err(CheckedLongError::Overflow(_)) => {
            set_long_overflow_msg(c"Python int too large to convert to C unsigned long long");
            SENTINEL
        }
        Err(CheckedLongError::NotInt) => {
            set_long_type_error();
            SENTINEL
        }
        Err(CheckedLongError::Raised) => SENTINEL,
    }
}

fn set_positive_converter_error() {
    unsafe {
        crate::api::errors::PyErr_SetString(
            (&raw mut crate::abi_types::PyExc_ValueError).cast::<crate::abi_types::PyObject>(),
            c"value must be positive".as_ptr(),
        );
    }
}

fn unsigned_converter_value(
    op: *mut PyObject,
    max: u64,
    overflow_message: &'static std::ffi::CStr,
) -> Option<u64> {
    match checked_unsigned_value(op, max) {
        Ok(value) => Some(value),
        Err(CheckedLongError::Negative) => {
            set_positive_converter_error();
            None
        }
        Err(CheckedLongError::Overflow(_)) => {
            set_long_overflow_msg(overflow_message);
            None
        }
        Err(CheckedLongError::NotInt) => {
            set_long_type_error();
            None
        }
        Err(CheckedLongError::Raised) => None,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyLong_Size_t_Converter(op: *mut PyObject, out: *mut c_void) -> c_int {
    if out.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return 0;
    }
    let Some(value) = unsigned_converter_value(
        op,
        usize::MAX as u64,
        c"Python int too large to convert to C size_t",
    ) else {
        return 0;
    };
    unsafe { out.cast::<usize>().write(value as usize) };
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyLong_UnsignedShort_Converter(
    op: *mut PyObject,
    out: *mut c_void,
) -> c_int {
    if out.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return 0;
    }
    let Some(value) = unsigned_converter_value(
        op,
        std::os::raw::c_ushort::MAX as u64,
        c"Python int too large for C unsigned short",
    ) else {
        return 0;
    };
    unsafe { out.cast::<std::os::raw::c_ushort>().write(value as _) };
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyLong_UnsignedInt_Converter(
    op: *mut PyObject,
    out: *mut c_void,
) -> c_int {
    if out.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return 0;
    }
    let Some(value) = unsigned_converter_value(
        op,
        std::os::raw::c_uint::MAX as u64,
        c"Python int too large for C unsigned int",
    ) else {
        return 0;
    };
    unsafe { out.cast::<std::os::raw::c_uint>().write(value as _) };
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyLong_UnsignedLong_Converter(
    op: *mut PyObject,
    out: *mut c_void,
) -> c_int {
    if out.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return 0;
    }
    let Some(value) = unsigned_converter_value(
        op,
        c_ulong::MAX as u64,
        c"Python int too large for C unsigned long",
    ) else {
        return 0;
    };
    unsafe { out.cast::<c_ulong>().write(value as c_ulong) };
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyLong_UnsignedLongLong_Converter(
    op: *mut PyObject,
    out: *mut c_void,
) -> c_int {
    if out.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return 0;
    }
    let Some(value) = unsigned_converter_value(
        op,
        c_ulonglong::MAX,
        c"Python int too large for C unsigned long long",
    ) else {
        return 0;
    };
    unsafe { out.cast::<c_ulonglong>().write(value as c_ulonglong) };
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsUnsignedLongMask(op: *mut PyObject) -> c_ulong {
    match masked_unsigned_value(op, c_ulong::BITS) {
        Ok(value) => value as c_ulong,
        Err(CheckedLongError::NotInt) => {
            set_long_type_error();
            c_ulong::MAX
        }
        Err(CheckedLongError::Raised) => c_ulong::MAX,
        Err(CheckedLongError::Negative | CheckedLongError::Overflow(_)) => unreachable!(),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsUnsignedLongLongMask(op: *mut PyObject) -> c_ulonglong {
    match masked_unsigned_value(op, c_ulonglong::BITS) {
        Ok(value) => value as c_ulonglong,
        Err(CheckedLongError::NotInt) => {
            set_long_type_error();
            c_ulonglong::MAX
        }
        Err(CheckedLongError::Raised) => c_ulonglong::MAX,
        Err(CheckedLongError::Negative | CheckedLongError::Overflow(_)) => unreachable!(),
    }
}

/// CPython `PyLong_AsVoidPtr`: TypeError for non-int, OverflowError when the
/// value does not fit a pointer; NULL is returned only with an exception set
/// (the pre-fix body returned a bare NULL on every failure).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsVoidPtr(op: *mut PyObject) -> *mut c_void {
    match py_long_value(op, false) {
        Ok(LongValue::Signed(v)) if v < 0 => match isize::try_from(v) {
            Ok(v) => v as *mut c_void,
            Err(_) => {
                set_long_overflow_msg(c"Python int too large to convert to C void*");
                ptr::null_mut()
            }
        },
        Ok(LongValue::Signed(v)) => match usize::try_from(v) {
            Ok(v) => v as *mut c_void,
            Err(_) => {
                set_long_overflow_msg(c"Python int too large to convert to C void*");
                ptr::null_mut()
            }
        },
        Ok(LongValue::Big(v)) => match usize::try_from(v) {
            Ok(v) => v as *mut c_void,
            Err(_) => {
                set_long_overflow_msg(c"Python int too large to convert to C void*");
                ptr::null_mut()
            }
        },
        Err(LongError::NotInt) => {
            set_long_type_error();
            ptr::null_mut()
        }
        Ok(LongValue::Wide { .. }) => {
            set_long_overflow_msg(c"Python int too large to convert to C void*");
            ptr::null_mut()
        }
        Err(LongError::Raised) => ptr::null_mut(),
    }
}

fn native_bytes_little_endian(flags: c_int) -> Option<bool> {
    if flags != -1 && (flags < 0 || flags & !0x1f != 0) {
        return None;
    }
    let order = if flags == -1 { 3 } else { flags & 3 };
    match order {
        0 => Some(false),
        1 => Some(true),
        3 => Some(cfg!(target_endian = "little")),
        _ => None,
    }
}

#[inline]
fn inline_int_signed_byte_width(value: i64) -> usize {
    let significant = if value >= 0 {
        65 - value.leading_zeros() as usize
    } else {
        65 - (!value).leading_zeros() as usize
    };
    significant.div_ceil(8)
}

#[inline]
fn inline_int_num_bits(value: i64) -> usize {
    let magnitude = value.unsigned_abs();
    (u64::BITS - magnitude.leading_zeros()) as usize
}

unsafe fn write_inline_native_bytes(
    value: i64,
    buffer: *mut c_void,
    n_bytes: usize,
    little_endian: bool,
) {
    let output = unsafe { std::slice::from_raw_parts_mut(buffer.cast::<u8>(), n_bytes) };
    let raw = value as u64;
    let extension = if value < 0 { 0xff } else { 0 };
    for (index, byte) in output.iter_mut().enumerate() {
        let significance = if little_endian {
            index
        } else {
            n_bytes - index - 1
        };
        *byte = if significance < size_of::<u64>() {
            (raw >> (significance * 8)) as u8
        } else {
            extension
        };
    }
}

fn native_bytes_width_result(required: usize) -> Py_ssize_t {
    match Py_ssize_t::try_from(required) {
        Ok(value) => value,
        Err(_) => {
            set_long_overflow_msg(c"int too large to convert to native bytes");
            -1
        }
    }
}

unsafe fn native_bytes_non_int(
    op: *mut PyObject,
    buffer: *mut c_void,
    n_bytes: Py_ssize_t,
    flags: c_int,
    allow_index: bool,
) -> Py_ssize_t {
    if allow_index {
        let index = unsafe { crate::api::abstract_number::PyNumber_Index(op) };
        if index.is_null() {
            return -1;
        }
        let result = unsafe { PyLong_AsNativeBytes(index, buffer, n_bytes, flags & !16) };
        unsafe { crate::api::refcount::Py_DECREF(index) };
        return result;
    }
    set_long_type_error();
    -1
}

fn set_native_bytes_authority_error() {
    if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
                c"runtime integer native-bytes authority unavailable".as_ptr(),
            );
        }
    }
}

unsafe fn layout_long_as_native_bytes(
    op: *mut PyObject,
    tag: usize,
    buffer: *mut c_void,
    n_bytes: Py_ssize_t,
    little: bool,
    unsigned_buffer: bool,
    reject_negative: bool,
) -> Py_ssize_t {
    let sign = layout_long_sign(tag);
    if sign < 0 && reject_negative {
        set_long_overflow_msg(c"can't convert negative int to unsigned");
        return -1;
    }
    let bits = unsafe { layout_long_num_bits(op, tag) };
    let required = if sign < 0 {
        let base = bits.div_ceil(8).max(1);
        if bits != 0 && bits % 8 == 0 && !unsafe { layout_long_magnitude_is_power_of_two(op, tag) }
        {
            base + 1
        } else {
            base
        }
    } else if unsigned_buffer {
        bits.div_ceil(8).max(1)
    } else {
        bits.saturating_add(1).div_ceil(8).max(1)
    };
    if n_bytes != 0 {
        let rc = unsafe {
            layout_long_as_byte_array(op, tag, buffer.cast(), n_bytes as usize, little as c_int, 1)
        };
        if rc != 0 {
            unsafe { crate::api::errors::PyErr_Clear() };
        }
    }
    native_bytes_width_result(required)
}

/// CPython `PyLong_AsNativeBytes`: returns the number of bytes actually needed
/// (the size-query contract — the pre-fix body always reported 8, so a size
/// probe for the value 5 was told 8), copies min(n_bytes, needed) bytes, and
/// sets an exception on every -1 return.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsNativeBytes(
    op: *mut PyObject,
    buffer: *mut c_void,
    n_bytes: Py_ssize_t,
    flags: c_int,
) -> Py_ssize_t {
    if n_bytes < 0 || buffer.is_null() && n_bytes != 0 {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return -1;
    }
    let Some(little) = native_bytes_little_endian(flags) else {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_ValueError).cast::<crate::abi_types::PyObject>(),
                c"invalid PyLong native-bytes flags".as_ptr(),
            )
        };
        return -1;
    };
    let unsigned_buffer = flags == -1 || flags & 4 != 0;
    let reject_negative = flags != -1 && flags & 8 != 0;
    let allow_index = flags != -1 && flags & 16 != 0;
    if let Some(tag) = unsafe { layout_long_tag(op) } {
        return unsafe {
            layout_long_as_native_bytes(
                op,
                tag,
                buffer,
                n_bytes,
                little,
                unsigned_buffer,
                reject_negative,
            )
        };
    }
    let Some(handle) = resolved_molt_handle(op) else {
        return unsafe { native_bytes_non_int(op, buffer, n_bytes, flags, allow_index) };
    };
    let decoded = handle.decode();
    let inline_value = decoded
        .as_int()
        .or_else(|| decoded.as_bool().map(i64::from));
    if let Some(value) = inline_value {
        if value < 0 && reject_negative {
            set_long_overflow_msg(c"can't convert negative int to unsigned");
            return -1;
        }
        let bits = inline_int_num_bits(value);
        let required = if value < 0 {
            inline_int_signed_byte_width(value)
        } else if unsigned_buffer {
            bits.div_ceil(8).max(1)
        } else {
            bits.saturating_add(1).div_ceil(8).max(1)
        };
        if n_bytes != 0 {
            unsafe {
                write_inline_native_bytes(value, buffer, n_bytes as usize, little);
            }
        }
        return native_bytes_width_result(required);
    }
    let hooks = hooks_or_stubs();
    if !decoded.is_ptr()
        || unsafe { (hooks.classify_heap)(handle.bits()) }
            != crate::abi_types::MoltTypeTag::Int as u8
    {
        return unsafe { native_bytes_non_int(op, buffer, n_bytes, flags, allow_index) };
    }
    let sign = unsafe { (hooks.int_sign)(handle.bits()) };
    if sign < 0 && reject_negative {
        set_long_overflow_msg(c"can't convert negative int to unsigned");
        return -1;
    }
    let mut bits = 0usize;
    if unsafe { (hooks.int_num_bits)(handle.bits(), &raw mut bits) } != 0 {
        set_native_bytes_authority_error();
        return -1;
    }
    let required = if sign < 0 {
        let mut width = 0usize;
        if unsafe { (hooks.int_signed_byte_width)(handle.bits(), &raw mut width) } != 0 {
            set_native_bytes_authority_error();
            return -1;
        }
        width
    } else if unsigned_buffer {
        bits.div_ceil(8).max(1)
    } else {
        bits.saturating_add(1).div_ceil(8).max(1)
    };
    if n_bytes != 0 {
        let status = unsafe {
            (hooks.int_to_bytes)(
                handle.bits(),
                buffer.cast(),
                n_bytes as usize,
                little as c_int,
                1,
            )
        };
        if status == crate::hooks::INT_BYTES_INVALID {
            set_native_bytes_authority_error();
            return -1;
        }
    }
    native_bytes_width_result(required)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromNativeBytes(
    buffer: *const c_void,
    n_bytes: usize,
    flags: c_int,
) -> *mut PyObject {
    if buffer.is_null() && n_bytes != 0 {
        return ptr::null_mut();
    }
    let Some(little) = native_bytes_little_endian(flags) else {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_ValueError).cast::<crate::abi_types::PyObject>(),
                c"invalid PyLong native-bytes flags".as_ptr(),
            )
        };
        return ptr::null_mut();
    };
    let bits =
        unsafe { (hooks_or_stubs().int_from_bytes)(buffer.cast(), n_bytes, little as c_int, 1) };
    if bits == 0 {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
                c"runtime int_from_bytes authority failed".as_ptr(),
            )
        };
        return ptr::null_mut();
    }
    unsafe { crate::bridge::molt_capi_result_to_pyobj(bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromUnsignedNativeBytes(
    buffer: *const c_void,
    n_bytes: usize,
    flags: c_int,
) -> *mut PyObject {
    if buffer.is_null() && n_bytes != 0 {
        return ptr::null_mut();
    }
    let Some(little) = native_bytes_little_endian(flags) else {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_ValueError).cast::<crate::abi_types::PyObject>(),
                c"invalid PyLong native-bytes flags".as_ptr(),
            )
        };
        return ptr::null_mut();
    };
    let bits =
        unsafe { (hooks_or_stubs().int_from_bytes)(buffer.cast(), n_bytes, little as c_int, 0) };
    if bits == 0 {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
                c"runtime int_from_bytes authority failed".as_ptr(),
            )
        };
        return ptr::null_mut();
    }
    unsafe { crate::bridge::molt_capi_result_to_pyobj(bits) }
}

/// CPython `_PyLong_AsInt` / `PyLong_AsInt` (Objects/longobject.c): via
/// `PyLong_AsLongAndOverflow`, so `__index__` dispatches and a non-int raises
/// TypeError; out-of-`int` raises OverflowError
/// "Python int too large to convert to C int". The pre-fix body returned a
/// bare -1 for a non-int (in-range, so indistinguishable from int(-1)).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyLong_AsInt(op: *mut PyObject) -> c_int {
    let mut overflow: c_int = 0;
    let value = unsafe { PyLong_AsLongAndOverflow(op, &raw mut overflow) };
    if value == -1 && overflow == 0 && !unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
        return -1;
    }
    // Widen to i64 so the range test is platform-independent (c_long is 32-bit
    // on Windows/wasm32, 64-bit on LP64) and never an absurd comparison.
    let wide = i64::from(value);
    if overflow != 0 || wide > c_int::MAX as i64 || wide < c_int::MIN as i64 {
        set_long_overflow_msg(c"Python int too large to convert to C int");
        return -1;
    }
    value as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsInt(op: *mut PyObject) -> c_int {
    unsafe { _PyLong_AsInt(op) }
}

fn set_long_overflow() {
    unsafe {
        crate::api::errors::PyErr_SetString(
            (&raw mut crate::abi_types::PyExc_OverflowError).cast::<crate::abi_types::PyObject>(),
            c"int too big to convert".as_ptr(),
        );
    }
}

/// CPython `_PyLong_AsByteArray` (Objects/longobject.c): serializes the int
/// into exactly `n` bytes, raising OverflowError "int too big to convert" when
/// it does not fit and "can't convert negative int to unsigned" for a negative
/// value with `is_signed == 0`. The pre-fix body silently truncated any value
/// beyond 64 bits and returned bare -1 for a non-int; both now raise honestly
/// (values beyond the ±2^64 hook envelope raise OverflowError rather than
/// round-tripping corrupted).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyLong_AsByteArray(
    v: *mut crate::abi_types::PyLongObject,
    bytes: *mut u8,
    n: usize,
    little_endian: c_int,
    is_signed: c_int,
) -> c_int {
    if v.is_null() || (bytes.is_null() && n != 0) {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return -1;
    }
    if let Some(tag) = unsafe { layout_long_tag(v.cast::<PyObject>()) } {
        return unsafe {
            layout_long_as_byte_array(
                v.cast::<PyObject>(),
                tag,
                bytes,
                n,
                little_endian,
                is_signed,
            )
        };
    }
    // Widen to i128 so the (i64::MAX, u64::MAX] band serializes correctly.
    let value: i128 = match py_long_value(v.cast::<PyObject>(), false) {
        Ok(LongValue::Signed(v)) => v as i128,
        Ok(LongValue::Big(v)) => v as i128,
        Err(LongError::NotInt) => {
            set_long_type_error();
            return -1;
        }
        Ok(LongValue::Wide { bits, .. }) => {
            let status = unsafe {
                (hooks_or_stubs().int_to_bytes)(bits, bytes, n, little_endian, is_signed)
            };
            return match status {
                crate::hooks::INT_BYTES_OK => 0,
                crate::hooks::INT_BYTES_NEGATIVE_UNSIGNED => {
                    set_long_overflow_msg(c"can't convert negative int to unsigned");
                    -1
                }
                crate::hooks::INT_BYTES_OVERFLOW => {
                    set_long_overflow();
                    -1
                }
                _ => {
                    if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
                        set_long_overflow_msg(
                            c"runtime arbitrary-width integer authority unavailable",
                        );
                    }
                    -1
                }
            };
        }
        Err(LongError::Raised) => return -1,
    };
    if n == 0 {
        if value == 0 {
            return 0;
        }
        set_long_overflow();
        return -1;
    }
    if value < 0 && is_signed == 0 {
        set_long_overflow_msg(c"can't convert negative int to unsigned");
        return -1;
    }
    let raw = value as u128;
    let fill = if value < 0 { 0xff } else { 0x00 };
    for index in 0..n {
        let source_index = if little_endian != 0 {
            index
        } else {
            n - 1 - index
        };
        let byte = if source_index < 16 {
            ((raw >> (source_index * 8)) & 0xff) as u8
        } else {
            fill
        };
        unsafe {
            *bytes.add(index) = byte;
        }
    }
    let fits = if is_signed != 0 {
        if n >= 16 {
            true
        } else {
            let bits = (n * 8) as u32;
            let min = -(1i128 << (bits - 1));
            let max = (1i128 << (bits - 1)) - 1;
            value >= min && value <= max
        }
    } else if n >= 16 {
        true
    } else {
        value >= 0 && (value as u128) < (1u128 << (n * 8))
    };
    if fits {
        0
    } else {
        set_long_overflow();
        -1
    }
}

// ─── PyFloat ─────────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_GetInfo() -> *mut PyObject {
    let info = unsafe { crate::api::sys::PySys_GetObject(c"int_info".as_ptr()) };
    if info.is_null() {
        return ptr::null_mut();
    }
    unsafe { crate::api::refcount::Py_INCREF(info) };
    info
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyFloat_FromDouble(v: c_double) -> *mut PyObject {
    let bits = MoltObject::from_float(v).bits();
    unsafe { materialize_numeric_owned_handle(bits).0 }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyFloat_GetMax() -> c_double {
    f64::MAX
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyFloat_GetMin() -> c_double {
    f64::MIN_POSITIVE
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyFloat_GetInfo() -> *mut PyObject {
    let info = unsafe { crate::api::sys::PySys_GetObject(c"float_info".as_ptr()) };
    if info.is_null() {
        return ptr::null_mut();
    }
    unsafe { crate::api::refcount::Py_INCREF(info) };
    info
}

fn write_ordered<const N: usize>(data: *mut c_char, bytes: [u8; N], little_endian: c_int) {
    let output = unsafe { std::slice::from_raw_parts_mut(data.cast::<u8>(), N) };
    if little_endian != 0 {
        for (dst, src) in output.iter_mut().zip(bytes.iter().rev()) {
            *dst = *src;
        }
    } else {
        output.copy_from_slice(&bytes);
    }
}

fn read_ordered<const N: usize>(data: *const c_char, little_endian: c_int) -> [u8; N] {
    let input = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), N) };
    let mut bytes = [0u8; N];
    if little_endian != 0 {
        for (dst, src) in bytes.iter_mut().zip(input.iter().rev()) {
            *dst = *src;
        }
    } else {
        bytes.copy_from_slice(input);
    }
    bytes
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyFloat_Pack2(x: c_double, data: *mut c_char, le: c_int) -> c_int {
    let bits = match f64_to_f16_bits(x) {
        Ok(bits) => bits,
        Err(FloatNarrowError::FiniteOverflow) => {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_OverflowError)
                        .cast::<crate::abi_types::PyObject>(),
                    c"float too large to pack with e format".as_ptr(),
                )
            };
            return -1;
        }
    };
    write_ordered(data, bits.to_be_bytes(), le);
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyFloat_Pack4(x: c_double, data: *mut c_char, le: c_int) -> c_int {
    let bits = match f64_to_f32_bits(x) {
        Ok(bits) => bits,
        Err(FloatNarrowError::FiniteOverflow) => {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_OverflowError)
                        .cast::<crate::abi_types::PyObject>(),
                    c"float too large to pack with f format".as_ptr(),
                )
            };
            return -1;
        }
    };
    write_ordered(data, bits.to_be_bytes(), le);
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyFloat_Pack8(x: c_double, data: *mut c_char, le: c_int) -> c_int {
    write_ordered(data, x.to_bits().to_be_bytes(), le);
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyFloat_Unpack2(data: *const c_char, le: c_int) -> c_double {
    f16_bits_to_f64(u16::from_be_bytes(read_ordered(data, le)))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyFloat_Unpack4(data: *const c_char, le: c_int) -> c_double {
    f32_bits_to_f64(u32::from_be_bytes(read_ordered(data, le)))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyFloat_Unpack8(data: *const c_char, le: c_int) -> c_double {
    f64::from_bits(u64::from_be_bytes(read_ordered(data, le)))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyFloat_FromString(v: *mut PyObject) -> *mut PyObject {
    let bytes = match unsafe { py_textlike_bytes(v) } {
        Ok(bytes) => bytes,
        Err(kind) => {
            unsafe {
                set_numeric_parse_error(kind, c"float() argument must be a string-like object")
            };
            return ptr::null_mut();
        }
    };
    match parse_python_float_literal(&bytes) {
        Ok(value) => unsafe { PyFloat_FromDouble(value) },
        Err(kind) => {
            unsafe { set_numeric_parse_error(kind, c"could not convert string to float") };
            ptr::null_mut()
        }
    }
}

/// The `tp_name` of `op`'s type, for CPython-shaped conversion errors. Safe
/// for any non-null object with a readable header.
unsafe fn float_arg_type_name(op: *mut PyObject) -> String {
    let tp = unsafe { (*op).ob_type };
    if tp.is_null() {
        return "<unknown>".to_string();
    }
    let name = unsafe { (*tp).tp_name };
    if name.is_null() {
        "<unknown>".to_string()
    } else {
        unsafe { std::ffi::CStr::from_ptr(name) }
            .to_string_lossy()
            .into_owned()
    }
}

/// CPython `PyFloat_AsDouble` (Objects/floatobject.c): exact float read, else
/// `nb_float` dispatch, else the `nb_index` route, else TypeError
/// "must be real number, not '<type>'" with **-1.0** (never a silent NaN — the
/// pre-fix NaN return poisoned numpy compute paths with fake values).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyFloat_AsDouble(op: *mut PyObject) -> c_double {
    if op.is_null() {
        unsafe { crate::api::errors::PyErr_BadArgument() };
        return -1.0;
    }
    if let Some(value) = unsafe { layout_float_value(op) } {
        return value;
    }
    let resolution = resolve_pyobject(op);
    let op_handle = match resolution {
        Some(ResolvedPyObject::ManagedMolt(handle)) => Some(handle),
        Some(ResolvedPyObject::Foreign) | None => None,
    };
    if let Some(bits) = op_handle {
        let obj = bits.decode();
        if obj.is_float() {
            return obj.as_float().unwrap_or(-1.0);
        }
        if let Some(i) = obj.as_int() {
            return i as f64;
        }
        if obj.is_bool() {
            return obj.as_bool().unwrap_or(false) as i64 as f64;
        }
        if obj.is_ptr() {
            let h = hooks_or_stubs();
            if unsafe { (h.classify_heap)(bits.bits()) } == crate::abi_types::MoltTypeTag::Int as u8
            {
                // Heap bignum: checked 64-bit reads, then the exact runtime
                // conversion authority for the beyond-64-bit band.
                let mut sv = 0i64;
                if unsafe { (h.int_as_i64_checked)(bits.bits(), &raw mut sv) } == 0 {
                    return sv as f64;
                }
                let mut uv = 0u64;
                if unsafe { (h.int_as_u64_checked)(bits.bits(), &raw mut uv) } == 0 {
                    return uv as f64;
                }
                if let Some(v) = big_int_as_f64(bits.bits()) {
                    return v;
                }
                if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
                    set_long_overflow_msg(c"int too large to convert to float");
                }
                return -1.0;
            }
        }
    }
    // Foreign or non-numeric: dispatch `nb_float` (PyNumber_Float's foreign
    // path), then the `nb_index` route, mirroring floatobject.c.
    let protocol_op = op;
    let converted = unsafe { crate::api::abstract_number::PyNumber_Float(protocol_op) };
    if !converted.is_null() {
        let converted_handle = resolved_molt_handle(converted);
        let value = if let Some(bits) = converted_handle {
            bits.decode().as_float()
        } else {
            unsafe { layout_float_value(converted) }
        };
        unsafe { crate::api::refcount::Py_DECREF(converted) };
        if let Some(v) = value {
            return v;
        }
    } else if unsafe { crate::api::abstract_number::PyIndex_Check(protocol_op) } != 0 {
        unsafe { crate::api::errors::PyErr_Clear() };
        let index = unsafe { crate::api::abstract_number::PyNumber_Index(protocol_op) };
        if index.is_null() {
            return -1.0;
        }
        let value = unsafe { PyLong_AsDouble(index) };
        unsafe { crate::api::refcount::Py_DECREF(index) };
        return value;
    }
    // Shape the failure like PyFloat_AsDouble (the pending PyNumber_Float
    // TypeError has CPython's float()-flavored text; PyFloat_AsDouble's is
    // "must be real number"). Preserve any non-TypeError exception verbatim.
    if unsafe {
        crate::api::errors::PyErr_ExceptionMatches(
            (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
        )
    } == 1
        || unsafe { crate::api::errors::PyErr_Occurred() }.is_null()
    {
        let message = format!("must be real number, not '{}'", unsafe {
            float_arg_type_name(protocol_op)
        });
        let cmsg = std::ffi::CString::new(message).unwrap_or_default();
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
                cmsg.as_ptr(),
            );
        }
    }
    -1.0
}

const PY_HASH_INF: isize = 314159;

fn pointer_hash(ptr: *mut PyObject) -> isize {
    let width = usize::BITS;
    let raw = ptr as usize;
    let rotated = (raw >> 4) | (raw << (width - 4));
    let hash = rotated as isize;
    if hash == -1 { -2 } else { hash }
}

unsafe extern "C" {
    fn molt_capi_errno() -> c_int;
    fn molt_capi_set_errno(value: c_int);
}

#[inline]
fn set_c_errno(value: c_int) {
    unsafe { molt_capi_set_errno(value) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _Py_c_sum(a: Py_complex, b: Py_complex) -> Py_complex {
    Py_complex {
        real: a.real + b.real,
        imag: a.imag + b.imag,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _Py_c_diff(a: Py_complex, b: Py_complex) -> Py_complex {
    Py_complex {
        real: a.real - b.real,
        imag: a.imag - b.imag,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _Py_c_neg(a: Py_complex) -> Py_complex {
    Py_complex {
        real: -a.real,
        imag: -a.imag,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _Py_c_prod(a: Py_complex, b: Py_complex) -> Py_complex {
    Py_complex {
        real: a.real * b.real - a.imag * b.imag,
        imag: a.real * b.imag + a.imag * b.real,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _Py_c_quot(a: Py_complex, b: Py_complex) -> Py_complex {
    let abs_real = b.real.abs();
    let abs_imag = b.imag.abs();
    if abs_real >= abs_imag {
        if abs_real == 0.0 {
            set_c_errno(crate::platform::C_EDOM);
            return Py_complex {
                real: 0.0,
                imag: 0.0,
            };
        }
        let ratio = b.imag / b.real;
        let denom = b.real + b.imag * ratio;
        Py_complex {
            real: (a.real + a.imag * ratio) / denom,
            imag: (a.imag - a.real * ratio) / denom,
        }
    } else if abs_imag >= abs_real {
        let ratio = b.real / b.imag;
        let denom = b.real * ratio + b.imag;
        Py_complex {
            real: (a.real * ratio + a.imag) / denom,
            imag: (a.imag * ratio - a.real) / denom,
        }
    } else {
        Py_complex {
            real: f64::NAN,
            imag: f64::NAN,
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _Py_c_pow(a: Py_complex, b: Py_complex) -> Py_complex {
    let saved_errno = unsafe { molt_capi_errno() };
    if b.real == 0.0 && b.imag == 0.0 {
        return Py_complex {
            real: 1.0,
            imag: 0.0,
        };
    }
    if a.real == 0.0 && a.imag == 0.0 {
        if b.imag != 0.0 || b.real < 0.0 {
            set_c_errno(crate::platform::C_EDOM);
        }
        return Py_complex {
            real: 0.0,
            imag: 0.0,
        };
    }
    let magnitude = a.real.hypot(a.imag);
    let angle = a.imag.atan2(a.real);
    let mut len = magnitude.powf(b.real);
    let mut phase = angle * b.real;
    if b.imag != 0.0 {
        len *= (-angle * b.imag).exp();
        phase += b.imag * magnitude.ln();
    }
    let result = Py_complex {
        real: len * phase.cos(),
        imag: len * phase.sin(),
    };
    if a.real.is_finite()
        && a.imag.is_finite()
        && b.real.is_finite()
        && b.imag.is_finite()
        && ((!result.real.is_finite() || !result.imag.is_finite())
            || (result.real == 0.0 && result.imag == 0.0 && len == 0.0))
    {
        set_c_errno(crate::platform::C_ERANGE);
    } else {
        set_c_errno(saved_errno);
    }
    result
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _Py_c_abs(value: Py_complex) -> c_double {
    if value.real.is_infinite() {
        set_c_errno(0);
        return value.real.abs();
    }
    if value.imag.is_infinite() {
        set_c_errno(0);
        return value.imag.abs();
    }
    if value.real.is_nan() || value.imag.is_nan() {
        return f64::NAN;
    }
    let result = value.real.hypot(value.imag);
    set_c_errno(if result.is_finite() {
        0
    } else {
        crate::platform::C_ERANGE
    });
    result
}

fn frexp_abs(value: f64) -> (f64, i32) {
    if value == 0.0 {
        return (0.0, 0);
    }
    let bits = value.to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as i32;
    let mantissa = bits & ((1u64 << 52) - 1);
    if exponent == 0 {
        let (m, e) = frexp_abs(value * ((1u64 << 54) as f64));
        return (m, e - 54);
    }
    let m = f64::from_bits((1022u64 << 52) | mantissa);
    (m, exponent - 1022)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _Py_HashDouble(inst: *mut PyObject, v: c_double) -> isize {
    if v.is_infinite() {
        return if v.is_sign_positive() {
            PY_HASH_INF
        } else {
            -PY_HASH_INF
        };
    }
    if v.is_nan() {
        return pointer_hash(inst);
    }

    let sign = if v.is_sign_negative() { -1 } else { 1 };
    let (mut mantissa, mut exponent) = frexp_abs(v.abs());
    let hash_bits = if std::mem::size_of::<isize>() >= 8 {
        61u32
    } else {
        31u32
    };
    let modulus = (1u64 << hash_bits) - 1;
    let mut hash = 0u64;

    while mantissa != 0.0 {
        hash = ((hash << 28) & modulus) | (hash >> (hash_bits - 28));
        mantissa *= 268_435_456.0;
        exponent -= 28;
        let chunk = mantissa as u64;
        mantissa -= chunk as f64;
        hash += chunk;
        if hash >= modulus {
            hash -= modulus;
        }
    }

    let rotate = if exponent >= 0 {
        (exponent as u32) % hash_bits
    } else {
        hash_bits - 1 - ((-1 - exponent) as u32 % hash_bits)
    };
    if rotate != 0 {
        hash = ((hash << rotate) & modulus) | (hash >> (hash_bits - rotate));
    }

    let signed = if sign < 0 {
        -(hash as isize)
    } else {
        hash as isize
    };
    if signed == -1 { -2 } else { signed }
}

unsafe fn allocate_complex_carrier(real: c_double, imag: c_double, bits: u64) -> *mut PyObject {
    let obj = Box::new(PyComplexObject {
        ob_base: PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut crate::abi_types::PyComplex_Type,
        },
        cval: Py_complex { real, imag },
    });
    let ptr = Box::into_raw(obj).cast::<PyObject>();
    GLOBAL_BRIDGE.register_numeric_carrier(
        ptr,
        Some(bits),
        crate::bridge::NumericCarrierKind::Complex,
    );
    ptr
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyComplex_FromDoubles(real: c_double, imag: c_double) -> *mut PyObject {
    let bits = match unsafe { (hooks_or_stubs().complex_from_doubles)(real, imag) }.decode() {
        crate::hooks::DecodedHandleResult::Ok(bits) => bits,
        crate::hooks::DecodedHandleResult::Missing | crate::hooks::DecodedHandleResult::Error => {
            if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
                unsafe {
                    crate::api::errors::PyErr_SetString(
                        (&raw mut crate::abi_types::PyExc_SystemError)
                            .cast::<crate::abi_types::PyObject>(),
                        c"complex constructor runtime authority unavailable".as_ptr(),
                    )
                };
            }
            return ptr::null_mut();
        }
    };
    unsafe { allocate_complex_carrier(real, imag, bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyComplex_FromCComplex(value: Py_complex) -> *mut PyObject {
    unsafe { PyComplex_FromDoubles(value.real, value.imag) }
}

pub unsafe extern "C" fn molt_complex_dealloc(op: *mut PyObject) {
    if op.is_null() {
        return;
    }
    let Some(record) = GLOBAL_BRIDGE.unregister_numeric_carrier(op) else {
        eprintln!(
            "molt fatal: complex carrier dealloc missing allocation provenance ptr={:p}",
            op
        );
        std::process::abort();
    };
    if let Some(bits) = record.bits
        && MoltObject::from_bits(bits).is_ptr()
    {
        unsafe { (hooks_or_stubs().dec_ref)(bits) };
    }
    unsafe { drop(Box::from_raw(op.cast::<PyComplexObject>())) };
}

/// Build a layout-compatible CPython numeric carrier for a runtime handle.
/// Foreign numeric slots are allowed to read builtin fields directly, so the
/// generic BridgeHeader proxy must never be exposed as a long/float/complex.
/// The boolean return says whether the caller owns a temporary C reference.
pub(crate) unsafe fn materialize_numeric_owned_handle(bits: u64) -> (*mut PyObject, bool) {
    let result = unsafe { materialize_numeric_carrier(bits) };
    if result.0.is_null() && MoltObject::from_bits(bits).is_ptr() {
        unsafe { (hooks_or_stubs().dec_ref)(bits) };
    }
    result
}

pub(crate) unsafe fn materialize_numeric_borrowed_handle(bits: u64) -> (*mut PyObject, bool) {
    if MoltObject::from_bits(bits).is_ptr() {
        unsafe { (hooks_or_stubs().inc_ref)(bits) };
    }
    unsafe { materialize_numeric_owned_handle(bits) }
}

pub(crate) fn is_numeric_handle(bits: u64) -> bool {
    let value = MoltObject::from_bits(bits);
    value.is_bool()
        || value.is_int()
        || value.is_float()
        || value.is_ptr()
            && matches!(
                unsafe { (hooks_or_stubs().classify_heap)(bits) },
                tag if tag == crate::abi_types::MoltTypeTag::Int as u8
                    || tag == crate::abi_types::MoltTypeTag::Complex as u8
            )
}

unsafe fn materialize_numeric_carrier(bits: u64) -> (*mut PyObject, bool) {
    let obj = MoltObject::from_bits(bits);
    if obj.is_bool() {
        let ptr = if obj.as_bool().unwrap_or(false) {
            (&raw mut Py_True).cast::<PyObject>()
        } else {
            (&raw mut Py_False).cast::<PyObject>()
        };
        return (ptr, false);
    }
    if let Some(value) = obj.as_int()
        && let Some(ptr) = cached_small_int_ptr(value)
    {
        return (ptr, false);
    }
    if let Some(value) = obj.as_float() {
        let carrier = Box::new(PyFloatObject {
            ob_base: PyObject {
                ob_refcnt: 1,
                ob_type: &raw mut crate::abi_types::PyFloat_Type,
            },
            ob_fval: value,
        });
        let ptr = Box::into_raw(carrier).cast();
        GLOBAL_BRIDGE.register_numeric_carrier(
            ptr,
            Some(bits),
            crate::bridge::NumericCarrierKind::Float,
        );
        return (ptr, true);
    }
    if let Some(value) = obj.as_int() {
        return unsafe { materialize_long_carrier(bits, value.signum() as i32) };
    }
    if obj.is_ptr() {
        let hooks = hooks_or_stubs();
        match unsafe { (hooks.classify_heap)(bits) } {
            tag if tag == crate::abi_types::MoltTypeTag::Int as u8 => {
                return unsafe { materialize_long_carrier(bits, (hooks.int_sign)(bits)) };
            }
            tag if tag == crate::abi_types::MoltTypeTag::Complex as u8 => {
                let mut real = 0.0;
                let mut imag = 0.0;
                if unsafe { (hooks.complex_parts)(bits, &raw mut real, &raw mut imag) } == 0 {
                    let ptr = unsafe { allocate_complex_carrier(real, imag, bits) };
                    return (ptr, true);
                }
            }
            _ => {}
        }
    }
    (ptr::null_mut(), false)
}

unsafe fn materialize_long_carrier(bits: u64, sign: i32) -> (*mut PyObject, bool) {
    let hooks = hooks_or_stubs();
    let inline_value = MoltObject::from_bits(bits).as_int();
    let bit_len = if let Some(value) = inline_value {
        value
            .unsigned_abs()
            .checked_ilog2()
            .map_or(0, |n| n as usize + 1)
    } else {
        let mut bit_len = 0usize;
        if unsafe { (hooks.int_num_bits)(bits, &raw mut bit_len) } != 0 {
            return (ptr::null_mut(), false);
        }
        bit_len
    };
    if !matches!(sign, -1..=1) || sign == 0 && bit_len != 0 {
        return (ptr::null_mut(), false);
    }
    // Inline integers already fit in one machine word. Construct their base-
    // 2^30 digits directly and reserve scratch bytes only for arbitrary-width
    // runtime integers. This removes the second allocation from the common
    // non-cached PyLong path.
    let mut heap_magnitude = if inline_value.is_none() {
        let Some(byte_len) = bit_len.div_ceil(8).checked_add(1) else {
            unsafe { crate::api::errors::PyErr_NoMemory() };
            return (ptr::null_mut(), false);
        };
        let byte_len = byte_len.max(1);
        let mut magnitude = Vec::new();
        if magnitude.try_reserve_exact(byte_len).is_err() {
            unsafe { crate::api::errors::PyErr_NoMemory() };
            return (ptr::null_mut(), false);
        }
        magnitude.resize(byte_len, 0);
        let status = unsafe { (hooks.int_to_bytes)(bits, magnitude.as_mut_ptr(), byte_len, 1, 1) };
        if status != crate::hooks::INT_BYTES_OK {
            return (ptr::null_mut(), false);
        }
        if sign < 0 {
            let mut carry = 1u16;
            for byte in &mut magnitude {
                let next = ((!*byte) as u16) + carry;
                *byte = next as u8;
                carry = next >> 8;
            }
        }
        while magnitude.len() > 1 && magnitude.last() == Some(&0) {
            magnitude.pop();
        }
        Some(magnitude)
    } else {
        None
    };
    let digits = if sign == 0 {
        0
    } else {
        bit_len.div_ceil(PYLONG_BITS_IN_DIGIT)
    };
    let digit_slots = digits.max(1);
    let Some(size) = digit_slots
        .checked_mul(std::mem::size_of::<u32>())
        .and_then(|digits| digits.checked_add(std::mem::size_of::<usize>()))
        .and_then(|tail| tail.checked_add(std::mem::size_of::<PyObject>()))
    else {
        unsafe { crate::api::errors::PyErr_NoMemory() };
        return (ptr::null_mut(), false);
    };
    let layout = std::alloc::Layout::from_size_align(size, std::mem::align_of::<usize>())
        .expect("PyLong carrier layout");
    let raw = unsafe { std::alloc::alloc_zeroed(layout) };
    if raw.is_null() {
        return (ptr::null_mut(), false);
    }
    let op = raw.cast::<PyObject>();
    unsafe {
        std::ptr::write(
            op,
            PyObject {
                ob_refcnt: 1,
                ob_type: &raw mut crate::abi_types::PyLong_Type,
            },
        );
        let tag_ptr = raw.add(std::mem::size_of::<PyObject>()).cast::<usize>();
        tag_ptr.write(
            (digits << 3)
                | if sign < 0 {
                    PYLONG_NEGATIVE_TAG
                } else if sign == 0 {
                    PYLONG_ZERO_TAG
                } else {
                    0
                },
        );
        let digit_ptr = tag_ptr.add(1).cast::<u32>();
        for digit_index in 0..digits {
            let digit = if let Some(value) = inline_value {
                ((value.unsigned_abs() >> (digit_index * PYLONG_BITS_IN_DIGIT))
                    & ((1_u64 << PYLONG_BITS_IN_DIGIT) - 1)) as u32
            } else {
                let magnitude = heap_magnitude
                    .as_mut()
                    .expect("heap PyLong carrier missing magnitude bytes");
                let mut digit = 0u32;
                for bit in 0..PYLONG_BITS_IN_DIGIT {
                    let source_bit = digit_index * PYLONG_BITS_IN_DIGIT + bit;
                    let byte = source_bit / 8;
                    if byte < magnitude.len() && magnitude[byte] & (1 << (source_bit % 8)) != 0 {
                        digit |= 1 << bit;
                    }
                }
                digit
            };
            digit_ptr.add(digit_index).write(digit);
        }
    }
    GLOBAL_BRIDGE.register_numeric_carrier(
        op,
        Some(bits),
        crate::bridge::NumericCarrierKind::Long {
            allocation_size: size,
        },
    );
    (op, true)
}

pub unsafe extern "C" fn molt_numeric_scalar_dealloc(op: *mut PyObject) {
    if op.is_null() {
        return;
    }
    let Some(record) = GLOBAL_BRIDGE.unregister_numeric_carrier(op) else {
        eprintln!(
            "molt fatal: numeric carrier dealloc missing allocation provenance ptr={:p}",
            op
        );
        std::process::abort();
    };
    if let Some(bits) = record.bits
        && MoltObject::from_bits(bits).is_ptr()
    {
        unsafe { (hooks_or_stubs().dec_ref)(bits) };
    }
    match record.kind {
        crate::bridge::NumericCarrierKind::Float => {
            unsafe { drop(Box::from_raw(op.cast::<PyFloatObject>())) };
        }
        crate::bridge::NumericCarrierKind::Long { allocation_size } => {
            let layout =
                std::alloc::Layout::from_size_align(allocation_size, std::mem::align_of::<usize>())
                    .expect("PyLong carrier layout");
            unsafe { std::alloc::dealloc(op.cast(), layout) };
        }
        crate::bridge::NumericCarrierKind::Complex => {
            unsafe { drop(Box::from_raw(op.cast::<PyComplexObject>())) };
        }
    }
}

fn runtime_complex_parts(op: *mut PyObject) -> Option<Py_complex> {
    let bits = resolved_molt_handle(op)?.bits();
    let hooks = hooks_or_stubs();
    if unsafe { (hooks.classify_heap)(bits) } != crate::abi_types::MoltTypeTag::Complex as u8 {
        return None;
    }
    let mut real = 0.0;
    let mut imag = 0.0;
    (unsafe { (hooks.complex_parts)(bits, &raw mut real, &raw mut imag) } == 0)
        .then_some(Py_complex { real, imag })
}

/// CPython `PyComplex_AsCComplex` (Objects/complexobject.c): a real complex
/// reads directly; otherwise call `__complex__`, validate that it returned a
/// complex instance, then fall back to `PyFloat_AsDouble` for the real part.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyComplex_AsCComplex(op: *mut PyObject) -> Py_complex {
    if op.is_null() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
                c"complex argument is NULL".as_ptr(),
            );
        }
        return Py_complex {
            real: -1.0,
            imag: 0.0,
        };
    }
    if let Some(value) = unsafe { layout_complex_value(op) } {
        return value;
    }
    if let Some(value) = runtime_complex_parts(op) {
        return value;
    }
    if let Some(handle) = resolved_molt_handle(op) {
        let value = handle.decode();
        let is_builtin_real = value.is_int()
            || value.is_bool()
            || value.is_float()
            || value.is_ptr()
                && unsafe { (hooks_or_stubs().classify_heap)(handle.bits()) }
                    == crate::abi_types::MoltTypeTag::Int as u8;
        if is_builtin_real {
            return Py_complex {
                real: unsafe { PyFloat_AsDouble(op) },
                imag: 0.0,
            };
        }
    }
    match unsafe { crate::api::object::call_optional_special_noargs(op, c"__complex__".as_ptr()) } {
        Err(()) => {
            return Py_complex {
                real: -1.0,
                imag: 0.0,
            };
        }
        Ok(Some(result)) => {
            if unsafe { PyComplex_Check(result) } == 0 {
                let type_name = unsafe { crate::api::object::type_name_lossy(result) };
                let message = std::ffi::CString::new(format!(
                    "__complex__ returned non-complex (type {type_name})"
                ))
                .unwrap_or_default();
                unsafe {
                    crate::api::errors::PyErr_SetString(
                        (&raw mut crate::abi_types::PyExc_TypeError)
                            .cast::<crate::abi_types::PyObject>(),
                        message.as_ptr(),
                    );
                    crate::api::refcount::Py_DECREF(result);
                }
                return Py_complex {
                    real: -1.0,
                    imag: 0.0,
                };
            }
            let value = if let Some(value) = runtime_complex_parts(result) {
                value
            } else {
                let cval = unsafe { &raw const (*result.cast::<PyComplexObject>()).cval };
                unsafe { std::ptr::read_unaligned(cval) }
            };
            unsafe { crate::api::refcount::Py_DECREF(result) };
            return value;
        }
        Ok(None) => {}
    }
    let real = unsafe { PyFloat_AsDouble(op) };
    if real == -1.0 && !unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
        return Py_complex {
            real: -1.0,
            imag: 0.0,
        };
    }
    Py_complex { real, imag: 0.0 }
}

/// CPython `PyComplex_RealAsDouble`: the real part of a complex, else
/// `PyFloat_AsDouble` (Objects/complexobject.c).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyComplex_RealAsDouble(op: *mut PyObject) -> c_double {
    if let Some(value) = unsafe { layout_complex_value(op) } {
        return value.real;
    }
    if let Some(value) = runtime_complex_parts(op) {
        return value.real;
    }
    unsafe { PyFloat_AsDouble(op) }
}

/// CPython `PyComplex_ImagAsDouble`: the imag part of a complex, else **0.0
/// with NO error and no conversion attempt** (Objects/complexobject.c). The
/// pre-fix delegation to `PyComplex_AsCComplex` left a live TypeError while
/// returning 0.0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyComplex_ImagAsDouble(op: *mut PyObject) -> c_double {
    if let Some(value) = unsafe { layout_complex_value(op) } {
        return value.imag;
    }
    if let Some(value) = runtime_complex_parts(op) {
        return value.imag;
    }
    0.0
}

/// CPython `PyComplex_Check` (Include/complexobject.h): `PyObject_TypeCheck`
/// semantics — exact type OR any subclass via the subtype walk, not the
/// pre-fix exact-pointer identity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyComplex_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    if unsafe { has_layout_complex(op) } {
        return 1;
    }
    if let Some(value) = resolved_molt_handle(op) {
        return (unsafe { (hooks_or_stubs().classify_heap)(value.bits()) }
            == crate::abi_types::MoltTypeTag::Complex as u8) as c_int;
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyComplex_CheckExact(op: *mut PyObject) -> c_int {
    if !op.is_null()
        && std::ptr::eq(
            unsafe { (*op).ob_type },
            &raw const crate::abi_types::PyComplex_Type,
        )
    {
        return 1;
    }
    if let Some(value) = resolved_molt_handle(op) {
        return (value.decode().is_ptr()
            && unsafe { (hooks_or_stubs().classify_heap)(value.bits()) }
                == crate::abi_types::MoltTypeTag::Complex as u8) as c_int;
    }
    0
}

// ─── PyBool ──────────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBool_FromLong(v: c_long) -> *mut PyObject {
    if v != 0 {
        (&raw mut Py_True).cast::<PyObject>()
    } else {
        (&raw mut Py_False).cast::<PyObject>()
    }
}

// ─── Type checks (PyLong_Check etc.) ─────────────────────────────────────────

macro_rules! type_check {
    ($name:ident, $pred:ident) => {
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(op: *mut PyObject) -> c_int {
            if op.is_null() {
                return 0;
            }
            let op_handle = resolved_molt_handle(op);
            match op_handle {
                Some(value) => value.decode().$pred() as c_int,
                None => 0,
            }
        }
    };
}

/// CPython `PyLong_Check` (Include/longobject.h): `Py_TPFLAGS_LONG_SUBCLASS`
/// semantics — true for int, **bool** (an int subtype: `PyLong_Check(True)`
/// is 1), heap bignums, and foreign int subclasses via the subtype walk. The
/// pre-fix `is_int()` answered 0 for bool AND for heap bignums.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    if unsafe { has_layout_long(op) } {
        return 1;
    }
    let op_handle = resolved_molt_handle(op);
    if let Some(value) = op_handle {
        let bits = value.bits();
        let obj = value.decode();
        if obj.is_int() || obj.is_bool() {
            return 1;
        }
        if obj.is_ptr() {
            return (unsafe { (hooks_or_stubs().classify_heap)(bits) }
                == crate::abi_types::MoltTypeTag::Int as u8) as c_int;
        }
        return 0;
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_CheckExact(op: *mut PyObject) -> c_int {
    if !op.is_null()
        && std::ptr::eq(
            unsafe { (*op).ob_type },
            &raw const crate::abi_types::PyLong_Type,
        )
    {
        return 1;
    }
    if let Some(value) = resolved_molt_handle(op) {
        let object = value.decode();
        if object.is_bool() {
            return 0;
        }
        return (object.is_int()
            || object.is_ptr()
                && unsafe { (hooks_or_stubs().classify_heap)(value.bits()) }
                    == crate::abi_types::MoltTypeTag::Int as u8) as c_int;
    }
    0
}

/// CPython `PyFloat_Check` (Include/floatobject.h): float plus foreign float
/// subclasses (numpy `float64` subclasses `float`) via the subtype walk.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyFloat_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    if unsafe { has_layout_float(op) } {
        return 1;
    }
    let op_handle = resolved_molt_handle(op);
    if let Some(value) = op_handle {
        return value.decode().is_float() as c_int;
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyFloat_CheckExact(op: *mut PyObject) -> c_int {
    if !op.is_null()
        && std::ptr::eq(
            unsafe { (*op).ob_type },
            &raw const crate::abi_types::PyFloat_Type,
        )
    {
        return 1;
    }
    if let Some(value) = resolved_molt_handle(op) {
        return value.decode().is_float() as c_int;
    }
    0
}

type_check!(PyBool_Check, is_bool);

/// CPython `PyNumber_Check` (Objects/abstract.c): true when the type provides
/// `nb_index`/`nb_int`/`nb_float` or is complex — including foreign C objects,
/// whose slots the pre-fix bridged-only test never consulted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    // Scalar layout carriers are real public builtin objects even when their
    // canonical slot tables have not yet been consulted.  Check physical
    // identity before bridge resolution so a carrier backed by a Molt handle
    // cannot fall into the generic heap classifier and lose Complex.
    let physical_type = unsafe { (*op).ob_type };
    if std::ptr::eq(physical_type, &raw const crate::abi_types::PyLong_Type)
        || std::ptr::eq(physical_type, &raw const crate::abi_types::PyBool_Type)
        || std::ptr::eq(physical_type, &raw const crate::abi_types::PyFloat_Type)
        || std::ptr::eq(physical_type, &raw const crate::abi_types::PyComplex_Type)
    {
        return 1;
    }
    if unsafe { has_layout_long(op) || has_layout_float(op) || has_layout_complex(op) } {
        return 1;
    }
    let op_handle = resolved_molt_handle(op);
    if let Some(value) = op_handle {
        let bits = value.bits();
        let obj = value.decode();
        if obj.is_int() || obj.is_float() || obj.is_bool() {
            return 1;
        }
        if obj.is_ptr() {
            let tag = unsafe { (hooks_or_stubs().classify_heap)(bits) };
            if tag == crate::abi_types::MoltTypeTag::Int as u8
                || tag == crate::abi_types::MoltTypeTag::Float as u8
                || tag == crate::abi_types::MoltTypeTag::Complex as u8
            {
                return 1;
            }
        }
        return 0;
    }
    if unsafe { PyComplex_Check(op) } != 0 {
        return 1;
    }
    let tp = unsafe { (*op).ob_type };
    if tp.is_null() {
        return 0;
    }
    let num = unsafe { (*tp).tp_as_number }.cast::<crate::abi_types::PyNumberMethods>();
    if num.is_null() {
        return 0;
    }
    (!unsafe { (*num).nb_index }.is_null()
        || !unsafe { (*num).nb_int }.is_null()
        || !unsafe { (*num).nb_float }.is_null()) as c_int
}

#[cfg(test)]
mod tests {
    use super::{
        NumericParseError, PyLong_AsLongLong, PyLong_FromLong, cached_small_int_bits_from_ptr,
        cached_small_int_ptr, parse_python_float_literal, parse_python_int_literal,
    };
    use crate::abi_types::{IMMORTAL_REFCNT, is_immortal_refcnt};
    use crate::api::refcount::Py_DECREF;
    use molt_lang_obj_model::MoltObject;

    #[test]
    fn parses_python_int_literals_with_base_prefixes_and_underscores() {
        assert_eq!(parse_python_int_literal(b"  +1_024  ", 10), Ok(1024));
        assert_eq!(parse_python_int_literal(b"0xff", 0), Ok(255));
        assert_eq!(parse_python_int_literal(b"-0b101", 0), Ok(-5));
        assert_eq!(
            parse_python_int_literal(b"-9223372036854775808", 10),
            Ok(i64::MIN as i128)
        );
    }

    #[test]
    fn rejects_invalid_or_overflowing_python_int_literals() {
        assert_eq!(
            parse_python_int_literal(b"1__0", 10),
            Err(NumericParseError::InvalidLiteral)
        );
        assert_eq!(
            parse_python_int_literal(b"10", 1),
            Err(NumericParseError::InvalidBase)
        );
        assert_eq!(
            parse_python_int_literal(b"18446744073709551616", 10),
            Err(NumericParseError::Overflow)
        );
    }

    #[test]
    fn parses_python_float_literals_with_special_values_and_underscores() {
        assert_eq!(parse_python_float_literal(b"  1_024.5  "), Ok(1024.5));
        assert!(parse_python_float_literal(b"nan").unwrap().is_nan());
        assert_eq!(
            parse_python_float_literal(b"-Infinity"),
            Ok(f64::NEG_INFINITY)
        );
    }

    #[test]
    fn rejects_invalid_python_float_literals() {
        assert_eq!(
            parse_python_float_literal(b"1__0.0"),
            Err(NumericParseError::InvalidLiteral)
        );
        assert_eq!(
            parse_python_float_literal("π".as_bytes()),
            Err(NumericParseError::InvalidLiteral)
        );
    }

    #[test]
    fn small_integer_family_has_one_immortal_bidirectional_authority() {
        for value in [-5_i64, 0, 1, 101, 256] {
            let first = unsafe { PyLong_FromLong(value as _) };
            let second = unsafe { PyLong_FromLong(value as _) };
            assert_eq!(first, second, "cached identity drifted for {value}");
            assert_eq!(cached_small_int_ptr(value), Some(first));
            assert_eq!(
                cached_small_int_bits_from_ptr(first),
                MoltObject::try_from_int(value).map(MoltObject::bits)
            );
            assert_eq!(unsafe { PyLong_AsLongLong(first) }, value as _);
            assert_eq!(unsafe { (*first).ob_refcnt }, IMMORTAL_REFCNT);
            assert!(is_immortal_refcnt(unsafe { (*first).ob_refcnt }));
            unsafe {
                Py_DECREF(first);
                Py_DECREF(second);
            }
            assert_eq!(unsafe { (*first).ob_refcnt }, IMMORTAL_REFCNT);
        }

        let below_a = unsafe { PyLong_FromLong(-6) };
        let below_b = unsafe { PyLong_FromLong(-6) };
        let above_a = unsafe { PyLong_FromLong(257) };
        let above_b = unsafe { PyLong_FromLong(257) };
        assert_ne!(below_a, below_b, "-6 must retain ordinary CPython identity");
        assert_ne!(
            above_a, above_b,
            "257 must retain ordinary CPython identity"
        );
        assert_eq!(cached_small_int_bits_from_ptr(below_a), None);
        assert_eq!(cached_small_int_bits_from_ptr(above_a), None);
        unsafe {
            Py_DECREF(below_a);
            Py_DECREF(below_b);
            Py_DECREF(above_a);
            Py_DECREF(above_b);
        }
    }

    #[test]
    fn small_integer_identity_is_deterministic_across_threads() {
        let expected = cached_small_int_ptr(101).unwrap() as usize;
        let workers = (0..8)
            .map(|_| {
                std::thread::spawn(move || {
                    for _ in 0..4096 {
                        let ptr = unsafe { PyLong_FromLong(101) };
                        assert_eq!(ptr as usize, expected);
                        unsafe { Py_DECREF(ptr) };
                    }
                })
            })
            .collect::<Vec<_>>();
        for worker in workers {
            worker.join().unwrap();
        }
    }
}
