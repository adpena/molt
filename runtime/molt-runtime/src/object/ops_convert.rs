// Type conversion operations.
// Split from ops.rs for compilation-unit size reduction.

use crate::BuiltinExc;
use crate::const_data_cache::{
    ConstDataLiteralKind, const_data_literal_insert, const_data_literal_lookup,
};
use crate::object::accessors::object_field_init_ptr_raw;
use crate::object::ops::{as_float_extended, float_result_bits};
use crate::object::ops_format::{format_bytes, format_string_repr_bytes};
use crate::*;
use molt_obj_model::MoltObject;
use molt_obj_model::float_literal::{format_hex_float, parse_hex_float};
use molt_obj_model::int_literal::{IntLiteralErrorKind, scan_int_literal_with_limit};
use num_bigint::BigInt;
use num_integer::Integer;
use num_traits::{Signed, Zero};

/// Format an i64 into a stack-allocated buffer, returning the UTF-8 slice.
/// This avoids the heap allocation of `i.to_string()` in the hot path.
#[inline(always)]
pub(crate) fn int_to_stack_str(val: i64, buf: &mut [u8; 24]) -> &[u8] {
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

pub(crate) fn molt_str_from_obj_attached_body(_py: &crate::PyToken<'_>, val_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let obj = obj_from_bits(val_bits);
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
        crate::object::ops_format::format_obj_str_bits(_py, obj)
    })
}

pub(crate) fn molt_repr_from_obj_attached_body(_py: &crate::PyToken<'_>, val_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        crate::object::ops_format::format_obj_bits(py, obj_from_bits(val_bits))
    })
}

