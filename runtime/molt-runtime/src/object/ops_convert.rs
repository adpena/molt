// Type conversion operations.
// Split from ops.rs for compilation-unit size reduction.

use crate::const_data_cache::{
    ConstDataLiteralKind, const_data_literal_insert, const_data_literal_lookup,
};
use crate::object::accessors::object_field_init_ptr_raw;
use crate::object::inc_ref_ptr;
use crate::object::ops::{as_float_extended, float_result_bits, is_float_extended};
use crate::object::ops_format::{format_bytes, format_string_repr_bytes};
use crate::*;
use molt_obj_model::MoltObject;
use num_bigint::BigInt;
use num_integer::Integer;
use num_traits::{Signed, ToPrimitive, Zero};

/// Format an i64 into a stack-allocated buffer, returning the UTF-8 slice.
/// This avoids the heap allocation of `i.to_string()` in the hot path.
#[inline(always)]
fn int_to_stack_str(val: i64, buf: &mut [u8; 24]) -> &[u8] {
    if val == 0 {
        buf[0] = b'0';
        return &buf[..1];
    }
    let negative = val < 0;
    // Work with absolute value using unsigned to avoid i64::MIN overflow.
    let mut uval: u64 = if negative {
        (val as u64).wrapping_neg()
    } else {
        val as u64
    };
    let mut pos = buf.len();
    while uval > 0 {
        pos -= 1;
        buf[pos] = b'0' + (uval % 10) as u8;
        uval /= 10;
    }
    if negative {
        pos -= 1;
        buf[pos] = b'-';
    }
    &buf[pos..]
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_str_from_obj(val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(val_bits);
        // Fast path: already a string -- inc_ref and return as-is.
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_STRING {
                    molt_inc_ref(ptr);
                    return val_bits;
                }
                if object_type_id(ptr) == TYPE_ID_EXCEPTION
                    && let Some(bits) =
                        crate::object::ops_format::exception_cached_message_str_bits(_py, ptr)
                {
                    return bits;
                }
            }
        }
        // Fast path: inline int -- stack-format to avoid String allocation.
        if let Some(i) = obj.as_int() {
            // Max i64 is 19 digits + sign = 20 bytes; 24 for safety.
            let mut buf = [0u8; 24];
            let s = int_to_stack_str(i, &mut buf);
            let ptr = alloc_string(_py, s);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        // Fast path: inline bool -- use pre-rendered strings.
        if let Some(b) = obj.as_bool() {
            let s = if b { b"True" as &[u8] } else { b"False" };
            let ptr = alloc_string(_py, s);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        let rendered = format_obj_str(_py, obj);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let ptr = alloc_string(_py, rendered.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_repr_from_obj(val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(val_bits);
        let rendered = format_obj(_py, obj);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let ptr = alloc_string(_py, rendered.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

fn ascii_escape(text: &str) -> String {
    let bytes = text.as_bytes();
    // SIMD fast path: if entire string is ASCII, return as-is (common case)
    if bytes.is_ascii() {
        return text.to_string();
    }
    // Find the first non-ASCII byte using SIMD scan, copy the safe prefix in bulk
    let mut first_non_ascii = 0usize;

    #[cfg(target_arch = "aarch64")]
    {
        unsafe {
            use std::arch::aarch64::*;
            let high_bit = vdupq_n_u8(0x80);
            while first_non_ascii + 16 <= bytes.len() {
                let chunk = vld1q_u8(bytes.as_ptr().add(first_non_ascii));
                let is_non_ascii = vandq_u8(chunk, high_bit);
                if vmaxvq_u8(is_non_ascii) != 0 {
                    break;
                }
                first_non_ascii += 16;
            }
        }
    }

    #[cfg(target_arch = "x86_64")]
    {
        unsafe {
            use std::arch::x86_64::*;
            while first_non_ascii + 16 <= bytes.len() {
                let chunk = _mm_loadu_si128(bytes.as_ptr().add(first_non_ascii) as *const __m128i);
                let mask = _mm_movemask_epi8(chunk) as u32; // high bit of each byte
                if mask != 0 {
                    break;
                }
                first_non_ascii += 16;
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    {
        unsafe {
            use std::arch::wasm32::*;
            let high_bit = u8x16_splat(0x80);
            while first_non_ascii + 16 <= bytes.len() {
                let chunk = v128_load(bytes.as_ptr().add(first_non_ascii) as *const v128);
                let has_high = v128_and(chunk, high_bit);
                if u8x16_bitmask(has_high) != 0 {
                    break;
                }
                first_non_ascii += 16;
            }
        }
    }

    while first_non_ascii < bytes.len() && bytes[first_non_ascii].is_ascii() {
        first_non_ascii += 1;
    }

    let mut out = String::with_capacity(text.len());
    // Copy the all-ASCII prefix in bulk
    out.push_str(&text[..first_non_ascii]);
    // Process remaining characters
    for ch in text[first_non_ascii..].chars() {
        if ch.is_ascii() {
            out.push(ch);
        } else {
            let code = ch as u32;
            if code <= 0xff {
                out.push_str(&format!("\\x{:02x}", code));
            } else if code <= 0xffff {
                out.push_str(&format!("\\u{:04x}", code));
            } else {
                out.push_str(&format!("\\U{:08x}", code));
            }
        }
    }
    out
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ascii_from_obj(val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(val_bits);
        let rendered = format_obj(_py, obj);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let escaped = ascii_escape(&rendered);
        let ptr = alloc_string(_py, escaped.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

fn format_int_base(value: &BigInt, base: u32, prefix: &str, upper: bool) -> String {
    let negative = value.is_negative();
    let mut abs_val = if negative { -value } else { value.clone() };
    if abs_val.is_zero() {
        abs_val = BigInt::from(0);
    }
    let mut digits = abs_val.to_str_radix(base);
    if upper {
        digits = digits.to_uppercase();
    }
    if negative {
        format!("-{prefix}{digits}")
    } else {
        format!("{prefix}{digits}")
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bin_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let type_name = class_name_for_error(type_of_bits(_py, val_bits));
        let msg = format!("'{type_name}' object cannot be interpreted as an integer");
        let Some(value) = index_bigint_from_obj(_py, val_bits, &msg) else {
            return MoltObject::none().bits();
        };
        let text = format_int_base(&value, 2, "0b", false);
        let ptr = alloc_string(_py, text.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_oct_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let type_name = class_name_for_error(type_of_bits(_py, val_bits));
        let msg = format!("'{type_name}' object cannot be interpreted as an integer");
        let Some(value) = index_bigint_from_obj(_py, val_bits, &msg) else {
            return MoltObject::none().bits();
        };
        let text = format_int_base(&value, 8, "0o", false);
        let ptr = alloc_string(_py, text.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_hex_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let type_name = class_name_for_error(type_of_bits(_py, val_bits));
        let msg = format!("'{type_name}' object cannot be interpreted as an integer");
        let Some(value) = index_bigint_from_obj(_py, val_bits, &msg) else {
            return MoltObject::none().bits();
        };
        let text = format_int_base(&value, 16, "0x", false);
        let ptr = alloc_string(_py, text.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

fn parse_float_from_bytes(bytes: &[u8]) -> Result<f64, ()> {
    let text = std::str::from_utf8(bytes).map_err(|_| ())?;
    let trimmed = text.trim();
    trimmed.parse::<f64>().map_err(|_| ())
}

fn parse_complex_from_str(text: &str) -> Result<ComplexParts, ()> {
    let mut trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(());
    }
    if trimmed.starts_with('(') && trimmed.ends_with(')') && trimmed.len() >= 2 {
        trimmed = trimmed[1..trimmed.len() - 1].trim();
        if trimmed.is_empty() {
            return Err(());
        }
    }
    if trimmed.chars().any(|ch| ch.is_whitespace()) {
        return Err(());
    }
    let bytes = trimmed.as_bytes();
    let ends_with_j = matches!(bytes.last(), Some(b'j') | Some(b'J'));
    if ends_with_j {
        let core = &trimmed[..trimmed.len() - 1];
        if core.is_empty() {
            return Ok(ComplexParts { re: 0.0, im: 1.0 });
        }
        if core == "+" {
            return Ok(ComplexParts { re: 0.0, im: 1.0 });
        }
        if core == "-" {
            return Ok(ComplexParts { re: 0.0, im: -1.0 });
        }
        let mut sep_idx = None;
        let core_bytes = core.as_bytes();
        for idx in 1..core_bytes.len() {
            let ch = core_bytes[idx] as char;
            if ch == '+' || ch == '-' {
                let prev = core_bytes[idx - 1] as char;
                if prev == 'e' || prev == 'E' {
                    continue;
                }
                sep_idx = Some(idx);
            }
        }
        if let Some(idx) = sep_idx {
            let real_part = &core[..idx];
            let imag_part = &core[idx..];
            let real = parse_float_from_bytes(real_part.as_bytes())?;
            let imag = if imag_part == "+" {
                1.0
            } else if imag_part == "-" {
                -1.0
            } else {
                parse_float_from_bytes(imag_part.as_bytes())?
            };
            return Ok(ComplexParts { re: real, im: imag });
        }
        let imag = parse_float_from_bytes(core.as_bytes())?;
        return Ok(ComplexParts { re: 0.0, im: imag });
    }
    let real = parse_float_from_bytes(trimmed.as_bytes())?;
    Ok(ComplexParts { re: real, im: 0.0 })
}

fn parse_int_from_str(text: &str, base: i64) -> Result<(BigInt, i64), ()> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(());
    }
    let mut sign = 1i32;
    let mut digits = trimmed;
    if let Some(rest) = digits.strip_prefix('+') {
        digits = rest;
    } else if let Some(rest) = digits.strip_prefix('-') {
        digits = rest;
        sign = -1;
    }
    let mut base_val = base;
    if base_val == 0 {
        if let Some(rest) = digits
            .strip_prefix("0x")
            .or_else(|| digits.strip_prefix("0X"))
        {
            base_val = 16;
            digits = rest;
        } else if let Some(rest) = digits
            .strip_prefix("0o")
            .or_else(|| digits.strip_prefix("0O"))
        {
            base_val = 8;
            digits = rest;
        } else if let Some(rest) = digits
            .strip_prefix("0b")
            .or_else(|| digits.strip_prefix("0B"))
        {
            base_val = 2;
            digits = rest;
        } else {
            base_val = 10;
        }
    } else if base_val == 16 {
        if let Some(rest) = digits
            .strip_prefix("0x")
            .or_else(|| digits.strip_prefix("0X"))
        {
            digits = rest;
        }
    } else if base_val == 8 {
        if let Some(rest) = digits
            .strip_prefix("0o")
            .or_else(|| digits.strip_prefix("0O"))
        {
            digits = rest;
        }
    } else if base_val == 2
        && let Some(rest) = digits
            .strip_prefix("0b")
            .or_else(|| digits.strip_prefix("0B"))
    {
        digits = rest;
    }
    let digits = digits.replace('_', "");
    if digits.is_empty() {
        return Err(());
    }
    let parsed = BigInt::parse_bytes(digits.as_bytes(), base_val as u32).ok_or(())?;
    let parsed = if sign < 0 { -parsed } else { parsed };
    Ok((parsed, base_val))
}

#[inline(always)]
fn parse_simple_ascii_decimal_i64(text: &str) -> Option<i64> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let bytes = trimmed.as_bytes();
    let mut idx = 0usize;
    let mut negative = false;
    match bytes[0] {
        b'+' => idx = 1,
        b'-' => {
            negative = true;
            idx = 1;
        }
        _ => {}
    }
    if idx == bytes.len() {
        return None;
    }
    let limit: u64 = if negative {
        (i64::MAX as u64) + 1
    } else {
        i64::MAX as u64
    };
    let mut value = 0u64;
    while idx < bytes.len() {
        let byte = bytes[idx];
        if !byte.is_ascii_digit() {
            return None;
        }
        let digit = (byte - b'0') as u64;
        if value > (limit - digit) / 10 {
            return None;
        }
        value = value * 10 + digit;
        idx += 1;
    }
    if negative {
        if value == (i64::MAX as u64) + 1 {
            Some(i64::MIN)
        } else {
            Some(-(value as i64))
        }
    } else {
        Some(value as i64)
    }
}

/// # Safety
/// - `ptr` must be null or valid for `len_bits` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_bigint_from_str(ptr: *const u8, len_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let len = usize_from_bits(len_bits);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            let data_key = ptr as usize;
            if let Some(bits) =
                const_data_literal_lookup(ConstDataLiteralKind::BigInt, data_key, len)
            {
                inc_ref_bits(_py, bits);
                return bits;
            }
            let bytes = std::slice::from_raw_parts(ptr, len);
            let text = match std::str::from_utf8(bytes) {
                Ok(val) => val,
                Err(_) => {
                    return raise_exception::<_>(_py, "ValueError", "invalid literal for int()");
                }
            };
            let bits = if let Some(i) = parse_simple_ascii_decimal_i64(text) {
                int_bits_from_i64(_py, i)
            } else {
                let (parsed, _base_used) = match parse_int_from_str(text, 10) {
                    Ok(val) => val,
                    Err(_) => {
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "invalid literal for int()",
                        );
                    }
                };
                if let Some(i) = bigint_to_inline(&parsed) {
                    MoltObject::from_int(i).bits()
                } else {
                    bigint_bits(_py, parsed)
                }
            };
            if !obj_from_bits(bits).is_none() {
                const_data_literal_insert(_py, ConstDataLiteralKind::BigInt, data_key, len, bits);
            }
            bits
        })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_float_from_obj(val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(val_bits);
        // Inline non-NaN float: return as-is.
        if obj.is_float() {
            return val_bits;
        }
        // Heap-allocated NaN float (TYPE_ID_FLOAT): `float(x)` returns the
        // same object when x is already a float, matching CPython semantics.
        if let Some(ptr) = obj.as_ptr()
            && unsafe { object_type_id(ptr) } == TYPE_ID_FLOAT
        {
            unsafe { inc_ref_ptr(_py, ptr) };
            return val_bits;
        }
        if complex_ptr_from_bits(val_bits).is_some() {
            let type_label = type_name(_py, obj);
            let msg =
                format!("float() argument must be a string or a real number, not '{type_label}'");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        if let Some(i) = to_i64(obj) {
            return MoltObject::from_float(i as f64).bits();
        }
        if let Some(ptr) = bigint_ptr_from_bits(val_bits) {
            let big = unsafe { bigint_ref(ptr) };
            if let Some(val) = big.to_f64() {
                return MoltObject::from_float(val).bits();
            }
            return raise_exception::<_>(_py, "OverflowError", "int too large to convert to float");
        }
        if let Some(ptr) = maybe_ptr_from_bits(val_bits) {
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_STRING {
                    let len = string_len(ptr);
                    let bytes = std::slice::from_raw_parts(string_bytes(ptr), len);
                    if let Ok(parsed) = parse_float_from_bytes(bytes) {
                        return float_result_bits(_py, parsed);
                    }
                    // CPython embeds `repr(arg)`: a str renders with quote
                    // selection and escapes (`"it's"`, `'a\nb'`), not raw bytes.
                    let msg = format!(
                        "could not convert string to float: {}",
                        format_string_repr_bytes(bytes)
                    );
                    return raise_exception::<_>(_py, "ValueError", &msg);
                }
                if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
                    let len = bytes_len(ptr);
                    let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                    if let Ok(parsed) = parse_float_from_bytes(bytes) {
                        return float_result_bits(_py, parsed);
                    }
                    // CPython embeds `repr(arg)`: `b'...'` for bytes,
                    // `bytearray(b'...')` for bytearray, with byte-wise `\xNN`.
                    let rendered = if type_id == TYPE_ID_BYTEARRAY {
                        format!("bytearray({})", format_bytes(bytes))
                    } else {
                        format_bytes(bytes)
                    };
                    let msg = format!("could not convert string to float: {rendered}");
                    return raise_exception::<_>(_py, "ValueError", &msg);
                }
                let float_name_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.float_name, b"__float__");
                if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, float_name_bits) {
                    let res_bits = call_callable0(_py, call_bits);
                    dec_ref_bits(_py, call_bits);
                    let res_obj = obj_from_bits(res_bits);
                    if is_float_extended(res_obj) {
                        return res_bits;
                    }
                    let owner = class_name_for_error(type_of_bits(_py, val_bits));
                    let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                    if res_obj.as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    let msg = format!("{owner}.__float__ returned non-float (type {res_type})");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                let index_name_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.index_name, b"__index__");
                if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, index_name_bits) {
                    let res_bits = call_callable0(_py, call_bits);
                    dec_ref_bits(_py, call_bits);
                    let res_obj = obj_from_bits(res_bits);
                    if let Some(i) = to_i64(res_obj) {
                        return MoltObject::from_float(i as f64).bits();
                    }
                    let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                    if res_obj.as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    let msg = format!("__index__ returned non-int (type {res_type})");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
            }
        }
        let msg = format!(
            "float() argument must be a string or a real number, not '{}'",
            type_name(_py, obj)
        );
        raise_exception::<_>(_py, "TypeError", &msg)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_float_new(cls_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let cls_obj = obj_from_bits(cls_bits);
        let Some(cls_ptr) = cls_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "float.__new__ expects type");
        };
        unsafe {
            if object_type_id(cls_ptr) != TYPE_ID_TYPE {
                return raise_exception::<_>(_py, "TypeError", "float.__new__ expects type");
            }
        }
        let builtins = builtin_classes(_py);
        if cls_bits != builtins.float && !issubclass_bits(cls_bits, builtins.float) {
            let type_label = class_name_for_error(cls_bits);
            let msg = format!("float.__new__ expects type, got {}", type_label);
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let float_bits = molt_float_from_obj(val_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if cls_bits == builtins.float {
            return float_bits;
        }
        let inst_bits = unsafe { alloc_instance_for_class(_py, cls_ptr) };
        let Some(inst_ptr) = obj_from_bits(inst_bits).as_ptr() else {
            if obj_from_bits(float_bits).as_ptr().is_some() {
                dec_ref_bits(_py, float_bits);
            }
            return MoltObject::none().bits();
        };
        let Some(slot_name_bits) = attr_name_bits_from_bytes(_py, b"__molt_float_value__") else {
            if obj_from_bits(float_bits).as_ptr().is_some() {
                dec_ref_bits(_py, float_bits);
            }
            return raise_exception::<_>(
                _py,
                "TypeError",
                "float subclass layout missing value slot",
            );
        };
        let Some(offset) = (unsafe { class_field_offset(_py, cls_ptr, slot_name_bits) }) else {
            dec_ref_bits(_py, slot_name_bits);
            if obj_from_bits(float_bits).as_ptr().is_some() {
                dec_ref_bits(_py, float_bits);
            }
            return raise_exception::<_>(
                _py,
                "TypeError",
                "float subclass layout missing value slot",
            );
        };
        dec_ref_bits(_py, slot_name_bits);
        unsafe {
            let _ = object_field_init_ptr_raw(_py, inst_ptr, offset, float_bits);
        }
        if obj_from_bits(float_bits).as_ptr().is_some() {
            dec_ref_bits(_py, float_bits);
        }
        inst_bits
    })
}

fn parse_float_fromhex_text(text: &str) -> Result<f64, ()> {
    let mut src = text.trim();
    if src.is_empty() {
        return Err(());
    }
    let mut sign = 1.0f64;
    if let Some(rest) = src.strip_prefix('+') {
        src = rest;
    } else if let Some(rest) = src.strip_prefix('-') {
        src = rest;
        sign = -1.0;
    }
    if src.eq_ignore_ascii_case("inf") || src.eq_ignore_ascii_case("infinity") {
        return Ok(sign * f64::INFINITY);
    }
    if src.eq_ignore_ascii_case("nan") {
        return Ok(f64::NAN);
    }
    let Some(hex_src) = src.strip_prefix("0x").or_else(|| src.strip_prefix("0X")) else {
        return Err(());
    };
    let mut split = hex_src.split(['p', 'P']);
    let significand = split.next().ok_or(())?;
    let exponent_text = split.next().ok_or(())?;
    if split.next().is_some() {
        return Err(());
    }
    let exponent = exponent_text.parse::<i32>().map_err(|_| ())?;
    let (int_part, frac_part) = if let Some((left, right)) = significand.split_once('.') {
        (left, right)
    } else {
        (significand, "")
    };
    if int_part.is_empty() && frac_part.is_empty() {
        return Err(());
    }
    let mut mantissa = 0.0f64;
    let mut digits = 0usize;
    for ch in int_part.bytes() {
        let Some(d) = (ch as char).to_digit(16) else {
            return Err(());
        };
        mantissa = mantissa * 16.0 + d as f64;
        digits += 1;
    }
    let mut frac_digits = 0usize;
    for ch in frac_part.bytes() {
        let Some(d) = (ch as char).to_digit(16) else {
            return Err(());
        };
        mantissa = mantissa * 16.0 + d as f64;
        digits += 1;
        frac_digits += 1;
    }
    if digits == 0 {
        return Err(());
    }
    let exp2 = exponent
        .checked_sub((frac_digits.saturating_mul(4)) as i32)
        .ok_or(())?;
    let mut out = mantissa * 2f64.powi(exp2);
    if sign.is_sign_negative() {
        out = -out;
    }
    Ok(out)
}

fn float_hex_string(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_string();
    }
    if value.is_infinite() {
        if value.is_sign_negative() {
            return "-inf".to_string();
        }
        return "inf".to_string();
    }
    if value == 0.0 {
        if value.is_sign_negative() {
            return "-0x0.0p+0".to_string();
        }
        return "0x0.0p+0".to_string();
    }
    let bits = value.to_bits();
    let sign = if (bits >> 63) != 0 { "-" } else { "" };
    let exp_bits = ((bits >> 52) & 0x7ff) as i32;
    let frac_bits = bits & ((1u64 << 52) - 1);
    let (lead, exponent) = if exp_bits == 0 {
        (0u8, -1022)
    } else {
        (1u8, exp_bits - 1023)
    };
    format!("{sign}0x{lead:x}.{frac_bits:013x}p{exponent:+}")
}

fn float_value_bits_or_descriptor_error(
    _py: &PyToken<'_>,
    self_bits: u64,
    method: &str,
) -> Option<u64> {
    let obj = obj_from_bits(self_bits);
    if as_float_extended(obj).is_some() {
        return Some(self_bits);
    }
    if let Some(bits) = float_subclass_value_bits_raw(self_bits)
        && as_float_extended(obj_from_bits(bits)).is_some()
    {
        return Some(bits);
    }
    let type_label = class_name_for_error(type_of_bits(_py, self_bits));
    let msg = format!(
        "descriptor '{method}' for 'float' objects doesn't apply to a '{type_label}' object"
    );
    let _ = raise_exception::<u64>(_py, "TypeError", &msg);
    None
}

fn float_value_or_descriptor_error(_py: &PyToken<'_>, self_bits: u64, method: &str) -> Option<f64> {
    let bits = float_value_bits_or_descriptor_error(_py, self_bits, method)?;
    as_float_extended(obj_from_bits(bits))
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_float_float(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(bits) = float_value_bits_or_descriptor_error(_py, self_bits, "__float__") else {
            return MoltObject::none().bits();
        };
        if obj_from_bits(bits).as_ptr().is_some() {
            inc_ref_bits(_py, bits);
        }
        bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_float_conjugate(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(value) = float_value_or_descriptor_error(_py, self_bits, "conjugate") else {
            return MoltObject::none().bits();
        };
        float_result_bits(_py, value)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_float_is_integer(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(value) = float_value_or_descriptor_error(_py, self_bits, "is_integer") else {
            return MoltObject::none().bits();
        };
        let out = value.is_finite() && value.fract() == 0.0;
        MoltObject::from_bool(out).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_float_as_integer_ratio(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(value) = float_value_or_descriptor_error(_py, self_bits, "as_integer_ratio")
        else {
            return MoltObject::none().bits();
        };
        if value.is_nan() {
            return raise_exception::<_>(_py, "ValueError", "cannot convert NaN to integer ratio");
        }
        if value.is_infinite() {
            return raise_exception::<_>(
                _py,
                "OverflowError",
                "cannot convert Infinity to integer ratio",
            );
        }
        if value == 0.0 {
            let zero = MoltObject::from_int(0).bits();
            let one = MoltObject::from_int(1).bits();
            let tuple_ptr = alloc_tuple(_py, &[zero, one]);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(tuple_ptr).bits();
        }
        let bits = value.to_bits();
        let negative = (bits >> 63) != 0;
        let exp_bits = ((bits >> 52) & 0x7ff) as i32;
        let mut mantissa = bits & ((1u64 << 52) - 1);
        let exponent = if exp_bits == 0 {
            -1022 - 52
        } else {
            mantissa |= 1u64 << 52;
            exp_bits - 1023 - 52
        };
        let mut numerator = BigInt::from(mantissa);
        if negative {
            numerator = -numerator;
        }
        let mut denominator = BigInt::from(1u8);
        if exponent >= 0 {
            numerator <<= exponent as usize;
        } else {
            denominator <<= (-exponent) as usize;
        }
        let gcd = numerator.abs().gcd(&denominator);
        if !gcd.is_zero() {
            numerator /= &gcd;
            denominator /= &gcd;
        }
        let num_bits = int_bits_from_bigint(_py, numerator);
        let den_bits = int_bits_from_bigint(_py, denominator);
        let tuple_ptr = alloc_tuple(_py, &[num_bits, den_bits]);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_float_hex(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(value) = float_value_or_descriptor_error(_py, self_bits, "hex") else {
            return MoltObject::none().bits();
        };
        let text = float_hex_string(value);
        let ptr = alloc_string(_py, text.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_float_fromhex(cls_bits: u64, text_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let text_obj = obj_from_bits(text_bits);
        // CPython's float.fromhex emits a generic, type-name-free message on all
        // supported versions (3.12/3.13/3.14) for a non-str argument.
        let Some(text_ptr) = text_obj.as_ptr() else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "bad argument type for built-in operation",
            );
        };
        unsafe {
            if object_type_id(text_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "bad argument type for built-in operation",
                );
            }
            let bytes = std::slice::from_raw_parts(string_bytes(text_ptr), string_len(text_ptr));
            let text = match std::str::from_utf8(bytes) {
                Ok(val) => val,
                Err(_) => {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "invalid hexadecimal floating-point string",
                    );
                }
            };
            let value = match parse_float_fromhex_text(text) {
                Ok(val) => val,
                Err(()) => {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "invalid hexadecimal floating-point string",
                    );
                }
            };
            let out_bits = float_result_bits(_py, value);
            let builtins = builtin_classes(_py);
            if cls_bits == builtins.float {
                return out_bits;
            }
            if !issubclass_bits(cls_bits, builtins.float) {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "fromhex() requires a float subclass",
                );
            }
            let res_bits = call_callable1(_py, cls_bits, out_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            res_bits
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_float_from_number(cls_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Some(ptr) = maybe_ptr_from_bits(val_bits) {
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_STRING
                    || type_id == TYPE_ID_BYTES
                    || type_id == TYPE_ID_BYTEARRAY
                {
                    let msg = format!(
                        "must be real number, not {}",
                        type_name(_py, obj_from_bits(val_bits))
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            }
        }
        if complex_ptr_from_bits(val_bits).is_some() {
            let msg = format!(
                "must be real number, not {}",
                type_name(_py, obj_from_bits(val_bits))
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let out_bits = molt_float_from_obj(val_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let builtins = builtin_classes(_py);
        if cls_bits == builtins.float {
            return out_bits;
        }
        if !issubclass_bits(cls_bits, builtins.float) {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "from_number() requires a float subclass",
            );
        }
        let res_bits = unsafe { call_callable1(_py, cls_bits, out_bits) };
        dec_ref_bits(_py, out_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        res_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_complex_from_obj(val_bits: u64, imag_bits: u64, has_imag_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let has_imag = to_i64(obj_from_bits(has_imag_bits)).unwrap_or(0) != 0;
        let val_obj = obj_from_bits(val_bits);
        if !has_imag {
            if complex_ptr_from_bits(val_bits).is_some() {
                inc_ref_bits(_py, val_bits);
                return val_bits;
            }
            if let Some(f) = val_obj.as_float() {
                return complex_bits(_py, f, 0.0);
            }
            if let Some(i) = to_i64(val_obj) {
                return complex_bits(_py, i as f64, 0.0);
            }
            if let Some(ptr) = bigint_ptr_from_bits(val_bits) {
                if let Some(val) = unsafe { bigint_ref(ptr) }.to_f64() {
                    return complex_bits(_py, val, 0.0);
                }
                return raise_exception::<_>(
                    _py,
                    "OverflowError",
                    "int too large to convert to float",
                );
            }
            if let Some(ptr) = maybe_ptr_from_bits(val_bits) {
                unsafe {
                    let type_id = object_type_id(ptr);
                    if type_id == TYPE_ID_STRING {
                        let len = string_len(ptr);
                        let bytes = std::slice::from_raw_parts(string_bytes(ptr), len);
                        let text = match std::str::from_utf8(bytes) {
                            Ok(val) => val,
                            Err(_) => {
                                return raise_exception::<_>(
                                    _py,
                                    "ValueError",
                                    "complex() arg is a malformed string",
                                );
                            }
                        };
                        match parse_complex_from_str(text) {
                            Ok(parts) => {
                                return complex_bits(_py, parts.re, parts.im);
                            }
                            Err(()) => {
                                return raise_exception::<_>(
                                    _py,
                                    "ValueError",
                                    "complex() arg is a malformed string",
                                );
                            }
                        }
                    }
                    if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
                        let type_label = type_name(_py, val_obj);
                        let msg = format!(
                            "complex() argument must be a string or a number, not {type_label}"
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                    if let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__complex__") {
                        if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, name_bits)
                        {
                            let res_bits = call_callable0(_py, call_bits);
                            dec_ref_bits(_py, call_bits);
                            if exception_pending(_py) {
                                return MoltObject::none().bits();
                            }
                            if complex_ptr_from_bits(res_bits).is_some() {
                                return res_bits;
                            }
                            let owner = class_name_for_error(type_of_bits(_py, val_bits));
                            let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                            if obj_from_bits(res_bits).as_ptr().is_some() {
                                dec_ref_bits(_py, res_bits);
                            }
                            let msg = format!(
                                "{owner}.__complex__ returned non-complex (type {res_type})"
                            );
                            return raise_exception::<_>(_py, "TypeError", &msg);
                        }
                        dec_ref_bits(_py, name_bits);
                    }
                    let float_name_bits = intern_static_name(
                        _py,
                        &runtime_state(_py).interned.float_name,
                        b"__float__",
                    );
                    if let Some(call_bits) =
                        attr_lookup_ptr_allow_missing(_py, ptr, float_name_bits)
                    {
                        let res_bits = call_callable0(_py, call_bits);
                        dec_ref_bits(_py, call_bits);
                        let res_obj = obj_from_bits(res_bits);
                        if let Some(f) = res_obj.as_float() {
                            return complex_bits(_py, f, 0.0);
                        }
                        let owner = class_name_for_error(type_of_bits(_py, val_bits));
                        let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                        if res_obj.as_ptr().is_some() {
                            dec_ref_bits(_py, res_bits);
                        }
                        let msg = format!("{owner}.__float__ returned non-float (type {res_type})");
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    let index_name_bits = intern_static_name(
                        _py,
                        &runtime_state(_py).interned.index_name,
                        b"__index__",
                    );
                    if let Some(call_bits) =
                        attr_lookup_ptr_allow_missing(_py, ptr, index_name_bits)
                    {
                        let res_bits = call_callable0(_py, call_bits);
                        dec_ref_bits(_py, call_bits);
                        let res_obj = obj_from_bits(res_bits);
                        if let Some(i) = to_i64(res_obj) {
                            return complex_bits(_py, i as f64, 0.0);
                        }
                        let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                        if res_obj.as_ptr().is_some() {
                            dec_ref_bits(_py, res_bits);
                        }
                        let msg = format!("__index__ returned non-int (type {res_type})");
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                }
            }
            return raise_exception::<_>(
                _py,
                "TypeError",
                "complex() argument must be a string or a number",
            );
        }
        let imag_obj = obj_from_bits(imag_bits);
        if let Some(ptr) = maybe_ptr_from_bits(val_bits) {
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_STRING
                    || type_id == TYPE_ID_BYTES
                    || type_id == TYPE_ID_BYTEARRAY
                {
                    let type_label = type_name(_py, val_obj);
                    let msg = format!(
                        "complex() argument 'real' must be a real number, not {type_label}"
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            }
        }
        if let Some(ptr) = maybe_ptr_from_bits(imag_bits) {
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_STRING
                    || type_id == TYPE_ID_BYTES
                    || type_id == TYPE_ID_BYTEARRAY
                {
                    let type_label = type_name(_py, imag_obj);
                    let msg = format!(
                        "complex() argument 'imag' must be a real number, not {type_label}"
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            }
        }
        let real = match complex_from_obj_strict(_py, val_obj) {
            Ok(Some(val)) => val,
            Ok(None) => {
                let type_label = type_name(_py, val_obj);
                let msg =
                    format!("complex() argument 'real' must be a real number, not {type_label}");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            Err(()) => {
                return raise_exception::<_>(
                    _py,
                    "OverflowError",
                    "int too large to convert to float",
                );
            }
        };
        let imag = match complex_from_obj_strict(_py, imag_obj) {
            Ok(Some(val)) => val,
            Ok(None) => {
                let type_label = type_name(_py, imag_obj);
                let msg =
                    format!("complex() argument 'imag' must be a real number, not {type_label}");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            Err(()) => {
                return raise_exception::<_>(
                    _py,
                    "OverflowError",
                    "int too large to convert to float",
                );
            }
        };
        let re = real.re - imag.im;
        let im = real.im + imag.re;
        complex_bits(_py, re, im)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_complex_conjugate(val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(ptr) = complex_ptr_from_bits(val_bits) else {
            return raise_exception::<_>(_py, "TypeError", "complex.conjugate expects complex");
        };
        let value = unsafe { *complex_ref(ptr) };
        complex_bits(_py, value.re, -value.im)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_complex_from_number(cls_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Some(ptr) = maybe_ptr_from_bits(val_bits) {
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_STRING
                    || type_id == TYPE_ID_BYTES
                    || type_id == TYPE_ID_BYTEARRAY
                {
                    let msg = format!(
                        "must be real number, not {}",
                        type_name(_py, obj_from_bits(val_bits))
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            }
        }
        let none_bits = MoltObject::none().bits();
        let false_bits = MoltObject::from_bool(false).bits();
        let out_bits = molt_complex_from_obj(val_bits, none_bits, false_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let builtins = builtin_classes(_py);
        if cls_bits == builtins.complex {
            return out_bits;
        }
        if !issubclass_bits(cls_bits, builtins.complex) {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "from_number() requires a complex subclass",
            );
        }
        let res_bits = unsafe { call_callable1(_py, cls_bits, out_bits) };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        res_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_new(cls_bits: u64, val_bits: u64, base_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let cls_obj = obj_from_bits(cls_bits);
        let Some(cls_ptr) = cls_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "int.__new__ expects type");
        };
        unsafe {
            if object_type_id(cls_ptr) != TYPE_ID_TYPE {
                return raise_exception::<_>(_py, "TypeError", "int.__new__ expects type");
            }
        }
        let has_base = base_bits != missing_bits(_py);
        let has_base_bits = MoltObject::from_int(if has_base { 1 } else { 0 }).bits();
        let int_bits = molt_int_from_obj(val_bits, base_bits, has_base_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let builtins = builtin_classes(_py);
        if cls_bits == builtins.int {
            return int_bits;
        }
        if !issubclass_bits(cls_bits, builtins.int) {
            let type_label = class_name_for_error(cls_bits);
            let msg = format!("int.__new__ expects type, got {}", type_label);
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let inst_bits = unsafe { alloc_instance_for_class(_py, cls_ptr) };
        let Some(inst_ptr) = obj_from_bits(inst_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        let Some(slot_name_bits) = attr_name_bits_from_bytes(_py, b"__molt_int_value__") else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "int subclass layout missing value slot",
            );
        };
        let Some(offset) = (unsafe { class_field_offset(_py, cls_ptr, slot_name_bits) }) else {
            dec_ref_bits(_py, slot_name_bits);
            return raise_exception::<_>(
                _py,
                "TypeError",
                "int subclass layout missing value slot",
            );
        };
        dec_ref_bits(_py, slot_name_bits);
        unsafe {
            let _ = object_field_init_ptr_raw(_py, inst_ptr, offset, int_bits);
        }
        inst_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_int(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(self_bits);
        if obj.is_int() {
            return self_bits;
        }
        if obj.is_bool() {
            return MoltObject::from_int(if obj.as_bool().unwrap_or(false) { 1 } else { 0 }).bits();
        }
        if bigint_ptr_from_bits(self_bits).is_some() {
            inc_ref_bits(_py, self_bits);
            return self_bits;
        }
        if let Some(bits) = int_subclass_value_bits_raw(self_bits) {
            if obj_from_bits(bits).as_ptr().is_some() {
                inc_ref_bits(_py, bits);
            }
            return bits;
        }
        let type_label = class_name_for_error(type_of_bits(_py, self_bits));
        let msg = format!(
            "descriptor '__int__' requires a 'int' object but received '{}'",
            type_label
        );
        raise_exception::<_>(_py, "TypeError", &msg)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_index(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(self_bits);
        if obj.is_int() {
            return self_bits;
        }
        if obj.is_bool() {
            return MoltObject::from_int(if obj.as_bool().unwrap_or(false) { 1 } else { 0 }).bits();
        }
        if bigint_ptr_from_bits(self_bits).is_some() {
            inc_ref_bits(_py, self_bits);
            return self_bits;
        }
        if let Some(bits) = int_subclass_value_bits_raw(self_bits) {
            if obj_from_bits(bits).as_ptr().is_some() {
                inc_ref_bits(_py, bits);
            }
            return bits;
        }
        let type_label = class_name_for_error(type_of_bits(_py, self_bits));
        let msg = format!(
            "descriptor '__index__' requires a 'int' object but received '{}'",
            type_label
        );
        raise_exception::<_>(_py, "TypeError", &msg)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_bit_length(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(self_bits);
        let Some(value) = to_bigint(obj) else {
            let type_label = class_name_for_error(type_of_bits(_py, self_bits));
            let msg = format!(
                "descriptor 'bit_length' requires a 'int' object but received '{}'",
                type_label
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        let (_sign, bytes) = value.to_bytes_be();
        if bytes.is_empty() {
            return MoltObject::from_int(0).bits();
        }
        let lead = bytes[0];
        let lead_bits = 8usize.saturating_sub(lead.leading_zeros() as usize);
        let total_bits = (bytes.len().saturating_sub(1) * 8) + lead_bits;
        MoltObject::from_int(total_bits as i64).bits()
    })
}

fn int_method_value_bits_or_error(_py: &PyToken<'_>, self_bits: u64, method: &str) -> Option<u64> {
    let obj = obj_from_bits(self_bits);
    if obj.is_int() {
        return Some(self_bits);
    }
    if obj.is_bool() {
        return Some(
            MoltObject::from_int(if obj.as_bool().unwrap_or(false) { 1 } else { 0 }).bits(),
        );
    }
    if bigint_ptr_from_bits(self_bits).is_some() {
        return Some(self_bits);
    }
    if let Some(bits) = int_subclass_value_bits_raw(self_bits) {
        return Some(bits);
    }
    let type_label = class_name_for_error(type_of_bits(_py, self_bits));
    let msg = format!(
        "descriptor '{method}' requires a 'int' object but received '{}'",
        type_label
    );
    let _ = raise_exception::<u64>(_py, "TypeError", &msg);
    None
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_bit_count(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(value_bits) = int_method_value_bits_or_error(_py, self_bits, "bit_count") else {
            return MoltObject::none().bits();
        };
        let value_obj = obj_from_bits(value_bits);
        if let Some(i) = to_i64(value_obj) {
            let count = i.unsigned_abs().count_ones() as i64;
            return MoltObject::from_int(count).bits();
        }
        if let Some(ptr) = bigint_ptr_from_bits(value_bits) {
            let abs = unsafe { bigint_ref(ptr) }.abs();
            let (_sign, bytes) = abs.to_bytes_le();
            let mut count = 0i64;
            for byte in bytes {
                count += byte.count_ones() as i64;
            }
            return MoltObject::from_int(count).bits();
        }
        // int subclasses should always lower to int/bigint storage.
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_as_integer_ratio(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(num_bits) = int_method_value_bits_or_error(_py, self_bits, "as_integer_ratio")
        else {
            return MoltObject::none().bits();
        };
        let one_bits = MoltObject::from_int(1).bits();
        let tuple_ptr = alloc_tuple(_py, &[num_bits, one_bits]);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_conjugate(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(out_bits) = int_method_value_bits_or_error(_py, self_bits, "conjugate") else {
            return MoltObject::none().bits();
        };
        if obj_from_bits(out_bits).as_ptr().is_some() {
            inc_ref_bits(_py, out_bits);
        }
        out_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_is_integer(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if int_method_value_bits_or_error(_py, self_bits, "is_integer").is_none() {
            return MoltObject::none().bits();
        }
        MoltObject::from_bool(true).bits()
    })
}

#[inline(always)]
unsafe fn int_from_default_exception_single_inline_int_str(
    _py: &PyToken<'_>,
    val_bits: u64,
) -> Option<u64> {
    let ptr = maybe_ptr_from_bits(val_bits)?;
    unsafe {
        if object_type_id(ptr) != TYPE_ID_EXCEPTION {
            return None;
        }
        let msg_bits = exception_msg_bits(ptr);
        if !exception_message_is_lazy(msg_bits)
            && !crate::object::ops_format::exception_uses_cached_message_str(_py, ptr)
        {
            return None;
        }
        let args_bits = exception_args_bits(ptr);
        if exception_args_is_lazy_single(args_bits) {
            let arg = obj_from_bits(exception_args_payload_bits(ptr));
            return arg.as_int().map(|value| MoltObject::from_int(value).bits());
        }
        let args_ptr = maybe_ptr_from_bits(args_bits)?;
        if object_type_id(args_ptr) != TYPE_ID_TUPLE || tuple_len(args_ptr) != 1 {
            return None;
        }
        let arg_bits = seq_vec_ref(args_ptr)[0];
        let arg = obj_from_bits(arg_bits);
        arg.as_int().map(|value| MoltObject::from_int(value).bits())
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_from_str_of_obj(
    val_bits: u64,
    base_bits: u64,
    has_base_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let has_base = to_i64(obj_from_bits(has_base_bits)).unwrap_or(0) != 0;
        if !has_base
            && let Some(bits) =
                unsafe { int_from_default_exception_single_inline_int_str(_py, val_bits) }
        {
            return bits;
        }

        let str_bits = molt_str_from_obj(val_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let out_bits = molt_int_from_obj(str_bits, base_bits, has_base_bits);
        dec_ref_bits(_py, str_bits);
        out_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_from_obj(val_bits: u64, base_bits: u64, has_base_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(val_bits);
        let has_base = to_i64(obj_from_bits(has_base_bits)).unwrap_or(0) != 0;
        let base_val = if has_base {
            let base = index_i64_from_obj(_py, base_bits, "int() base must be int");
            if base != 0 && !(2..=36).contains(&base) {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "int() base must be >= 2 and <= 36, or 0",
                );
            }
            base
        } else {
            10
        };
        let invalid_literal = |base: i64, literal: &str| -> u64 {
            let msg = format!("invalid literal for int() with base {base}: '{literal}'");
            raise_exception::<_>(_py, "ValueError", &msg)
        };
        if has_base {
            let Some(ptr) = maybe_ptr_from_bits(val_bits) else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "int() can't convert non-string with explicit base",
                );
            };
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id != TYPE_ID_STRING
                    && type_id != TYPE_ID_BYTES
                    && type_id != TYPE_ID_BYTEARRAY
                {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "int() can't convert non-string with explicit base",
                    );
                }
            }
        }
        if !has_base {
            if complex_ptr_from_bits(val_bits).is_some() {
                let type_label = type_name(_py, obj);
                let msg = format!(
                    "int() argument must be a string, a bytes-like object or a real number, not '{type_label}'"
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            // A bare heap BigInt is already an exact `int`; return it unchanged
            // (preserving the object). Checked BEFORE the `to_i64` fast path so a
            // BigInt whose magnitude fits in i64 but exceeds the 47-bit inline
            // window is NOT re-boxed through the inline path (which truncates).
            // Int *subclasses* are not bare BigInts here, so they still fall
            // through to the value-extracting path below that strips to base int.
            // `int()` returns an OWNED reference, so retain the aliased object.
            if bigint_ptr_from_bits(val_bits).is_some() {
                inc_ref_bits(_py, val_bits);
                return val_bits;
            }
            if let Some(i) = to_i64(obj) {
                // Full-range boxing (inline for small magnitudes, heap BigInt
                // for |i| >= 2**46) — never the inline-only `from_int`, which
                // would silently truncate values in [2**46, 2**63).
                return int_bits_from_i64(_py, i);
            }
            if let Some(f) = to_f64(obj) {
                if f.is_nan() {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "cannot convert float NaN to integer",
                    );
                }
                if f.is_infinite() {
                    return raise_exception::<_>(
                        _py,
                        "OverflowError",
                        "cannot convert float infinity to integer",
                    );
                }
                let big = bigint_from_f64_trunc(f);
                if let Some(i) = bigint_to_inline(&big) {
                    return MoltObject::from_int(i).bits();
                }
                return bigint_bits(_py, big);
            }
        }
        if let Some(ptr) = maybe_ptr_from_bits(val_bits) {
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_STRING {
                    let len = string_len(ptr);
                    let bytes = std::slice::from_raw_parts(string_bytes(ptr), len);
                    let text = match std::str::from_utf8(bytes) {
                        Ok(val) => val,
                        Err(_) => return invalid_literal(base_val, "<bytes>"),
                    };
                    let base = if has_base { base_val } else { 10 };
                    if base == 10
                        && let Some(i) = parse_simple_ascii_decimal_i64(text)
                    {
                        return int_bits_from_i64(_py, i);
                    }
                    let (parsed, _base_used) = match parse_int_from_str(text, base) {
                        Ok(val) => val,
                        Err(_) => return invalid_literal(base, text),
                    };
                    if let Some(i) = bigint_to_inline(&parsed) {
                        return MoltObject::from_int(i).bits();
                    }
                    return bigint_bits(_py, parsed);
                }
                if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
                    let len = bytes_len(ptr);
                    let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                    let text = String::from_utf8_lossy(bytes);
                    let base = if has_base { base_val } else { 10 };
                    if base == 10
                        && let Some(i) = parse_simple_ascii_decimal_i64(&text)
                    {
                        return int_bits_from_i64(_py, i);
                    }
                    let (parsed, _base_used) = match parse_int_from_str(&text, base) {
                        Ok(val) => val,
                        Err(_) => return invalid_literal(base, &format!("b'{text}'")),
                    };
                    if let Some(i) = bigint_to_inline(&parsed) {
                        return MoltObject::from_int(i).bits();
                    }
                    return bigint_bits(_py, parsed);
                }
                if !has_base {
                    let int_name_bits =
                        intern_static_name(_py, &runtime_state(_py).interned.int_name, b"__int__");
                    if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, int_name_bits)
                    {
                        let res_bits = call_callable0(_py, call_bits);
                        dec_ref_bits(_py, call_bits);
                        let res_obj = obj_from_bits(res_bits);
                        // Bare BigInt result: return as-is, BEFORE the to_i64
                        // path, so a fit-i64 BigInt is not re-boxed through the
                        // truncating inline path.
                        if bigint_ptr_from_bits(res_bits).is_some() {
                            return res_bits;
                        }
                        if let Some(i) = to_i64(res_obj) {
                            // Full-range boxing, never inline-only `from_int`.
                            return int_bits_from_i64(_py, i);
                        }
                        let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                        if res_obj.as_ptr().is_some() {
                            dec_ref_bits(_py, res_bits);
                        }
                        let msg = format!("__int__ returned non-int (type {res_type})");
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    let index_name_bits = intern_static_name(
                        _py,
                        &runtime_state(_py).interned.index_name,
                        b"__index__",
                    );
                    if let Some(call_bits) =
                        attr_lookup_ptr_allow_missing(_py, ptr, index_name_bits)
                    {
                        let res_bits = call_callable0(_py, call_bits);
                        dec_ref_bits(_py, call_bits);
                        let res_obj = obj_from_bits(res_bits);
                        // Bare BigInt result: return as-is, BEFORE the to_i64
                        // path, so a fit-i64 BigInt is not re-boxed through the
                        // truncating inline path.
                        if bigint_ptr_from_bits(res_bits).is_some() {
                            return res_bits;
                        }
                        if let Some(i) = to_i64(res_obj) {
                            // Full-range boxing, never inline-only `from_int`.
                            return int_bits_from_i64(_py, i);
                        }
                        let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                        if res_obj.as_ptr().is_some() {
                            dec_ref_bits(_py, res_bits);
                        }
                        let msg = format!("__index__ returned non-int (type {res_type})");
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                }
            }
        }
        if has_base {
            return raise_exception::<_>(_py, "ValueError", "invalid literal for int()");
        }
        raise_exception::<_>(
            _py,
            "TypeError",
            "int() argument must be a string or a number",
        )
    })
}

/// Deforested `int(s.split(sep)[idx])`: parse a base-10 int directly from the
/// split field's bytes WITHOUT materializing the field as a heap `str`.
///
/// The frontend's split-field consumer fusion rewrites `int(field)` (where
/// `field = s.split(sep)[idx]` and `field` is read-only and used exactly once)
/// to this op, eliminating the per-field `alloc_string` that dominated the
/// csv/etl ETL profiles for numeric (digit-leading, non-interned) fields. It
/// also heals the general real-world `int(s.split(sep)[k])` idiom.
///
/// Byte-identical to `molt_int_from_obj` on the string the field WOULD have
/// materialized to (no explicit base): the same `parse_simple_ascii_decimal_i64`
/// fast path, the same `parse_int_from_str(_, 10)` BigInt fallback (so leading
/// zeros / sign / `_` separators / bigint overflow all match), and the same
/// `ValueError("invalid literal for int() with base 10: '<field>'")` on an
/// invalid literal — where `<field>` is the field text, exactly as CPython
/// reports. An out-of-range field index raises the same
/// `IndexError("list index out of range")` the materializing path would.
#[unsafe(no_mangle)]
pub extern "C" fn molt_string_split_field_to_int(
    hay_bits: u64,
    needle_bits: u64,
    index_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let Some((hay_ptr, needle_ptr, target_index)) =
                crate::object::ops_string::explicit_split_field_args(
                    _py,
                    hay_bits,
                    needle_bits,
                    index_bits,
                )
            else {
                return MoltObject::none().bits();
            };
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            let needle_bytes =
                std::slice::from_raw_parts(string_bytes(needle_ptr), string_len(needle_ptr));
            let Some((start, end)) = crate::object::ops_string::split_field_bounds_at_index(
                hay_bytes,
                needle_bytes,
                target_index,
            ) else {
                return raise_exception::<_>(_py, "IndexError", "list index out of range");
            };
            // The field is a sub-slice of a valid UTF-8 `str` split on a UTF-8
            // separator, so `[start..end]` is itself valid UTF-8 (split never
            // bisects a multibyte codepoint).
            let field = std::str::from_utf8_unchecked(&hay_bytes[start..end]);
            if let Some(i) = parse_simple_ascii_decimal_i64(field) {
                return int_bits_from_i64(_py, i);
            }
            let (parsed, _base_used) = match parse_int_from_str(field, 10) {
                Ok(val) => val,
                Err(_) => {
                    let msg = format!("invalid literal for int() with base 10: '{field}'");
                    return raise_exception::<_>(_py, "ValueError", &msg);
                }
            };
            if let Some(i) = bigint_to_inline(&parsed) {
                return MoltObject::from_int(i).bits();
            }
            bigint_bits(_py, parsed)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_guard_type(val_bits: u64, expected_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let expected = match to_i64(obj_from_bits(expected_bits)) {
            Some(val) => val,
            None => return raise_exception::<_>(_py, "TypeError", "guard type tag must be int"),
        };
        if expected == TYPE_TAG_ANY {
            return val_bits;
        }
        let obj = obj_from_bits(val_bits);
        let matches = match expected {
            TYPE_TAG_INT => obj.is_int() || bigint_ptr_from_bits(val_bits).is_some(),
            TYPE_TAG_FLOAT => obj.is_float(),
            TYPE_TAG_COMPLEX => obj
                .as_ptr()
                .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_COMPLEX }),
            TYPE_TAG_BOOL => obj.is_bool(),
            TYPE_TAG_NONE => obj.is_none(),
            TYPE_TAG_STR => obj
                .as_ptr()
                .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_STRING }),
            TYPE_TAG_BYTES => obj
                .as_ptr()
                .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_BYTES }),
            TYPE_TAG_BYTEARRAY => obj
                .as_ptr()
                .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_BYTEARRAY }),
            TYPE_TAG_LIST => obj
                .as_ptr()
                .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_LIST }),
            TYPE_TAG_TUPLE => obj
                .as_ptr()
                .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_TUPLE }),
            TYPE_TAG_INTARRAY => obj
                .as_ptr()
                .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_INTARRAY }),
            TYPE_TAG_DICT => obj
                .as_ptr()
                .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_DICT }),
            TYPE_TAG_SET => obj
                .as_ptr()
                .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_SET }),
            TYPE_TAG_FROZENSET => obj
                .as_ptr()
                .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_FROZENSET }),
            TYPE_TAG_RANGE => obj
                .as_ptr()
                .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_RANGE }),
            TYPE_TAG_SLICE => obj
                .as_ptr()
                .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_SLICE }),
            TYPE_TAG_DATACLASS => obj
                .as_ptr()
                .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_DATACLASS }),
            TYPE_TAG_BUFFER2D => obj
                .as_ptr()
                .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_BUFFER2D }),
            TYPE_TAG_MEMORYVIEW => obj
                .as_ptr()
                .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_MEMORYVIEW }),
            _ => false,
        };
        if !matches {
            profile_hit_unchecked(&GUARD_TAG_TYPE_MISMATCH_DEOPT_COUNT);
            // Deopt: return the value as-is instead of raising TypeError.
            // Type guards are performance hints, not correctness invariants.
            // Raising on mismatch breaks valid code that passes subtypes
            // (e.g., version_info tuple subclass where tuple[int,...] expected).
        }
        val_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_is_truthy(val: u64) -> i64 {
    crate::with_gil_entry_nopanic!(_py, {
        let result = is_truthy(_py, obj_from_bits(val));
        if exception_pending(_py) {
            return 0;
        }
        if result { 1 } else { 0 }
    })
}

