//! Numeric type bridge — PyLong_*, PyFloat_*, PyBool_*.

use crate::abi_types::{Py_False, Py_True, Py_complex, Py_ssize_t, PyComplexObject, PyObject};
use crate::bridge::GLOBAL_BRIDGE;
use crate::hooks::hooks_or_stubs;
use molt_lang_obj_model::MoltObject;
use std::ffi::c_void;
use std::os::raw::{c_char, c_double, c_int, c_long, c_longlong, c_ulong, c_ulonglong};
use std::ptr;

// ─── PyLong ──────────────────────────────────────────────────────────────────

fn py_long_from_i64(v: i64) -> *mut PyObject {
    let bits = MoltObject::try_from_int(v)
        .map(MoltObject::bits)
        .unwrap_or_else(|| unsafe { (hooks_or_stubs().int_from_i64)(v) });
    if bits == 0 {
        return ptr::null_mut();
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

fn py_long_from_u64(v: u64) -> *mut PyObject {
    let bits = MoltObject::try_from_uint(v)
        .map(MoltObject::bits)
        .unwrap_or_else(|| unsafe { (hooks_or_stubs().int_from_u64)(v) });
    if bits == 0 {
        return ptr::null_mut();
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

fn py_long_bits(op: *mut PyObject) -> Option<u64> {
    if op.is_null() {
        return None;
    }
    GLOBAL_BRIDGE.lock().pyobj_to_handle(op)
}

fn py_long_as_i64_checked(op: *mut PyObject) -> Option<i64> {
    let bits = py_long_bits(op)?;
    let obj = MoltObject::from_bits(bits);
    if obj.is_int() {
        obj.as_int()
    } else if obj.is_bool() {
        obj.as_bool().map(|b| if b { 1 } else { 0 })
    } else if obj.is_ptr() {
        let mut value = 0i64;
        let rc = unsafe { (hooks_or_stubs().int_as_i64_checked)(bits, &raw mut value) };
        (rc == 0).then_some(value)
    } else {
        None
    }
}

fn py_long_as_u64_checked(op: *mut PyObject) -> Option<u64> {
    let bits = py_long_bits(op)?;
    let obj = MoltObject::from_bits(bits);
    if obj.is_int() {
        obj.as_int()
            .and_then(|value| (value >= 0).then_some(value as u64))
    } else if obj.is_bool() {
        obj.as_bool().map(u64::from)
    } else if obj.is_ptr() {
        let mut value = 0u64;
        let rc = unsafe { (hooks_or_stubs().int_as_u64_checked)(bits, &raw mut value) };
        (rc == 0).then_some(value)
    } else {
        None
    }
}

fn py_long_as_i64(op: *mut PyObject) -> i64 {
    match py_long_bits(op) {
        Some(bits) => {
            let obj = MoltObject::from_bits(bits);
            if obj.is_int() {
                obj.as_int().unwrap_or(-1)
            } else if obj.is_bool() {
                obj.as_bool().map(|b| if b { 1 } else { 0 }).unwrap_or(-1)
            } else if obj.is_ptr() {
                unsafe { (hooks_or_stubs().int_as_i64)(bits) }
            } else {
                -1
            }
        }
        None => -1,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NumericParseError {
    InvalidBase,
    InvalidLiteral,
    Overflow,
    Type,
}

fn ascii_digit_value(byte: u8) -> Option<u32> {
    match byte {
        b'0'..=b'9' => Some((byte - b'0') as u32),
        b'a'..=b'z' => Some((byte - b'a' + 10) as u32),
        b'A'..=b'Z' => Some((byte - b'A' + 10) as u32),
        _ => None,
    }
}

fn strip_base_prefix(bytes: &[u8], base: u32) -> &[u8] {
    if bytes.len() >= 2 && bytes[0] == b'0' {
        match (bytes[1], base) {
            (b'x' | b'X', 16) | (b'o' | b'O', 8) | (b'b' | b'B', 2) => &bytes[2..],
            _ => bytes,
        }
    } else {
        bytes
    }
}

fn parse_python_int_literal(bytes: &[u8], base_arg: c_int) -> Result<i128, NumericParseError> {
    if base_arg != 0 && !(2..=36).contains(&base_arg) {
        return Err(NumericParseError::InvalidBase);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| NumericParseError::InvalidLiteral)?;
    let mut body = text.trim().as_bytes();
    if body.is_empty() {
        return Err(NumericParseError::InvalidLiteral);
    }

    let negative = match body[0] {
        b'+' => {
            body = &body[1..];
            false
        }
        b'-' => {
            body = &body[1..];
            true
        }
        _ => false,
    };
    if body.is_empty() {
        return Err(NumericParseError::InvalidLiteral);
    }

    let mut base = base_arg as u32;
    if base == 0 {
        base = if body.len() >= 2 && body[0] == b'0' {
            match body[1] {
                b'x' | b'X' => 16,
                b'o' | b'O' => 8,
                b'b' | b'B' => 2,
                _ => 10,
            }
        } else {
            10
        };
    }
    body = strip_base_prefix(body, base);

    let limit: u128 = if negative {
        (i64::MAX as u128) + 1
    } else {
        u64::MAX as u128
    };
    let mut value = 0u128;
    let mut saw_digit = false;
    let mut previous_digit = false;
    let mut previous_underscore = false;
    for &byte in body {
        if byte == b'_' {
            if !previous_digit || previous_underscore {
                return Err(NumericParseError::InvalidLiteral);
            }
            previous_digit = false;
            previous_underscore = true;
            continue;
        }
        let Some(digit) = ascii_digit_value(byte) else {
            return Err(NumericParseError::InvalidLiteral);
        };
        if digit >= base {
            return Err(NumericParseError::InvalidLiteral);
        }
        value = value
            .checked_mul(base as u128)
            .and_then(|acc| acc.checked_add(digit as u128))
            .ok_or(NumericParseError::Overflow)?;
        if value > limit {
            return Err(NumericParseError::Overflow);
        }
        saw_digit = true;
        previous_digit = true;
        previous_underscore = false;
    }
    if !saw_digit || previous_underscore {
        return Err(NumericParseError::InvalidLiteral);
    }
    if negative {
        if value == (i64::MAX as u128) + 1 {
            Ok(i64::MIN as i128)
        } else {
            Ok(-((value as i64) as i128))
        }
    } else {
        Ok(value as i128)
    }
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
        NumericParseError::InvalidBase | NumericParseError::InvalidLiteral => {
            &raw mut crate::abi_types::PyExc_ValueError
        }
        NumericParseError::Overflow => &raw mut crate::abi_types::PyExc_OverflowError,
        NumericParseError::Type => &raw mut crate::abi_types::PyExc_TypeError,
    };
    unsafe { crate::api::errors::PyErr_SetString(exc, message.as_ptr()) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromLong(v: c_long) -> *mut PyObject {
    #[allow(clippy::unnecessary_cast)]
    py_long_from_i64(v as i64)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromSsize_t(v: isize) -> *mut PyObject {
    unsafe { PyLong_FromLong(v as c_long) }
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
                &raw mut crate::abi_types::PyExc_ValueError,
                c"cannot convert float NaN to integer".as_ptr(),
            );
        }
        return ptr::null_mut();
    }
    if !v.is_finite() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_OverflowError,
                c"cannot convert float infinity to integer".as_ptr(),
            );
        }
        return ptr::null_mut();
    }

    let truncated = v.trunc();
    if !(-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&truncated) {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_OverflowError,
                c"float too large to convert to Molt verified integer".as_ptr(),
            );
        }
        return ptr::null_mut();
    }
    py_long_from_i64(truncated as i64)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromUnicodeObject(u: *mut PyObject, base: c_int) -> *mut PyObject {
    let bytes = match unsafe { py_textlike_bytes(u) } {
        Ok(bytes) => bytes,
        Err(kind) => {
            unsafe {
                set_numeric_parse_error(kind, c"int() argument must be a string-like object")
            };
            return ptr::null_mut();
        }
    };
    match parse_python_int_literal(&bytes, base) {
        Ok(value) if value < 0 => py_long_from_i64(value as i64),
        Ok(value) => py_long_from_u64(value as u64),
        Err(kind) => {
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
    if bytes.is_null() {
        return ptr::null_mut();
    }
    let data = unsafe { std::slice::from_raw_parts(bytes, n.min(8)) };
    let mut value = 0u64;
    if little_endian != 0 {
        for (shift, byte) in data.iter().enumerate() {
            value |= (*byte as u64) << (shift * 8);
        }
    } else {
        for byte in data {
            value = (value << 8) | (*byte as u64);
        }
    }
    if is_signed != 0 && !data.is_empty() {
        let sign_bit = if little_endian != 0 {
            data[data.len() - 1] & 0x80
        } else {
            data[0] & 0x80
        };
        if sign_bit != 0 {
            let bits = (data.len() * 8) as u32;
            let signed = if bits >= 64 {
                value as i64
            } else {
                let mask = 1u64 << (bits - 1);
                ((value ^ mask).wrapping_sub(mask)) as i64
            };
            return py_long_from_i64(signed);
        }
    }
    py_long_from_u64(value)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsLong(op: *mut PyObject) -> c_long {
    py_long_as_i64(op) as c_long
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsLongAndOverflow(
    op: *mut PyObject,
    overflow: *mut c_int,
) -> c_long {
    let value = py_long_as_i64(op);
    if !overflow.is_null() {
        unsafe {
            *overflow = if value > c_long::MAX as i64 {
                1
            } else if value < c_long::MIN as i64 {
                -1
            } else {
                0
            };
        }
    }
    if value > c_long::MAX as i64 {
        c_long::MAX
    } else if value < c_long::MIN as i64 {
        c_long::MIN
    } else {
        value as c_long
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsSsize_t(op: *mut PyObject) -> isize {
    py_long_as_i64(op) as isize
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsLongLong(op: *mut PyObject) -> c_longlong {
    #[allow(clippy::unnecessary_cast)]
    {
        py_long_as_i64(op) as c_longlong
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsLongLongAndOverflow(
    op: *mut PyObject,
    overflow: *mut c_int,
) -> c_longlong {
    if let Some(value) = py_long_as_i64_checked(op) {
        if !overflow.is_null() {
            unsafe { *overflow = 0 };
        }
        #[allow(clippy::unnecessary_cast)]
        {
            return value as c_longlong;
        }
    }
    if let Some(value) = py_long_as_u64_checked(op) {
        if !overflow.is_null() {
            unsafe {
                *overflow = if value > i64::MAX as u64 { 1 } else { 0 };
            }
        }
        if value > i64::MAX as u64 {
            return c_longlong::MAX;
        }
        #[allow(clippy::unnecessary_cast)]
        {
            return value as c_longlong;
        }
    }
    if !overflow.is_null() {
        unsafe { *overflow = 0 };
    }
    unsafe {
        crate::api::errors::PyErr_SetString(
            &raw mut crate::abi_types::PyExc_TypeError,
            c"object cannot be interpreted as an integer".as_ptr(),
        );
    }
    -1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsUnsignedLong(op: *mut PyObject) -> c_ulong {
    py_long_as_i64(op) as c_ulong
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsUnsignedLongLong(op: *mut PyObject) -> c_ulonglong {
    py_long_as_i64(op) as c_ulonglong
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsVoidPtr(op: *mut PyObject) -> *mut c_void {
    if let Some(value) = py_long_as_u64_checked(op) {
        return value as usize as *mut c_void;
    }
    if let Some(value) = py_long_as_i64_checked(op)
        && value < 0
    {
        return (value as isize) as *mut c_void;
    }
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsNativeBytes(
    op: *mut PyObject,
    buffer: *mut c_void,
    n_bytes: Py_ssize_t,
    _flags: c_int,
) -> Py_ssize_t {
    let Some(value) = py_long_as_i64_checked(op) else {
        return -1;
    };
    let bytes = value.to_le_bytes();
    let required = std::mem::size_of::<i64>() as Py_ssize_t;
    if !buffer.is_null() && n_bytes > 0 {
        let count = (n_bytes as usize).min(bytes.len());
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer.cast::<u8>(), count);
        }
    }
    required
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromNativeBytes(
    buffer: *const c_void,
    n_bytes: usize,
    _flags: c_int,
) -> *mut PyObject {
    if buffer.is_null() {
        return ptr::null_mut();
    }
    let bytes = unsafe { std::slice::from_raw_parts(buffer.cast::<u8>(), n_bytes.min(8)) };
    let sign_extend = bytes.last().is_some_and(|byte| (byte & 0x80) != 0);
    let fill = if sign_extend { 0xff } else { 0x00 };
    let mut raw = [fill; 8];
    raw[..bytes.len()].copy_from_slice(bytes);
    py_long_from_i64(i64::from_le_bytes(raw))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromUnsignedNativeBytes(
    buffer: *const c_void,
    n_bytes: usize,
    _flags: c_int,
) -> *mut PyObject {
    if buffer.is_null() {
        return ptr::null_mut();
    }
    let bytes = unsafe { std::slice::from_raw_parts(buffer.cast::<u8>(), n_bytes.min(8)) };
    let mut raw = [0u8; 8];
    raw[..bytes.len()].copy_from_slice(bytes);
    py_long_from_u64(u64::from_le_bytes(raw))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyLong_AsInt(op: *mut PyObject) -> c_int {
    let value = py_long_as_i64(op);
    if value > c_int::MAX as i64 || value < c_int::MIN as i64 {
        set_long_overflow();
        -1
    } else {
        value as c_int
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsInt(op: *mut PyObject) -> c_int {
    unsafe { _PyLong_AsInt(op) }
}

fn set_long_overflow() {
    unsafe {
        crate::api::errors::PyErr_SetString(
            &raw mut crate::abi_types::PyExc_OverflowError,
            c"int too big to convert".as_ptr(),
        );
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyLong_AsByteArray(
    v: *mut crate::abi_types::PyLongObject,
    bytes: *mut u8,
    n: usize,
    little_endian: c_int,
    is_signed: c_int,
) -> c_int {
    if v.is_null() || bytes.is_null() {
        return -1;
    }
    let value = py_long_as_i64(v.cast::<PyObject>());
    if n == 0 {
        if value == 0 {
            return 0;
        }
        set_long_overflow();
        return -1;
    }
    if value < 0 && is_signed == 0 {
        set_long_overflow();
        return -1;
    }
    if n < 8 {
        let bits = (n * 8) as u32;
        if is_signed != 0 {
            let min = -(1i128 << (bits - 1));
            let max = (1i128 << (bits - 1)) - 1;
            let wide = value as i128;
            if wide < min || wide > max {
                set_long_overflow();
                return -1;
            }
        } else {
            let max = (1u128 << bits) - 1;
            if (value as u64 as u128) > max {
                set_long_overflow();
                return -1;
            }
        }
    }

    let raw = value as u64;
    let fill = if value < 0 { 0xff } else { 0x00 };
    for index in 0..n {
        let source_index = if little_endian != 0 {
            index
        } else {
            n - 1 - index
        };
        let byte = if source_index < 8 {
            ((raw >> (source_index * 8)) & 0xff) as u8
        } else {
            fill
        };
        unsafe {
            *bytes.add(index) = byte;
        }
    }
    0
}

// ─── PyFloat ─────────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyFloat_FromDouble(v: c_double) -> *mut PyObject {
    let bits = MoltObject::from_float(v).bits();
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
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

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyFloat_AsDouble(op: *mut PyObject) -> c_double {
    if op.is_null() {
        return -1.0;
    }
    let bridge = GLOBAL_BRIDGE.lock();
    match bridge.pyobj_to_handle(op) {
        Some(bits) => {
            let obj = MoltObject::from_bits(bits);
            if obj.is_float() {
                obj.as_float().unwrap_or(f64::NAN)
            } else if obj.is_int() {
                obj.as_int().map(|i| i as f64).unwrap_or(f64::NAN)
            } else {
                f64::NAN
            }
        }
        None => f64::NAN,
    }
}

const PY_HASH_INF: isize = 314159;

fn pointer_hash(ptr: *mut PyObject) -> isize {
    let width = usize::BITS;
    let raw = ptr as usize;
    let rotated = (raw >> 4) | (raw << (width - 4));
    let hash = rotated as isize;
    if hash == -1 { -2 } else { hash }
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

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyComplex_FromDoubles(real: c_double, imag: c_double) -> *mut PyObject {
    let obj = Box::new(PyComplexObject {
        ob_base: PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut crate::abi_types::PyComplex_Type,
        },
        cval: Py_complex { real, imag },
    });
    Box::into_raw(obj).cast::<PyObject>()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyComplex_FromCComplex(value: Py_complex) -> *mut PyObject {
    unsafe { PyComplex_FromDoubles(value.real, value.imag) }
}

pub unsafe extern "C" fn molt_complex_dealloc(op: *mut PyObject) {
    if op.is_null() {
        return;
    }
    unsafe { drop(Box::from_raw(op.cast::<PyComplexObject>())) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyComplex_AsCComplex(op: *mut PyObject) -> Py_complex {
    if op.is_null() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_TypeError,
                c"complex argument is NULL".as_ptr(),
            );
        }
        return Py_complex {
            real: -1.0,
            imag: 0.0,
        };
    }
    if unsafe { PyComplex_Check(op) } != 0 && GLOBAL_BRIDGE.lock().pyobj_to_handle(op).is_none() {
        return unsafe { (*op.cast::<PyComplexObject>()).cval };
    }
    if unsafe { PyFloat_Check(op) } != 0
        || unsafe { PyLong_Check(op) } != 0
        || unsafe { PyBool_Check(op) } != 0
    {
        return Py_complex {
            real: unsafe { PyFloat_AsDouble(op) },
            imag: 0.0,
        };
    }
    unsafe {
        crate::api::errors::PyErr_SetString(
            &raw mut crate::abi_types::PyExc_TypeError,
            c"cannot convert object to complex".as_ptr(),
        );
    }
    Py_complex {
        real: -1.0,
        imag: 0.0,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyComplex_RealAsDouble(op: *mut PyObject) -> c_double {
    unsafe { PyComplex_AsCComplex(op).real }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyComplex_ImagAsDouble(op: *mut PyObject) -> c_double {
    unsafe { PyComplex_AsCComplex(op).imag }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyComplex_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    let ob_type = unsafe { (*op).ob_type };
    std::ptr::eq(ob_type, &raw const crate::abi_types::PyComplex_Type) as c_int
}

// ─── PyBool ──────────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBool_FromLong(v: c_long) -> *mut PyObject {
    if v != 0 {
        &raw mut Py_True
    } else {
        &raw mut Py_False
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
            match GLOBAL_BRIDGE.lock().pyobj_to_handle(op) {
                Some(bits) => MoltObject::from_bits(bits).$pred() as c_int,
                None => 0,
            }
        }
    };
}

type_check!(PyLong_Check, is_int);
type_check!(PyFloat_Check, is_float);
type_check!(PyBool_Check, is_bool);

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    match GLOBAL_BRIDGE.lock().pyobj_to_handle(op) {
        Some(bits) => {
            let obj = MoltObject::from_bits(bits);
            (obj.is_int() || obj.is_float() || obj.is_bool()) as c_int
        }
        None => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::{NumericParseError, parse_python_float_literal, parse_python_int_literal};

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
}