fn ascii_escape(bytes: &[u8]) -> String {
    // SIMD fast path: if entire string is ASCII, return as-is (common case)
    if bytes.is_ascii() {
        return String::from_utf8(bytes.to_vec()).expect("ASCII prefix");
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

    let mut out = String::with_capacity(bytes.len());
    // Copy the all-ASCII prefix in bulk
    out.push_str(std::str::from_utf8(&bytes[..first_non_ascii]).expect("ASCII prefix"));
    // Process remaining characters
    for code in molt_obj_model::text_storage::code_points(&bytes[first_non_ascii..]) {
        if code < 128 {
            out.push(code as u8 as char);
        } else {
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

pub(crate) fn molt_ascii_from_obj_attached_body(_py: &crate::PyToken<'_>, val_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let rendered_bits = molt_repr_from_obj_attached_body(_py, val_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, rendered_bits);
            return MoltObject::none().bits();
        }
        let rendered = crate::object::ops_format::string_obj_bytes(obj_from_bits(rendered_bits))
            .expect("validated repr string");
        if rendered.is_ascii() {
            return rendered_bits;
        }
        let escaped = ascii_escape(&rendered);
        dec_ref_bits(_py, rendered_bits);
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

pub(crate) fn molt_bin_builtin_attached_body(_py: &crate::PyToken<'_>, val_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let Some(value) = index_bigint_from_obj(
            _py,
            val_bits,
            crate::builtins::numbers::not_an_integer(_py, val_bits),
        ) else {
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

pub(crate) fn molt_oct_builtin_attached_body(_py: &crate::PyToken<'_>, val_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let Some(value) = index_bigint_from_obj(
            _py,
            val_bits,
            crate::builtins::numbers::not_an_integer(_py, val_bits),
        ) else {
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

pub(crate) fn molt_hex_builtin_attached_body(_py: &crate::PyToken<'_>, val_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let Some(value) = index_bigint_from_obj(
            _py,
            val_bits,
            crate::builtins::numbers::not_an_integer(_py, val_bits),
        ) else {
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

use molt_obj_model::unicode::numeric_whitespace;

fn parse_float_from_bytes(
    data: Option<&molt_obj_model::unicode::UnicodeData>,
    bytes: &[u8],
) -> Result<f64, ()> {
    let text = std::str::from_utf8(bytes).map_err(|_| ())?;
    let trimmed = text.trim_matches(|ch: char| numeric_whitespace(data, ch));
    trimmed.parse::<f64>().map_err(|_| ())
}

fn parse_complex_from_str(
    data: &molt_obj_model::unicode::UnicodeData,
    text: &str,
) -> Result<ComplexParts, ()> {
    let mut trimmed = text.trim_matches(|ch| numeric_whitespace(Some(data), ch));
    if trimmed.is_empty() {
        return Err(());
    }
    if trimmed.starts_with('(') && trimmed.ends_with(')') && trimmed.len() >= 2 {
        trimmed =
            trimmed[1..trimmed.len() - 1].trim_matches(|ch| numeric_whitespace(Some(data), ch));
        if trimmed.is_empty() {
            return Err(());
        }
    }
    if trimmed.chars().any(|ch| numeric_whitespace(Some(data), ch)) {
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
            let real = parse_float_from_bytes(Some(data), real_part.as_bytes())?;
            let imag = if imag_part == "+" {
                1.0
            } else if imag_part == "-" {
                -1.0
            } else {
                parse_float_from_bytes(Some(data), imag_part.as_bytes())?
            };
            return Ok(ComplexParts { re: real, im: imag });
        }
        let imag = parse_float_from_bytes(Some(data), core.as_bytes())?;
        return Ok(ComplexParts { re: 0.0, im: imag });
    }
    let real = parse_float_from_bytes(Some(data), trimmed.as_bytes())?;
    Ok(ComplexParts { re: real, im: 0.0 })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ParseIntError {
    Invalid,
    TooManyDigits { limit: usize, digits: usize },
}

fn parse_int_literal(
    bytes: &[u8],
    base: i64,
    max_str_digits: usize,
) -> Result<BigInt, ParseIntError> {
    let base_arg = i32::try_from(base).map_err(|_| ParseIntError::Invalid)?;
    let scanned = scan_int_literal_with_limit(bytes, base_arg, max_str_digits).map_err(
        |error| match error.kind {
            IntLiteralErrorKind::TooManyDigits => ParseIntError::TooManyDigits {
                limit: max_str_digits,
                digits: error.offset,
            },
            IntLiteralErrorKind::InvalidBase | IntLiteralErrorKind::InvalidLiteral => {
                ParseIntError::Invalid
            }
        },
    )?;
    let radix = BigInt::from(scanned.base);
    let mut parsed = BigInt::zero();
    for digit in scanned.digits {
        parsed = parsed * &radix + BigInt::from(digit);
    }
    if scanned.negative {
        parsed = -parsed;
    }
    Ok(parsed)
}

#[inline(always)]
fn parse_simple_ascii_decimal_i64(bytes: &[u8]) -> Option<i64> {
    let (&first, rest) = bytes.split_first()?;
    let digits = match first {
        b'+' | b'-' => rest,
        _ => bytes,
    };
    const _: () = assert!(molt_runtime_core::integer_decimal::INT_STR_DIGITS_CHECK_THRESHOLD >= 19);
    // Nineteen digits fit in u64 and below any configured digit limit.
    // The general scanner counts longer inputs, including leading zeros.
    if digits.is_empty() || digits.len() > 19 {
        return None;
    }
    let mut value = 0u64;
    for &byte in digits {
        let digit = byte.wrapping_sub(b'0');
        if digit >= 10 {
            return None;
        }
        value = value * 10 + u64::from(digit);
    }
    let negative = first == b'-';
    if negative && value == i64::MIN.unsigned_abs() {
        return Some(i64::MIN);
    }
    let signed = i64::try_from(value).ok()?;
    Some(if negative { -signed } else { signed })
}

#[derive(Clone, Copy)]
enum IntTextKind {
    Unicode,
    Bytes,
}

fn int_from_text(py: &PyToken<'_>, bytes: &[u8], base: i64, kind: IntTextKind) -> u64 {
    if base == 10
        && let Some(value) = parse_simple_ascii_decimal_i64(bytes)
    {
        return int_bits_from_i64(py, value);
    }
    int_from_text_general(py, bytes, base, kind)
}

#[cold]
fn int_from_text_general(py: &PyToken<'_>, bytes: &[u8], base: i64, kind: IntTextKind) -> u64 {
    let input = match kind {
        IntTextKind::Unicode => molt_obj_model::int_literal::normalize_unicode_int_literal(
            bytes,
            super::ops::unicode_data_for_bytes(py, bytes),
        ),
        IntTextKind::Bytes => std::borrow::Cow::Borrowed(bytes),
    };
    match parse_int_literal(
        &input,
        base,
        crate::builtins::sys_ext::current_int_max_str_digits(py),
    ) {
        Ok(parsed) => int_bits_from_bigint(py, parsed),
        Err(ParseIntError::Invalid) => {
            let repr = match kind {
                IntTextKind::Unicode => {
                    format_string_repr_bytes(super::ops::unicode_data_for_bytes(py, bytes), bytes)
                }
                IntTextKind::Bytes => format_bytes(&bytes[..bytes.len().min(200)]),
            };
            let end = repr
                .char_indices()
                .nth(200)
                .map_or(repr.len(), |(end, _)| end);
            raise_exception::<_>(
                py,
                BuiltinExc::ValueError,
                &format!(
                    "invalid literal for int() with base {base}: {}",
                    &repr[..end]
                ),
            )
        }
        Err(ParseIntError::TooManyDigits { limit, digits }) => raise_exception::<_>(
            py,
            BuiltinExc::ValueError,
            &format!(
                "Exceeds the limit ({limit} digits) for integer string conversion: value has {digits} digits; use sys.set_int_max_str_digits() to increase the limit"
            ),
        ),
    }
}

/// # Safety
/// - `ptr` must be null or valid for `len_bits` bytes.
/// - Nonempty data must be immutable compiler-emitted storage whose lifetime
///   spans this runtime generation. The pointer and length are cache identity.
/// - The compiler IR admission authority must have validated minimal signed
///   big-endian bytes. This internal materializer is not a public byte parser.
///
/// Invalid extent or failed materialization returns None with a pending exception.
pub(crate) unsafe fn molt_bigint_from_bytes_attached_body(
    _py: &crate::PyToken<'_>,
    ptr: *const u8,
    len_bits: u64,
) -> u64 {
    unsafe {
        crate::with_attached_entry_nopanic!(_py, _py, {
            let Some(len) = usize_from_bits(len_bits).filter(|len| *len <= isize::MAX as usize)
            else {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::OverflowError,
                    "integer literal byte length is too large",
                );
            };
            if ptr.is_null() || len == 0 {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::ValueError,
                    "integer literal requires nonempty signed bytes",
                );
            }
            let data_key = ptr as usize;
            if let Some(bits) =
                const_data_literal_lookup(_py, ConstDataLiteralKind::BigInt, data_key, len)
            {
                return bits;
            }
            let bytes = std::slice::from_raw_parts(ptr, len);
            // The compiler admits minimal signed big-endian bytes. Reuse the
            // Python int.from_bytes / C byte-array decoder: internal constants
            // never pass through the user-facing decimal digit policy.
            let parsed = crate::builtins::numbers::bigint_from_bytes(bytes, false, true);
            let bits = if let Some(i) = bigint_to_inline(&parsed) {
                MoltObject::from_int(i).bits()
            } else {
                bigint_bits(_py, parsed)
            };
            if !obj_from_bits(bits).is_none() {
                const_data_literal_insert(_py, ConstDataLiteralKind::BigInt, data_key, len, bits);
            }
            bits
        })
    }
}

pub(crate) fn molt_float_from_obj_attached_body(_py: &crate::PyToken<'_>, val_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        // Preserve identity only for an exact float. A physical float payload
        // can carry subclass identity and must dispatch its __float__ slot.
        if crate::builtins::numbers::is_exact_float(_py, val_bits) {
            inc_ref_bits(_py, val_bits);
            return val_bits;
        }
        if let Some(value) = crate::builtins::numbers::float_from_number_protocol(_py, val_bits) {
            return value.into_bits(_py);
        }
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if let Some(ptr) = maybe_ptr_from_bits(val_bits) {
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_STRING {
                    let len = string_len(ptr);
                    let bytes = std::slice::from_raw_parts(string_bytes(ptr), len);
                    if let Ok(parsed) =
                        parse_float_from_bytes(Some(super::ops::unicode_data(_py)), bytes)
                    {
                        return float_result_bits(_py, parsed);
                    }
                    // CPython embeds `repr(arg)`: a str renders with quote
                    // selection and escapes (`"it's"`, `'a\nb'`), not raw bytes.
                    let msg = format!(
                        "could not convert string to float: {}",
                        format_string_repr_bytes(super::ops::unicode_data(_py), bytes)
                    );
                    return raise_exception::<_>(_py, BuiltinExc::ValueError, &msg);
                }
                if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
                    let len = bytes_len(ptr);
                    let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                    if let Ok(parsed) = parse_float_from_bytes(None, bytes) {
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
                    return raise_exception::<_>(_py, BuiltinExc::ValueError, &msg);
                }
            }
        }
        let msg = format!(
            "float() argument must be a string or a real number, not '{}'",
            class_name_for_error(type_of_bits(_py, val_bits))
        );
        raise_exception::<_>(_py, BuiltinExc::TypeError, &msg)
    })
}

pub(crate) fn molt_float_new_attached_body(
    _py: &crate::PyToken<'_>,
    cls_bits: u64,
    val_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let builtins = builtin_classes(_py);
        let Some((_, cls_ptr)) = crate::builtins::type_ops::native_constructor_receiver(
            _py,
            builtins.float,
            Some(cls_bits),
            "float",
        ) else {
            return MoltObject::none().bits();
        };
        if unsafe { crate::object::class_finish_definition(_py, cls_ptr) }.is_err() {
            return MoltObject::none().bits();
        }
        let float_bits = molt_float_from_obj_attached_body(_py, val_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if cls_bits == builtins.float {
            return float_bits;
        }
        let _value_owner = obj_from_bits(float_bits)
            .as_ptr()
            .map(|__ptr| crate::PtrDropGuard::new(_py, __ptr));
        let mut projected_owner = None;
        let float_bits = if as_float_extended(obj_from_bits(float_bits)).is_some() {
            float_bits
        } else {
            let value = match crate::builtins::numbers::float_storage_value(_py, float_bits) {
                Ok(Some(value)) => value,
                Ok(None) => {
                    return raise_exception::<_>(
                        _py,
                        BuiltinExc::SystemError,
                        "float conversion returned no float storage",
                    );
                }
                Err(()) => return MoltObject::none().bits(),
            };
            let projected = float_result_bits(_py, value);
            projected_owner = obj_from_bits(projected)
                .as_ptr()
                .map(|__ptr| crate::PtrDropGuard::preserving(_py, __ptr));
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            projected
        };
        let _projected_owner = projected_owner;
        let inst_bits = unsafe { alloc_instance_for_class(_py, cls_ptr) };
        let Some(inst_ptr) = obj_from_bits(inst_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        let mut instance_owner = crate::PtrDropGuard::new(_py, inst_ptr);
        let Some(offset) = (unsafe {
            crate::object::class_layout::scalar_value_offset(
                _py,
                cls_ptr,
                crate::object::class_layout::ScalarValueKind::Float,
            )
        }) else {
            return MoltObject::none().bits();
        };
        if unsafe { object_field_init_ptr_raw(_py, inst_ptr, offset, float_bits) }.is_err() {
            return MoltObject::none().bits();
        }
        instance_owner.release();
        inst_bits
    })
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
    let type_label = class_name_for_error(type_of_bits(_py, self_bits));
    let msg = format!(
        "descriptor '{method}' for 'float' objects doesn't apply to a '{type_label}' object"
    );
    let _ = raise_exception::<u64>(_py, BuiltinExc::TypeError, &msg);
    None
}

pub(in crate::object) fn float_value_or_descriptor_error(
    _py: &PyToken<'_>,
    self_bits: u64,
    method: &str,
) -> Option<f64> {
    let bits = float_value_bits_or_descriptor_error(_py, self_bits, method)?;
    as_float_extended(obj_from_bits(bits))
}

pub(crate) fn molt_float_float_attached_body(_py: &crate::PyToken<'_>, self_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let Some(bits) = float_value_bits_or_descriptor_error(_py, self_bits, "__float__") else {
            return MoltObject::none().bits();
        };
        if crate::builtins::numbers::is_exact_float(_py, self_bits) {
            inc_ref_bits(_py, bits);
            bits
        } else {
            float_result_bits(_py, as_float_extended(obj_from_bits(bits)).unwrap())
        }
    })
}