/// Fast truthy check for known-int values. Zero is falsy, everything else is truthy.
/// Skips the 24-type dispatch chain in molt_is_truthy.
#[unsafe(no_mangle)]
pub extern "C" fn molt_is_truthy_int(bits: u64) -> i64 {
    let obj = obj_from_bits(bits);
    if let Some(i) = crate::to_i64(obj) {
        if i != 0 { 1 } else { 0 }
    } else if obj.is_bool() {
        if obj.as_bool().unwrap_or(false) { 1 } else { 0 }
    } else {
        // Fallback for unexpected types (e.g. runtime type widening)
        molt_is_truthy(bits)
    }
}

/// Fast truthy check for known-bool values. False is falsy, True is truthy.
/// Skips the 24-type dispatch chain in molt_is_truthy.
#[unsafe(no_mangle)]
pub extern "C" fn molt_is_truthy_bool(bits: u64) -> i64 {
    let obj = obj_from_bits(bits);
    if obj.is_bool() {
        if obj.as_bool().unwrap_or(false) { 1 } else { 0 }
    } else {
        // Fallback for unexpected types (e.g. runtime type widening)
        molt_is_truthy(bits)
    }
}

/// GIL-free truthy check for known-int values.
///
/// Identical to `molt_is_truthy_int` but named `_nogil` to make the contract
/// explicit: this function performs NO GIL acquisition, NO catch_unwind, and
/// NO pending-signal checks.  It is safe to call from hot loops on values
/// whose type is statically known to be `int` (or `bool`).
///
/// Fallback: if the NaN-boxed value is neither int nor bool, delegates to
/// `molt_is_truthy_int` which itself falls back to the full GIL-wrapped
/// `molt_is_truthy`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_is_truthy_int_nogil(bits: u64) -> i64 {
    // Delegate to the existing GIL-free implementation.
    // This is not a wrapper for code-size reasons — the compiler can inline
    // the call at LTO time, and having a single implementation avoids drift.
    molt_is_truthy_int(bits)
}