pub(crate) fn molt_float_conjugate_attached_body(_py: &crate::PyToken<'_>, self_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let Some(value) = float_value_or_descriptor_error(_py, self_bits, "conjugate") else {
            return MoltObject::none().bits();
        };
        float_result_bits(_py, value)
    })
}

pub(crate) fn molt_float_is_integer_attached_body(_py: &crate::PyToken<'_>, self_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let Some(value) = float_value_or_descriptor_error(_py, self_bits, "is_integer") else {
            return MoltObject::none().bits();
        };
        let out = value.is_finite() && value.fract() == 0.0;
        MoltObject::from_bool(out).bits()
    })
}

pub(crate) fn molt_float_as_integer_ratio_attached_body(
    _py: &crate::PyToken<'_>,
    self_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let Some(value) = float_value_or_descriptor_error(_py, self_bits, "as_integer_ratio")
        else {
            return MoltObject::none().bits();
        };
        if value.is_nan() {
            return raise_exception::<_>(
                _py,
                BuiltinExc::ValueError,
                "cannot convert NaN to integer ratio",
            );
        }
        if value.is_infinite() {
            return raise_exception::<_>(
                _py,
                BuiltinExc::OverflowError,
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
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let den_bits = int_bits_from_bigint(_py, denominator);
        if exception_pending(_py) {
            dec_ref_bits(_py, num_bits);
            return MoltObject::none().bits();
        }
        let tuple_ptr = crate::object::builders::alloc_tuple_owned(_py, &[num_bits, den_bits]);
        if tuple_ptr.is_null() {
            dec_ref_bits(_py, den_bits);
            dec_ref_bits(_py, num_bits);
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

pub(crate) fn molt_float_hex_attached_body(_py: &crate::PyToken<'_>, self_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let Some(value) = float_value_or_descriptor_error(_py, self_bits, "hex") else {
            return MoltObject::none().bits();
        };
        let text = format_hex_float(value);
        let ptr = alloc_string(_py, text.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

pub(crate) fn molt_float_fromhex_attached_body(
    _py: &crate::PyToken<'_>,
    cls_bits: u64,
    text_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let text_obj = obj_from_bits(text_bits);
        // CPython's float.fromhex emits a generic, type-name-free message on all
        // supported versions (3.12/3.13/3.14) for a non-str argument.
        let Some(text_ptr) = text_obj.as_ptr() else {
            return raise_exception::<_>(
                _py,
                BuiltinExc::TypeError,
                "bad argument type for built-in operation",
            );
        };
        unsafe {
            if object_type_id(text_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "bad argument type for built-in operation",
                );
            }
            // float.fromhex uses PyUnicode_AsUTF8AndSize: surrogate failures
            // belong to the same strict export authority as native text names.
            if !crate::object::ops_string::require_strict_utf8(_py, text_bits) {
                return MoltObject::none().bits();
            }
            let bytes = std::slice::from_raw_parts(string_bytes(text_ptr), string_len(text_ptr));
            let value = match parse_hex_float(bytes) {
                Ok(value) => value,
                Err(error) => {
                    let (kind, message) = error.diagnostic();
                    return raise_exception::<_>(_py, kind, message);
                }
            };
            let out_bits = float_result_bits(_py, value);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let builtins = builtin_classes(_py);
            if cls_bits == builtins.float {
                return out_bits;
            }
            // The subclass callback borrows the parsed float. Its temporary
            // owner ends on success, callback failure or invalid-class rejection.
            let _result_owner = obj_from_bits(out_bits)
                .as_ptr()
                .map(|__ptr| PtrDropGuard::new(_py, __ptr));
            if !issubclass_bits(cls_bits, builtins.float) {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
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

/// `float.__getformat__(typestr)` names the storage layout of the C `double`
/// or `float` type; every supported CPython keeps the same contract. Rust's `f64`
/// and `f32` are IEEE 754 binary64 and binary32 on every target, so only the
/// target's byte order varies. Argument Clinic admits a str (subclasses too),
/// exports it as strict UTF-8 and rejects an embedded NUL before it compares
/// the name. The class receiver does not change the answer.
pub(crate) fn molt_float_getformat_attached_body(
    _py: &crate::PyToken<'_>,
    _cls_bits: u64,
    typestr_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let typestr = obj_from_bits(typestr_bits);
        let Some(typestr_ptr) = typestr
            .as_ptr()
            .filter(|&ptr| unsafe { object_type_id(ptr) == TYPE_ID_STRING })
        else {
            // _PyArg_BadArgument names None as "None", else tp_name at %.50s.
            let actual = if typestr.is_none() {
                "None".to_string()
            } else {
                crate::object::ops::protocol_type_name(_py, typestr_bits, 50)
            };
            let message = format!("__getformat__() argument must be str, not {actual}");
            return raise_exception::<_>(_py, BuiltinExc::TypeError, &message);
        };
        if !crate::object::ops_string::require_strict_utf8(_py, typestr_bits) {
            return MoltObject::none().bits();
        }
        let name = unsafe {
            std::slice::from_raw_parts(string_bytes(typestr_ptr), string_len(typestr_ptr))
        };
        if name.contains(&0) {
            return raise_exception::<_>(_py, BuiltinExc::ValueError, "embedded null character");
        }
        if name != b"double" && name != b"float" {
            return raise_exception::<_>(
                _py,
                BuiltinExc::ValueError,
                "__getformat__() argument 1 must be 'double' or 'float'",
            );
        }
        let layout: &[u8] = if cfg!(target_endian = "big") {
            b"IEEE, big-endian"
        } else {
            b"IEEE, little-endian"
        };
        let ptr = alloc_string(_py, layout);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

pub(crate) fn molt_float_from_number_attached_body(
    _py: &crate::PyToken<'_>,
    cls_bits: u64,
    val_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let Some(value) = crate::builtins::numbers::float_as_double(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let out_bits = float_result_bits(_py, value);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let builtins = builtin_classes(_py);
        if cls_bits == builtins.float {
            return out_bits;
        }
        if !issubclass_bits(cls_bits, builtins.float) {
            dec_ref_bits(_py, out_bits);
            return raise_exception::<_>(
                _py,
                BuiltinExc::TypeError,
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

/// The physical descriptor never dispatches through a subtype override.
pub(crate) extern "C" fn complex_complex(__context: u64, bits: u64) -> u64 {
    let __attached_py = unsafe { crate::PyToken::from_attached_context(__context) };

    crate::with_attached_entry_nopanic!(__attached_py, py, {
        let Some(ptr) = complex_ptr_from_bits(bits) else {
            return raise_exception::<_>(
                py,
                BuiltinExc::TypeError,
                "descriptor '__complex__' requires a 'complex' object",
            );
        };
        if type_of_bits(py, bits) == builtin_classes(py).complex {
            inc_ref_bits(py, bits);
            return bits;
        }
        let parts = unsafe { *complex_ref(ptr) };
        complex_bits(py, parts.re, parts.im)
    })
}

/// Whether `bits` is a real number to complex(): CPython's `nb_float` or
/// `nb_index` slot (a float, an int, or a `__float__`/`__index__` method).
fn real_number_admitted(py: &PyToken<'_>, bits: u64) -> Result<bool, ()> {
    if crate::object::ops::as_float_extended(obj_from_bits(bits)).is_some()
        || obj_from_bits(bits).is_int()
        || obj_from_bits(bits).is_bool()
        || bigint_ptr_from_bits(bits).is_some()
    {
        return Ok(true);
    }
    unsafe {
        if crate::builtins::attr::has_special_method(py, bits, crate::NameId::DunderFloat)? {
            return Ok(true);
        }
        crate::builtins::attr::has_special_method(py, bits, crate::NameId::DunderIndex)
    }
}

fn complex_number_admitted(py: &PyToken<'_>, bits: u64) -> Result<bool, ()> {
    if complex_ptr_from_bits(bits).is_some() {
        return Ok(true);
    }
    real_number_admitted(py, bits)
}

/// How complex() received its arguments: the third argument of
/// `molt_complex_from_obj`. From 3.14 CPython sends a lone positional
/// argument through `actual_complex_new` and every other shape through
/// `complex_new_impl`; earlier targets use `complex_new_impl` for all.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ComplexArguments {
    /// `real` alone, by keyword or defaulted.
    Real = 0,
    /// `real` and `imag`.
    RealImag = 1,
    /// One positional argument and no keyword.
    LonePositional = 2,
}

impl ComplexArguments {
    pub(crate) fn bits(self) -> u64 {
        MoltObject::from_int(self as i64).bits()
    }

    fn from_bits(bits: u64) -> Option<Self> {
        match to_i64(obj_from_bits(bits))? {
            0 => Some(Self::Real),
            1 => Some(Self::RealImag),
            2 => Some(Self::LonePositional),
            _ => None,
        }
    }
}

/// `%T`: the type's fully qualified name, as 3.14+ complex() errors print it.
fn complex_argument_type_name(py: &PyToken<'_>, bits: u64) -> String {
    String::from_utf8_lossy(&crate::builtins::classes::class_fully_qualified_name(
        py,
        type_of_bits(py, bits),
    ))
    .into_owned()
}

pub(crate) fn molt_complex_from_obj_attached_body(
    _py: &crate::PyToken<'_>,
    val_bits: u64,
    imag_bits: u64,
    has_imag_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        let Some(shape) = ComplexArguments::from_bits(has_imag_bits) else {
            return raise_exception::<_>(
                py,
                BuiltinExc::SystemError,
                "complex() received an invalid argument shape",
            );
        };
        let has_imag = shape == ComplexArguments::RealImag;
        // 3.14 complex_new_impl names the 'real'/'imag' argument in its errors
        // and deprecates complex values where a real number belongs;
        // actual_complex_new keeps one generic error and no warning.
        let modern = crate::object::ops_sys::runtime_target_at_least(py, 3, 14);
        let lone = modern && shape == ComplexArguments::LonePositional;
        let named = modern && !lone;
        if !has_imag && !named && type_of_bits(py, val_bits) == builtin_classes(py).complex {
            inc_ref_bits(py, val_bits);
            return val_bits;
        }
        if let Some(ptr) = obj_from_bits(val_bits).as_ptr()
            && unsafe { object_type_id(ptr) == TYPE_ID_STRING }
            && !(named && has_imag)
        {
            if has_imag {
                return raise_exception::<_>(
                    py,
                    BuiltinExc::TypeError,
                    "complex() can't take second arg if first is a string",
                );
            }
            let bytes = unsafe { std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr)) };
            let parts = std::str::from_utf8(bytes)
                .ok()
                .and_then(|text| parse_complex_from_str(super::ops::unicode_data(py), text).ok());
            return match parts {
                Some(parts) => complex_bits(py, parts.re, parts.im),
                None => raise_exception::<_>(
                    py,
                    BuiltinExc::ValueError,
                    "complex() arg is a malformed string",
                ),
            };
        }
        if has_imag
            && !named
            && obj_from_bits(imag_bits)
                .as_ptr()
                .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_STRING })
        {
            return raise_exception::<_>(
                py,
                BuiltinExc::TypeError,
                "complex() second arg can't be a string",
            );
        }

        // Only the first argument uses __complex__. The returned reference stays
        // owned through result validation and any deprecation-warning callback.
        let mut real_bits = val_bits;
        let mut real_owner = None;
        if let Some(method) = unsafe {
            match crate::builtins::attr::lookup_special_method(
                py,
                val_bits,
                crate::NameId::DunderComplex,
            )
            .into_result()
            {
                Ok(value) => value,
                Err(()) => return MoltObject::none().bits(),
            }
        } {
            let result = unsafe { call_callable0(py, method) };
            dec_ref_bits(py, method);
            if exception_pending(py) {
                dec_ref_bits(py, result);
                return MoltObject::none().bits();
            }
            let Some(ptr) = complex_ptr_from_bits(result) else {
                let message = crate::builtins::classes::slot_result_type_error(
                    py,
                    molt_cpython_abi::slot_result::SlotResult::Complex,
                    val_bits,
                    result,
                );
                dec_ref_bits(py, result);
                return crate::builtins::exceptions::raise_exception_bytes::<_>(
                    py,
                    BuiltinExc::TypeError,
                    &message,
                );
            };
            real_owner = Some(PtrDropGuard::new(py, ptr));
            if type_of_bits(py, result) != builtin_classes(py).complex
                && !crate::builtins::numbers::warn_numeric_subclass_result(
                    py,
                    molt_cpython_abi::slot_result::SlotResult::Complex,
                    val_bits,
                    result,
                )
            {
                return MoltObject::none().bits();
            }
            real_bits = result;
        }

        // CPython admits both numeric argument kinds before running __float__
        // or __index__, so an invalid imaginary argument cannot trigger a real
        // conversion callback as a side effect.
        if !match complex_number_admitted(py, real_bits) {
            Ok(admitted) => admitted,
            Err(()) => return MoltObject::none().bits(),
        } {
            let message = if lone {
                format!(
                    "complex() argument must be a string or a number, not {}",
                    complex_argument_type_name(py, real_bits)
                )
            } else if named {
                format!(
                    "complex() argument 'real' must be a real number, not {}",
                    complex_argument_type_name(py, real_bits)
                )
            } else {
                format!(
                    "complex() first argument must be a string or a number, not '{}'",
                    class_name_for_error(type_of_bits(py, real_bits))
                )
            };
            return raise_exception::<_>(py, BuiltinExc::TypeError, &message);
        }
        if has_imag
            && !match complex_number_admitted(py, imag_bits) {
                Ok(admitted) => admitted,
                Err(()) => return MoltObject::none().bits(),
            }
        {
            let message = if named {
                format!(
                    "complex() argument 'imag' must be a real number, not {}",
                    complex_argument_type_name(py, imag_bits)
                )
            } else {
                format!(
                    "complex() second argument must be a number, not '{}'",
                    class_name_for_error(type_of_bits(py, imag_bits))
                )
            };
            return raise_exception::<_>(py, BuiltinExc::TypeError, &message);
        }
        if named {
            // A complex where a real number belongs: the real argument (or its
            // __complex__ result) when the original object is not itself a
            // real number, and any complex imag.
            if complex_ptr_from_bits(real_bits).is_some()
                && !match real_number_admitted(py, val_bits) {
                    Ok(admitted) => admitted,
                    Err(()) => return MoltObject::none().bits(),
                }
                && !crate::builtins::warnings_ext::emit_deprecation_warning(
                    py,
                    &format!(
                        "complex() argument 'real' must be a real number, not {}",
                        complex_argument_type_name(py, val_bits)
                    ),
                )
            {
                return MoltObject::none().bits();
            }
            if has_imag
                && complex_ptr_from_bits(imag_bits).is_some()
                && !crate::builtins::warnings_ext::emit_deprecation_warning(
                    py,
                    &format!(
                        "complex() argument 'imag' must be a real number, not {}",
                        complex_argument_type_name(py, imag_bits)
                    ),
                )
            {
                return MoltObject::none().bits();
            }
        }

        let real_complex = complex_ptr_from_bits(real_bits);
        let (mut re, real_im) = if let Some(ptr) = real_complex {
            let parts = unsafe { *complex_ref(ptr) };
            (parts.re, parts.im)
        } else if let Some(value) = crate::object::ops::as_float_extended(obj_from_bits(real_bits))
            && lone
        {
            // actual_complex_new reads a float (subclass included) with
            // PyFloat_AsDouble: its stored value, never its __float__.
            (value, 0.0)
        } else {
            let Some(value) = crate::builtins::numbers::float_from_number_protocol(py, real_bits)
            else {
                return MoltObject::none().bits();
            };
            (value.into_value(), 0.0)
        };
        drop(real_owner);
        let mut im = if !has_imag {
            real_im
        } else if let Some(ptr) = complex_ptr_from_bits(imag_bits) {
            let parts = unsafe { *complex_ref(ptr) };
            re -= parts.im;
            parts.re
        } else {
            let Some(value) = crate::builtins::numbers::float_from_number_protocol(py, imag_bits)
            else {
                return MoltObject::none().bits();
            };
            value.into_value()
        };
        // Adding an absent zero changes its sign. Only non-orthogonal complex
        // components take the correction path used by complex_new_impl.
        if has_imag && real_complex.is_some() {
            im += real_im;
        }
        complex_bits(py, re, im)
    })
}

pub(crate) fn molt_complex_conjugate_attached_body(_py: &crate::PyToken<'_>, val_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let Some(ptr) = complex_ptr_from_bits(val_bits) else {
            return raise_exception::<_>(
                _py,
                BuiltinExc::TypeError,
                "complex.conjugate expects complex",
            );
        };
        let value = unsafe { *complex_ref(ptr) };
        complex_bits(_py, value.re, -value.im)
    })
}

pub(crate) fn molt_complex_from_number_attached_body(
    _py: &crate::PyToken<'_>,
    cls_bits: u64,
    val_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
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
                    return raise_exception::<_>(_py, BuiltinExc::TypeError, &msg);
                }
            }
        }
        // complex.from_number reads its one argument as actual_complex_new does.
        let out_bits = molt_complex_from_obj_attached_body(
            _py,
            val_bits,
            MoltObject::none().bits(),
            ComplexArguments::LonePositional.bits(),
        );
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
                BuiltinExc::TypeError,
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

pub(crate) fn molt_int_new_attached_body(
    _py: &crate::PyToken<'_>,
    cls_bits: u64,
    val_bits: u64,
    base_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let builtins = builtin_classes(_py);
        let Some((_, cls_ptr)) = crate::builtins::type_ops::native_constructor_receiver(
            _py,
            builtins.int,
            Some(cls_bits),
            "int",
        ) else {
            return MoltObject::none().bits();
        };
        if unsafe { crate::object::class_finish_definition(_py, cls_ptr) }.is_err() {
            return MoltObject::none().bits();
        }
        let has_base = base_bits != missing_bits(_py);
        let has_base_bits = MoltObject::from_int(if has_base { 1 } else { 0 }).bits();
        let int_bits = molt_int_from_obj_attached_body(_py, val_bits, base_bits, has_base_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if cls_bits == builtins.int {
            return int_bits;
        }
        let _value_owner = obj_from_bits(int_bits)
            .as_ptr()
            .map(|__ptr| crate::PtrDropGuard::new(_py, __ptr));
        let mut projected_owner = None;
        let int_bits = if crate::builtins::numbers::index_integral_payload_bits(int_bits).is_some()
        {
            int_bits
        } else {
            let Some(projected) =
                crate::builtins::numbers::managed_integer_from_storage(_py, int_bits)
            else {
                return MoltObject::none().bits();
            };
            projected_owner = obj_from_bits(projected)
                .as_ptr()
                .map(|__ptr| crate::PtrDropGuard::preserving(_py, __ptr));
            projected
        };
        let _projected_owner = projected_owner;
        let inst_bits = unsafe { alloc_instance_for_class(_py, cls_ptr) };
        let Some(inst_ptr) = obj_from_bits(inst_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        let mut instance_owner = crate::PtrDropGuard::new(_py, inst_ptr);
        let Some(offset) = (unsafe {
            crate::object::class_layout::scalar_value_offset(
                _py,
                cls_ptr,
                crate::object::class_layout::ScalarValueKind::Int,
            )
        }) else {
            return MoltObject::none().bits();
        };
        if unsafe { object_field_init_ptr_raw(_py, inst_ptr, offset, int_bits) }.is_err() {
            return MoltObject::none().bits();
        }
        instance_owner.release();
        inst_bits
    })
}