/// GIL-free truthy check for known-bool values.
///
/// Same contract as `molt_is_truthy_int_nogil`: no GIL, no catch_unwind,
/// no signal checks.
#[unsafe(no_mangle)]
pub extern "C" fn molt_is_truthy_bool_nogil(bits: u64) -> i64 {
    molt_is_truthy_bool(bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_not(val: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        // NOTE: Do NOT check exception_pending here.  `not` may be
        // called inside an exception handler where a pending exception
        // is expected (e.g. the `not(is(exception_last, None))` idiom
        // that checks whether an exception was raised).  Short-circuiting
        // on the stale pending flag returns None instead of the correct
        // boolean, breaking the handler's control flow.  If is_truthy
        // itself raises (custom __bool__), the caller's check_exception
        // op will propagate it.
        let result = is_truthy(_py, obj_from_bits(val));
        MoltObject::from_bool(!result).bits()
    })
}

pub(crate) fn env_flag_enabled(name: &str) -> bool {
    std::env::var(name)
        .map(|val| !val.is_empty() && val != "0")
        .unwrap_or(false)
}

pub(crate) fn maybe_emit_runtime_feedback_file(payload: &serde_json::Value) {
    if !env_flag_enabled("MOLT_RUNTIME_FEEDBACK") {
        return;
    }
    let out_path = std::env::var("MOLT_RUNTIME_FEEDBACK_FILE")
        .ok()
        .filter(|val| !val.is_empty())
        .unwrap_or_else(|| "molt_runtime_feedback.json".to_string());
    let path = std::path::Path::new(&out_path);
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && let Err(err) = std::fs::create_dir_all(parent)
    {
        eprintln!(
            "molt_runtime_feedback_error stage=create_dir path={} err={}",
            path.display(),
            err
        );
        return;
    }
    let encoded = match serde_json::to_string_pretty(payload) {
        Ok(value) => value,
        Err(err) => {
            eprintln!(
                "molt_runtime_feedback_error stage=encode path={} err={}",
                path.display(),
                err
            );
            return;
        }
    };
    if let Err(err) = std::fs::write(path, encoded) {
        eprintln!(
            "molt_runtime_feedback_error stage=write path={} err={}",
            path.display(),
            err
        );
        return;
    }
    eprintln!("molt_runtime_feedback_file {}", path.display());
}