pub(crate) fn molt_int_int_attached_body(_py: &crate::PyToken<'_>, self_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        if let Some(value) = crate::builtins::numbers::exact_integer_from_storage(_py, self_bits) {
            return value;
        }
        let type_label = class_name_for_error(type_of_bits(_py, self_bits));
        let msg = format!(
            "descriptor '__int__' requires a 'int' object but received '{}'",
            type_label
        );
        raise_exception::<_>(_py, BuiltinExc::TypeError, &msg)
    })
}

pub(crate) fn molt_int_index_attached_body(_py: &crate::PyToken<'_>, self_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        if let Some(value) = crate::builtins::numbers::exact_integer_from_storage(_py, self_bits) {
            return value;
        }
        let type_label = class_name_for_error(type_of_bits(_py, self_bits));
        let msg = format!(
            "descriptor '__index__' requires a 'int' object but received '{}'",
            type_label
        );
        raise_exception::<_>(_py, BuiltinExc::TypeError, &msg)
    })
}

pub(crate) fn molt_int_bit_length_attached_body(_py: &crate::PyToken<'_>, self_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let obj = obj_from_bits(self_bits);
        let Some(value) = to_bigint(obj) else {
            let type_label = class_name_for_error(type_of_bits(_py, self_bits));
            let msg = format!(
                "descriptor 'bit_length' requires a 'int' object but received '{}'",
                type_label
            );
            return raise_exception::<_>(_py, BuiltinExc::TypeError, &msg);
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

pub(in crate::object) fn int_method_value_bits_or_error(
    _py: &PyToken<'_>,
    self_bits: u64,
    method: &str,
) -> Option<u64> {
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
    let _ = raise_exception::<u64>(_py, BuiltinExc::TypeError, &msg);
    None
}

pub(crate) fn molt_int_bit_count_attached_body(_py: &crate::PyToken<'_>, self_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
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

pub(crate) fn molt_int_as_integer_ratio_attached_body(
    _py: &crate::PyToken<'_>,
    self_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
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

pub(crate) fn molt_int_conjugate_attached_body(_py: &crate::PyToken<'_>, self_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let Some(out_bits) = int_method_value_bits_or_error(_py, self_bits, "conjugate") else {
            return MoltObject::none().bits();
        };
        if obj_from_bits(out_bits).as_ptr().is_some() {
            inc_ref_bits(_py, out_bits);
        }
        out_bits
    })
}

pub(crate) fn molt_int_is_integer_attached_body(_py: &crate::PyToken<'_>, self_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        if int_method_value_bits_or_error(_py, self_bits, "is_integer").is_none() {
            return MoltObject::none().bits();
        }
        MoltObject::from_bool(true).bits()
    })
}

pub(crate) fn molt_int_from_str_of_obj_attached_body(
    _py: &crate::PyToken<'_>,
    val_bits: u64,
    base_bits: u64,
    has_base_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        if obj_from_bits(has_base_bits).as_bool() == Some(false) {
            let object = obj_from_bits(val_bits);
            let inline = if object.as_int().is_some() {
                Some(val_bits)
            } else {
                crate::builtins::exceptions::rendering::builtin_exception_inline_decimal_arg(
                    _py, object,
                )
            };
            // The exact integer's decimal representation round trips, fits
            // below the digit limit, and cannot call Python code. Only the
            // enclosing conversion could observe the intermediate string.
            if let Some(bits) = inline {
                return if exception_pending(_py) {
                    MoltObject::none().bits()
                } else {
                    bits
                };
            }
        }
        let str_bits = molt_str_from_obj_attached_body(_py, val_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let out_bits = molt_int_from_obj_attached_body(_py, str_bits, base_bits, has_base_bits);
        dec_ref_bits(_py, str_bits);
        out_bits
    })
}

pub(crate) fn molt_int_from_obj_attached_body(
    _py: &crate::PyToken<'_>,
    val_bits: u64,
    base_bits: u64,
    has_base_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let has_base = to_i64(obj_from_bits(has_base_bits)).unwrap_or(0) != 0;
        let base_val = if has_base {
            let base = index_i64_from_obj(_py, base_bits, "int() base must be int");
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if base != 0 && !(2..=36).contains(&base) {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::ValueError,
                    "int() base must be >= 2 and <= 36, or 0",
                );
            }
            base
        } else {
            10
        };
        // Exact str has no numeric slots. Resolve an explicit base first
        // because its __index__ can execute Python.
        if let Some(bytes) = unsafe { super::ops::exact_string_bytes(_py, val_bits) } {
            return int_from_text(_py, bytes, base_val, IntTextKind::Unicode);
        }
        if !has_base {
            if let Some(value) = crate::builtins::numbers::int_from_number_protocol(_py, val_bits) {
                return value;
            }
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
        }
        if let Some(ptr) = maybe_ptr_from_bits(val_bits) {
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_STRING {
                    let bytes = std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr));
                    return int_from_text(_py, bytes, base_val, IntTextKind::Unicode);
                }
                if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
                    let bytes = std::slice::from_raw_parts(bytes_data(ptr), bytes_len(ptr));
                    return int_from_text(_py, bytes, base_val, IntTextKind::Bytes);
                }
            }
        }
        if has_base {
            return raise_exception::<_>(
                _py,
                BuiltinExc::TypeError,
                "int() can't convert non-string with explicit base",
            );
        }
        raise_exception::<_>(
            _py,
            BuiltinExc::TypeError,
            &format!(
                "int() argument must be a string, a bytes-like object or a real number, not '{}'",
                class_name_for_error(type_of_bits(_py, val_bits))
            ),
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
/// Parse the same Python string bytes as the materializing conversion.
/// An out-of-range index raises the same `IndexError` as the list access.
pub(crate) fn molt_string_split_field_to_int_attached_body(
    _py: &crate::PyToken<'_>,
    hay_bits: u64,
    needle_bits: u64,
    index_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
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
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::IndexError,
                    "list index out of range",
                );
            };
            int_from_text(_py, &hay_bytes[start..end], 10, IntTextKind::Unicode)
        }
    })
}

pub(crate) fn molt_is_truthy_attached_body(_py: &crate::PyToken<'_>, val: u64) -> i64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        // A handler tests its own match result while the exception it handles
        // is still pending (between `exception_last_pending` and
        // `exception_clear`). Only an exception this test raises (a failing
        // `__bool__`) makes the answer meaningless; an earlier pending one
        // does not.
        let pending_before = exception_pending(_py);
        let result = is_truthy(_py, obj_from_bits(val));
        if !pending_before && exception_pending(_py) {
            return 0;
        }
        if result { 1 } else { 0 }
    })
}

/// Fast truthy check for known-int values. Zero is falsy, everything else is truthy.
/// Skips the 24-type dispatch chain in molt_is_truthy.
pub(crate) fn molt_is_truthy_int_attached_body(_py: &crate::PyToken<'_>, bits: u64) -> i64 {
    let obj = obj_from_bits(bits);
    if let Some(i) = crate::to_i64(obj) {
        if i != 0 { 1 } else { 0 }
    } else if obj.is_bool() {
        if obj.as_bool().unwrap_or(false) { 1 } else { 0 }
    } else {
        // Fallback for unexpected types (e.g. runtime type widening)
        molt_is_truthy_attached_body(_py, bits)
    }
}

/// Fast truthy check for known-bool values. False is falsy, True is truthy.
/// Skips the 24-type dispatch chain in molt_is_truthy.
pub(crate) fn molt_is_truthy_bool_attached_body(_py: &crate::PyToken<'_>, bits: u64) -> i64 {
    let obj = obj_from_bits(bits);
    if obj.is_bool() {
        if obj.as_bool().unwrap_or(false) { 1 } else { 0 }
    } else {
        // Fallback for unexpected types (e.g. runtime type widening)
        molt_is_truthy_attached_body(_py, bits)
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
pub(crate) fn molt_is_truthy_int_nogil_attached_body(_py: &crate::PyToken<'_>, bits: u64) -> i64 {
    // Delegate to the existing GIL-free implementation.
    // This is not a wrapper for code-size reasons — the compiler can inline
    // the call at LTO time, and having a single implementation avoids drift.
    molt_is_truthy_int_attached_body(_py, bits)
}

/// GIL-free truthy check for known-bool values.
///
/// Same contract as `molt_is_truthy_int_nogil`: no GIL, no catch_unwind,
/// no signal checks.
pub(crate) fn molt_is_truthy_bool_nogil_attached_body(_py: &crate::PyToken<'_>, bits: u64) -> i64 {
    molt_is_truthy_bool_attached_body(_py, bits)
}

pub(crate) fn molt_not_attached_body(_py: &crate::PyToken<'_>, val: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
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

pub(crate) fn maybe_emit_runtime_feedback_file(py: &PyToken<'_>, payload: &serde_json::Value) {
    let Some(path) = crate::runtime_state(py).policy().feedback_path.as_ref() else {
        return;
    };
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

include!("../attached_exports_generated/object/ops_convert.rs");

#[cfg(test)]
mod int_literal_tests {
    use super::parse_int_literal;
    use num_bigint::BigInt;

    #[test]
    fn decimal_roundtrip_retains_base_and_exception_string_semantics() {
        use crate::builtins::exceptions::molt_exception_new_builtin_one_attached_body;
        use crate::{BuiltinExc, MoltObject};
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let decimal = MoltObject::from_bool(false).bits();
            let explicit = MoltObject::from_bool(true).bits();
            let hexadecimal = MoltObject::from_int(16).bits();
            let none = MoltObject::none().bits();
            let number = MoltObject::from_int(-17).bits();
            let wrapped = molt_exception_new_builtin_one_attached_body(
                py,
                BuiltinExc::ValueError as u64,
                number,
            );
            for input in [number, wrapped] {
                assert_eq!(
                    super::molt_int_from_str_of_obj_attached_body(py, input, none, decimal),
                    number,
                );
                assert_eq!(
                    super::molt_int_from_str_of_obj_attached_body(py, input, hexadecimal, explicit),
                    MoltObject::from_int(-23).bits(),
                );
            }
            crate::dec_ref_bits(py, wrapped);

            let wrapped_bool = molt_exception_new_builtin_one_attached_body(
                py,
                BuiltinExc::ValueError as u64,
                explicit,
            );
            assert_eq!(
                super::molt_int_from_str_of_obj_attached_body(py, wrapped_bool, none, decimal),
                none,
            );
            assert!(crate::exception_pending(py));
            crate::clear_exception(py);
            crate::dec_ref_bits(py, wrapped_bool);
        });
    }

    #[test]
    fn numeric_whitespace_preserves_python_string_and_bytes_domains() {
        let data = molt_obj_model::unicode::UnicodeData::for_version((3, 12)).unwrap();
        for text in ["\u{b}1.5\u{c}", "\u{a0}1.5\u{2003}"] {
            assert_eq!(
                super::parse_float_from_bytes(Some(data), text.as_bytes()),
                Ok(1.5)
            );
        }
        assert_eq!(super::parse_float_from_bytes(None, b"\x0b1.5\x0c"), Ok(1.5));
        assert!(super::parse_float_from_bytes(None, "\u{a0}1.5".as_bytes()).is_err());
        for code in 0x1c..=0x1f {
            let text = format!("{}1.5", char::from_u32(code).unwrap());
            assert!(super::parse_float_from_bytes(Some(data), text.as_bytes()).is_err());
        }
    }

    #[test]
    fn bigint_literal_invalid_inputs_return_none_with_pending_exception() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            static BYTES: &[u8] = &[0xff];
            for (ptr, len) in [
                (std::ptr::null(), 0),
                (std::ptr::null(), u64::MAX),
                (BYTES.as_ptr(), 0),
                (BYTES.as_ptr(), u64::MAX),
            ] {
                let bits = unsafe { super::molt_bigint_from_bytes_attached_body(py, ptr, len) };
                assert_eq!(bits, crate::MoltObject::none().bits());
                assert!(crate::exception_pending(py));
                crate::clear_exception(py);
            }
        });
    }

    #[test]
    fn binary_integer_literals_ignore_live_decimal_policy_on_cold_and_warm_cache() {
        use crate::builtins::exceptions::ExceptionValue;
        use crate::builtins::sys_ext::{
            current_int_max_str_digits, molt_sys_set_int_max_str_digits_attached_body,
        };
        static PAYLOAD: [u8; 401] = {
            let mut bytes = [0u8; 401];
            bytes[0] = 1;
            bytes
        };
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let _restore = std::mem::DropGuard::new(current_int_max_str_digits(py), |state| {
                {
                    crate::with_gil_entry_nopanic!(__host_py, {
                        crate::molt_sys_set_int_max_str_digits_attached_body(
                            __host_py,
                            crate::MoltObject::from_int(state as i64).bits(),
                        )
                    })
                };
            });
            let expected = BigInt::from(1u8) << 3200usize;
            for first_limit in [640, 0] {
                crate::const_data_cache::clear_const_data_literal_caches(py);
                molt_sys_set_int_max_str_digits_attached_body(
                    py,
                    crate::MoltObject::from_int(first_limit).bits(),
                );
                let cold = ExceptionValue::adopt(py, unsafe {
                    super::molt_bigint_from_bytes_attached_body(
                        py,
                        PAYLOAD.as_ptr(),
                        PAYLOAD.len() as u64,
                    )
                });
                assert!(!crate::exception_pending(py));
                assert_eq!(
                    crate::builtins::numbers::index_bigint_integral_bits(cold.bits()),
                    Some(expected.clone())
                );
                for next_limit in [0, 640] {
                    molt_sys_set_int_max_str_digits_attached_body(
                        py,
                        crate::MoltObject::from_int(next_limit).bits(),
                    );
                    let warm = ExceptionValue::adopt(py, unsafe {
                        super::molt_bigint_from_bytes_attached_body(
                            py,
                            PAYLOAD.as_ptr(),
                            PAYLOAD.len() as u64,
                        )
                    });
                    assert!(!crate::exception_pending(py));
                    assert_eq!(warm.bits(), cold.bits());
                }
            }
        });
    }

    #[test]
    fn bigint_literal_budget_failures_cover_i64_and_arbitrary_width_materialization() {
        use crate::resource::{LimitedTracker, ResourceLimits, set_tracker};
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            // Both full-width and arbitrary-width binary literals must propagate
            // allocation failure through bigint_bits.
            let payloads: [&[u8]; 2] = [
                &[0x7e, 0x9d, 0x67, 0x35, 0x2a, 0x14, 0x06, 0xea],
                &[1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            ];
            set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                max_memory: Some(0),
                ..Default::default()
            })));
            let reset = crate::test_support::restore_unlimited_tracker_on_drop();
            for bytes in payloads {
                let bits = unsafe {
                    super::molt_bigint_from_bytes_attached_body(
                        py,
                        bytes.as_ptr(),
                        bytes.len() as u64,
                    )
                };
                assert_eq!(bits, crate::MoltObject::none().bits());
                assert!(crate::exception_pending(py));
                crate::clear_exception(py);
            }
            drop(reset);
            for bytes in payloads {
                let bits = unsafe {
                    super::molt_bigint_from_bytes_attached_body(
                        py,
                        bytes.as_ptr(),
                        bytes.len() as u64,
                    )
                };
                let ptr = crate::obj_from_bits(bits)
                    .as_ptr()
                    .expect("owned bigint result");
                assert_eq!(unsafe { crate::object_type_id(ptr) }, crate::TYPE_ID_BIGINT);
                assert!(!crate::exception_pending(py));
                crate::dec_ref_bits(py, bits);
            }
        });
    }

    #[test]
    fn integer_runtime_parser_consumes_shared_cpython_lexical_authority() {
        assert_eq!(
            parse_int_literal(b"  -0x_FF  ", 0, 4300),
            Ok(BigInt::from(-255))
        );
        assert_eq!(
            parse_int_literal(b"1208925819614629174706176", 10, 4300),
            Ok(BigInt::from(1u8) << 80)
        );
        assert!(parse_int_literal(b"010", 0, 4300).is_err());
        assert!(parse_int_literal(b"1__2", 10, 4300).is_err());
        assert!(parse_int_literal(b"123 junk", 10, 4300).is_err());
        assert!(parse_int_literal(b"0x_FF", 16, 4300).is_ok());
        assert!(parse_int_literal(&vec![b'9'; 4301], 10, 4300).is_err());
        assert!(parse_int_literal(format!("0b1{}", "0".repeat(5000)).as_bytes(), 0, 4300).is_ok());
    }
}
