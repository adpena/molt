use crate::builtins::attr::lookup_special_method_bits;
// String percent-format runtime shared by `%` modulo dispatch.
// This owns the parser/conversion authority for legacy `%` formatting
// while ops_arith.rs keeps only arithmetic entrypoint dispatch.

use super::*;
use crate::object::ops_format::{
    FormatOutput, FormatWriter, format_float_value_with_spec, format_obj_output,
    format_obj_str_output,
};
use crate::object::ops_string::wtf8_step;

#[derive(Clone, Copy, Default)]
struct PercentFormatFlags {
    left_adjust: bool,
    sign_plus: bool,
    sign_space: bool,
    zero_pad: bool,
    alternate: bool,
}

fn percent_rhs_allows_unused_non_tuple(_py: &PyToken<'_>, rhs: MoltObject) -> bool {
    let Some(ptr) = rhs.as_ptr() else {
        return false;
    };
    unsafe {
        let type_id = object_type_id(ptr);
        if type_id == TYPE_ID_STRING || type_id == TYPE_ID_TUPLE {
            return false;
        }
    }
    crate::object::ops::value_supports_mp_subscript(_py, rhs.bits())
}

#[derive(Clone, Copy)]
enum PercentField {
    Width,
    Precision,
}

impl PercentField {
    fn limit(self) -> usize {
        match self {
            Self::Width => isize::MAX as usize,
            Self::Precision => i32::MAX as usize,
        }
    }
    fn too_big(self) -> &'static str {
        match self {
            Self::Width => "width too big",
            Self::Precision => "precision too big",
        }
    }
}

fn percent_parse_usize(
    py: &PyToken<'_>,
    bytes: &[u8],
    idx: &mut usize,
    field: PercentField,
) -> Option<usize> {
    let start = *idx;
    let mut out = 0usize;
    while *idx < bytes.len() && bytes[*idx].is_ascii_digit() {
        let digit = usize::from(bytes[*idx] - b'0');
        out = match out
            .checked_mul(10)
            .and_then(|value| value.checked_add(digit))
            .filter(|value| *value <= field.limit())
        {
            Some(value) => value,
            None => return raise_exception(py, "ValueError", field.too_big()),
        };
        *idx += 1;
    }
    (*idx != start).then_some(out)
}

/// '*' accepts an actual int or subtype, never an arbitrary __index__ protocol.
fn percent_star_integer(py: &PyToken<'_>, bits: u64, field: PercentField) -> Option<i64> {
    let Some(value) = crate::builtins::numbers::index_bigint_integral_bits(bits) else {
        return raise_exception(py, "TypeError", "* wants int");
    };
    let Some(value) = value.to_i64() else {
        return raise_exception(
            py,
            "OverflowError",
            match field {
                PercentField::Width => "Python int too large to convert to C ssize_t",
                PercentField::Precision => "Python int too large to convert to C int",
            },
        );
    };
    let admitted = match field {
        PercentField::Width => isize::try_from(value).is_ok(),
        PercentField::Precision => i32::try_from(value).is_ok(),
    };
    if !admitted {
        return raise_exception(
            py,
            "OverflowError",
            match field {
                PercentField::Width => "Python int too large to convert to C ssize_t",
                PercentField::Precision => "Python int too large to convert to C int",
            },
        );
    }
    Some(value)
}

fn percent_unsupported_char(
    py: &PyToken<'_>,
    text: &[u8],
    byte_idx: usize,
    codepoint: u32,
) -> Option<Vec<u8>> {
    // Parsing keeps byte offsets for lossless slices; Python diagnostics count
    // codepoints, including independently encoded surrogate codepoints.
    let mut cursor = 0;
    let mut index = 0;
    while cursor < byte_idx {
        cursor = wtf8_step(text, cursor, false)
            .expect("Python string codepoint boundary")
            .0;
        index += 1;
    }
    // unicodeobject.c deliberately prints '?' outside this ASCII range.
    let display = if (31..=126).contains(&codepoint) {
        char::from_u32(codepoint).expect("ASCII format character")
    } else {
        '?'
    };
    let message =
        format!("unsupported format character '{display}' (0x{codepoint:x}) at index {index}");
    raise_exception::<Option<Vec<u8>>>(py, "ValueError", &message)
}

fn percent_apply_numeric_width(
    py: &PyToken<'_>,
    prefix: &str,
    body: String,
    width: Option<usize>,
    left_adjust: bool,
    zero_pad: bool,
) -> Option<String> {
    let spec = FormatSpec {
        width,
        align: Some(if left_adjust {
            '<'
        } else if zero_pad {
            '='
        } else {
            '>'
        }),
        fill: u32::from(if zero_pad && !left_adjust { '0' } else { ' ' }),
        ..FormatSpec::default()
    };
    match crate::object::ops_format::apply_alignment(prefix, &body, &spec, '>') {
        Ok(bytes) => Some(String::from_utf8(bytes).expect("integer percent presentation is ASCII")),
        Err(error) => error.raise(py),
    }
}

fn percent_integer_precision(
    py: &PyToken<'_>,
    body: String,
    precision: Option<usize>,
) -> Option<String> {
    let spec = FormatSpec {
        width: precision,
        fill: u32::from('0'),
        ..FormatSpec::default()
    };
    match crate::object::ops_format::apply_alignment("", &body, &spec, '>') {
        Ok(bytes) => Some(String::from_utf8(bytes).expect("integer percent presentation is ASCII")),
        Err(error) => error.raise(py),
    }
}

fn percent_integer_precision_admit(py: &PyToken<'_>, precision: Option<usize>) -> Option<()> {
    if precision.is_some_and(|precision| precision > i32::MAX as usize - 3) {
        return raise_exception(py, "OverflowError", "precision too large");
    }
    Some(())
}

fn percent_raise_real_type_error_decimal(
    _py: &PyToken<'_>,
    obj: MoltObject,
    conv: u8,
) -> Option<BigInt> {
    let conv_ch = conv as char;
    let msg = format!(
        "%{conv_ch} format: a real number is required, not {}",
        type_name(_py, obj)
    );
    raise_exception::<Option<BigInt>>(_py, "TypeError", &msg)
}

fn percent_raise_integer_type_error(
    _py: &PyToken<'_>,
    obj: MoltObject,
    conv: u8,
) -> Option<BigInt> {
    let conv_ch = conv as char;
    let msg = format!(
        "%{conv_ch} format: an integer is required, not {}",
        type_name(_py, obj)
    );
    raise_exception::<Option<BigInt>>(_py, "TypeError", &msg)
}

fn percent_raise_char_type_error(_py: &PyToken<'_>, obj: MoltObject) -> Option<FormatOutput> {
    if crate::object::ops_sys::runtime_target_minor(_py) < 14 {
        return raise_exception(_py, "TypeError", "%c requires int or char");
    }
    let message = if let Some(ptr) = obj
        .as_ptr()
        .filter(|&ptr| unsafe { object_type_id(ptr) == TYPE_ID_STRING })
    {
        let bytes = unsafe { std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr)) };
        let length = crate::object::ops_string::wtf8_from_bytes(bytes)
            .code_points()
            .count();
        format!("%c requires an int or a unicode character, not a string of length {length}")
    } else {
        let name = crate::object::ops_format::format_diagnostic_type_name_bytes(_py, obj);
        if exception_pending(_py) {
            return None;
        }
        let mut message = Vec::new();
        for bytes in [
            b"%c requires an int or a unicode character, not ".as_slice(),
            name.as_slice(),
        ] {
            if let Err(error) = crate::object::ops_format::append_format_bytes(&mut message, bytes)
            {
                return error.raise(_py);
            }
        }
        return crate::object::ops_format::FormatError::Bytes("TypeError", message).raise(_py);
    };
    raise_exception(_py, "TypeError", &message)
}

fn percent_char_from_bigint(py: &PyToken<'_>, value: BigInt) -> Option<Vec<u8>> {
    match crate::object::ops_format::format_character_codepoint(&value) {
        Ok(bytes) => Some(bytes),
        Err(error) => error.raise(py),
    }
}

fn percent_decimal_float(py: &PyToken<'_>, value: f64) -> Option<BigInt> {
    if value.is_nan() {
        return raise_exception::<Option<BigInt>>(
            py,
            "ValueError",
            "cannot convert float NaN to integer",
        );
    }
    if value.is_infinite() {
        return raise_exception::<Option<BigInt>>(
            py,
            "OverflowError",
            "cannot convert float infinity to integer",
        );
    }
    Some(bigint_from_f64_trunc(value))
}

fn percent_decimal_from_obj(_py: &PyToken<'_>, value_bits: u64, conv: u8) -> Option<BigInt> {
    let obj = obj_from_bits(value_bits);
    if let Some(value) = crate::builtins::numbers::index_bigint_integral_bits(value_bits) {
        return Some(value);
    }
    if builtin_operand(_py, obj)
        && let Some(value) = as_float_extended(obj)
    {
        return percent_decimal_float(_py, value);
    }
    if let Some(ptr) = maybe_ptr_from_bits(value_bits) {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_COMPLEX
                || type_id == TYPE_ID_STRING
                || type_id == TYPE_ID_BYTES
                || type_id == TYPE_ID_BYTEARRAY
            {
                return percent_raise_real_type_error_decimal(_py, obj, conv);
            }
            let int_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.int_name, b"__int__");
            if let Some(call_bits) =
                lookup_special_method_bits(_py, MoltObject::from_ptr(ptr).bits(), int_name_bits)
            {
                let res_bits = call_callable0(_py, call_bits);
                molt_cpython_abi::api::errors::with_preserved_error(|| {
                    dec_ref_bits(_py, call_bits)
                });
                if exception_pending(_py) {
                    if obj_from_bits(res_bits).as_ptr().is_some() {
                        molt_cpython_abi::api::errors::with_preserved_error(|| {
                            dec_ref_bits(_py, res_bits)
                        });
                    }
                    return None;
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(value) = crate::builtins::numbers::index_bigint_integral_bits(res_bits)
                {
                    molt_cpython_abi::api::errors::with_preserved_error(|| {
                        dec_ref_bits(_py, res_bits)
                    });
                    return Some(value);
                }
                let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                if res_obj.as_ptr().is_some() {
                    molt_cpython_abi::api::errors::with_preserved_error(|| {
                        dec_ref_bits(_py, res_bits)
                    });
                }
                let msg = format!("__int__ returned non-int (type {res_type})");
                return raise_exception::<Option<BigInt>>(_py, "TypeError", &msg);
            }
            if exception_pending(_py) {
                return None;
            }
            // The float base payload applies only after an overriding __int__
            // has had its turn; floats do not fall through to __index__.
            if let Some(value) = as_float_extended(obj) {
                return percent_decimal_float(_py, value);
            }
            let index_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.index_name, b"__index__");
            if let Some(call_bits) =
                lookup_special_method_bits(_py, MoltObject::from_ptr(ptr).bits(), index_name_bits)
            {
                let res_bits = call_callable0(_py, call_bits);
                molt_cpython_abi::api::errors::with_preserved_error(|| {
                    dec_ref_bits(_py, call_bits)
                });
                if exception_pending(_py) {
                    if obj_from_bits(res_bits).as_ptr().is_some() {
                        molt_cpython_abi::api::errors::with_preserved_error(|| {
                            dec_ref_bits(_py, res_bits)
                        });
                    }
                    return None;
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(value) = crate::builtins::numbers::index_bigint_integral_bits(res_bits)
                {
                    molt_cpython_abi::api::errors::with_preserved_error(|| {
                        dec_ref_bits(_py, res_bits)
                    });
                    return Some(value);
                }
                let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                if res_obj.as_ptr().is_some() {
                    molt_cpython_abi::api::errors::with_preserved_error(|| {
                        dec_ref_bits(_py, res_bits)
                    });
                }
                let msg = format!("__index__ returned non-int (type {res_type})");
                return raise_exception::<Option<BigInt>>(_py, "TypeError", &msg);
            }
            if exception_pending(_py) {
                return None;
            }
        }
    }
    percent_raise_real_type_error_decimal(_py, obj, conv)
}

fn percent_integer_from_obj(_py: &PyToken<'_>, value_bits: u64, conv: u8) -> Option<BigInt> {
    let obj = obj_from_bits(value_bits);
    if let Some(value) = crate::builtins::numbers::index_bigint_integral_bits(value_bits) {
        return Some(value);
    }
    if let Some(ptr) = maybe_ptr_from_bits(value_bits) {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_COMPLEX
                || type_id == TYPE_ID_STRING
                || type_id == TYPE_ID_BYTES
                || type_id == TYPE_ID_BYTEARRAY
            {
                return percent_raise_integer_type_error(_py, obj, conv);
            }
            let index_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.index_name, b"__index__");
            if let Some(call_bits) =
                lookup_special_method_bits(_py, MoltObject::from_ptr(ptr).bits(), index_name_bits)
            {
                let res_bits = call_callable0(_py, call_bits);
                molt_cpython_abi::api::errors::with_preserved_error(|| {
                    dec_ref_bits(_py, call_bits)
                });
                if exception_pending(_py) {
                    if obj_from_bits(res_bits).as_ptr().is_some() {
                        molt_cpython_abi::api::errors::with_preserved_error(|| {
                            dec_ref_bits(_py, res_bits)
                        });
                    }
                    return None;
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(value) = crate::builtins::numbers::index_bigint_integral_bits(res_bits)
                {
                    molt_cpython_abi::api::errors::with_preserved_error(|| {
                        dec_ref_bits(_py, res_bits)
                    });
                    return Some(value);
                }
                let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                if res_obj.as_ptr().is_some() {
                    molt_cpython_abi::api::errors::with_preserved_error(|| {
                        dec_ref_bits(_py, res_bits)
                    });
                }
                let msg = format!("__index__ returned non-int (type {res_type})");
                return raise_exception::<Option<BigInt>>(_py, "TypeError", &msg);
            }
            if exception_pending(_py) {
                return None;
            }
        }
    }
    percent_raise_integer_type_error(_py, obj, conv)
}

fn percent_char_from_obj(_py: &PyToken<'_>, value_bits: u64) -> Option<FormatOutput> {
    let obj = obj_from_bits(value_bits);
    if let Some(ptr) = obj
        .as_ptr()
        .filter(|&ptr| unsafe { object_type_id(ptr) == TYPE_ID_STRING })
    {
        let text = unsafe { std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr)) };
        if wtf8_step(text, 0, false).is_some_and(|(end, _)| end == text.len()) {
            // %c writes a character, never adopts a str subclass receiver.
            let mut bytes = Vec::new();
            if let Err(error) = crate::object::ops_format::append_format_bytes(&mut bytes, text) {
                return error.raise(_py);
            }
            return Some(FormatOutput::Bytes(bytes));
        }
        return percent_raise_char_type_error(_py, obj);
    }
    if let Some(value) = crate::builtins::numbers::index_bigint_integral_bits(value_bits) {
        return percent_char_from_bigint(_py, value).map(Into::into);
    }
    if let Some(ptr) = maybe_ptr_from_bits(value_bits) {
        unsafe {
            let index_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.index_name, b"__index__");
            if let Some(call_bits) =
                lookup_special_method_bits(_py, MoltObject::from_ptr(ptr).bits(), index_name_bits)
            {
                let res_bits = call_callable0(_py, call_bits);
                molt_cpython_abi::api::errors::with_preserved_error(|| {
                    dec_ref_bits(_py, call_bits)
                });
                if exception_pending(_py) {
                    if obj_from_bits(res_bits).as_ptr().is_some() {
                        molt_cpython_abi::api::errors::with_preserved_error(|| {
                            dec_ref_bits(_py, res_bits)
                        });
                    }
                    return None;
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(value) = crate::builtins::numbers::index_bigint_integral_bits(res_bits)
                {
                    molt_cpython_abi::api::errors::with_preserved_error(|| {
                        dec_ref_bits(_py, res_bits)
                    });
                    return percent_char_from_bigint(_py, value).map(Into::into);
                }
                let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                if res_obj.as_ptr().is_some() {
                    molt_cpython_abi::api::errors::with_preserved_error(|| {
                        dec_ref_bits(_py, res_bits)
                    });
                }
                let msg = format!("__index__ returned non-int (type {res_type})");
                return raise_exception::<Option<FormatOutput>>(_py, "TypeError", &msg);
            }
            if exception_pending(_py) {
                return None;
            }
        }
    }
    percent_raise_char_type_error(_py, obj)
}

fn percent_numeric_prefix(is_negative: bool, flags: PercentFormatFlags) -> Option<char> {
    if is_negative {
        Some('-')
    } else if flags.sign_plus {
        Some('+')
    } else if flags.sign_space {
        Some(' ')
    } else {
        None
    }
}

fn percent_format_text(
    py: &PyToken<'_>,
    text: FormatOutput,
    width: Option<usize>,
    precision: Option<usize>,
    flags: PercentFormatFlags,
) -> Option<FormatOutput> {
    match crate::object::ops_format::format_text_output(
        py,
        text,
        width,
        precision,
        b" ",
        if flags.left_adjust { '<' } else { '>' },
        !flags.sign_plus && !flags.sign_space,
    ) {
        Ok(output) => Some(output),
        Err(error) => error.raise(py),
    }
}

fn percent_format_decimal(
    _py: &PyToken<'_>,
    value_bits: u64,
    width: Option<usize>,
    precision: Option<usize>,
    flags: PercentFormatFlags,
    conv: u8,
) -> Option<String> {
    let value = percent_decimal_from_obj(_py, value_bits, conv).or_else(|| {
        if percent_conversion_type_error(_py) {
            clear_exception(_py);
            percent_raise_real_type_error_decimal(_py, obj_from_bits(value_bits), conv)
        } else {
            None
        }
    })?;
    percent_integer_precision_admit(_py, precision)?;
    let negative = value.is_negative();
    let mut body = value.abs().to_string();
    body = percent_integer_precision(_py, body, precision)?;
    let mut prefix = String::new();
    if let Some(sign) = percent_numeric_prefix(negative, flags) {
        prefix.push(sign);
    }
    let zero_pad = flags.zero_pad && !flags.left_adjust;
    percent_apply_numeric_width(
        _py,
        prefix.as_str(),
        body,
        width,
        flags.left_adjust,
        zero_pad,
    )
}

fn percent_format_radix(
    _py: &PyToken<'_>,
    value_bits: u64,
    width: Option<usize>,
    precision: Option<usize>,
    flags: PercentFormatFlags,
    conv: u8,
) -> Option<String> {
    let value = percent_integer_from_obj(_py, value_bits, conv).or_else(|| {
        if percent_conversion_type_error(_py) {
            clear_exception(_py);
            percent_raise_integer_type_error(_py, obj_from_bits(value_bits), conv)
        } else {
            None
        }
    })?;
    percent_integer_precision_admit(_py, precision)?;
    let negative = value.is_negative();
    let mut body = match conv {
        b'o' => value.abs().to_str_radix(8),
        b'x' | b'X' => value.abs().to_str_radix(16),
        _ => value.abs().to_string(),
    };
    if conv == b'X' {
        body = body.to_uppercase();
    }
    body = percent_integer_precision(_py, body, precision)?;
    let mut prefix = String::new();
    if let Some(sign) = percent_numeric_prefix(negative, flags) {
        prefix.push(sign);
    }
    if flags.alternate {
        match conv {
            b'o' => prefix.push_str("0o"),
            b'x' => prefix.push_str("0x"),
            b'X' => prefix.push_str("0X"),
            _ => {}
        }
    }
    percent_apply_numeric_width(
        _py,
        prefix.as_str(),
        body,
        width,
        flags.left_adjust,
        flags.zero_pad && !flags.left_adjust,
    )
}

fn percent_format_float(
    _py: &PyToken<'_>,
    value_bits: u64,
    width: Option<usize>,
    precision: Option<usize>,
    flags: PercentFormatFlags,
    conv: u8,
) -> Option<Vec<u8>> {
    let value = crate::builtins::numbers::float_as_double(_py, value_bits)?;
    let sign = if flags.sign_plus {
        Some('+')
    } else if flags.sign_space {
        Some(' ')
    } else {
        None
    };
    let align = if flags.left_adjust {
        Some('<')
    } else if flags.zero_pad {
        Some('=')
    } else {
        None
    };
    let spec = FormatSpec {
        fill: u32::from(if flags.zero_pad && !flags.left_adjust {
            '0'
        } else {
            ' '
        }),
        align,
        zero_flag: flags.zero_pad && !flags.left_adjust,
        sign,
        coerce_negative_zero: false,
        alternate: flags.alternate,
        width,
        grouping: None,
        fractional_grouping: None,
        precision,
        ty: Some(u32::from(conv)),
    };
    match format_float_value_with_spec(value, &spec) {
        Ok(text) => Some(text),
        Err(error) => error.raise(_py),
    }
}

fn percent_format_ascii(
    _py: &PyToken<'_>,
    value_bits: u64,
    width: Option<usize>,
    precision: Option<usize>,
    flags: PercentFormatFlags,
) -> Option<FormatOutput> {
    let rendered_bits = molt_ascii_from_obj(value_bits);
    percent_format_text(
        _py,
        FormatOutput::OwnedString(rendered_bits),
        width,
        precision,
        flags,
    )
}

fn percent_format_char(
    py: &PyToken<'_>,
    value_bits: u64,
    width: Option<usize>,
    flags: PercentFormatFlags,
) -> Option<FormatOutput> {
    let text = percent_char_from_obj(py, value_bits).or_else(|| {
        if percent_conversion_type_error(py) {
            clear_exception(py);
            percent_raise_char_type_error(py, obj_from_bits(value_bits))
        } else {
            None
        }
    })?;
    percent_format_text(py, text, width, None, flags)
}

fn percent_conversion_type_error(py: &PyToken<'_>) -> bool {
    let target = crate::builtins::exceptions::exception_type_bits_from_name(py, "TypeError");
    crate::builtins::exceptions::pending_exception_matches_type(py, target)
}

fn percent_lookup_mapping_arg(_py: &PyToken<'_>, rhs_bits: u64, key: &[u8]) -> Option<(u64, bool)> {
    let rhs_obj = obj_from_bits(rhs_bits);
    let Some(rhs_ptr) = rhs_obj.as_ptr() else {
        return raise_exception::<Option<(u64, bool)>>(
            _py,
            "TypeError",
            "format requires a mapping",
        );
    };
    unsafe {
        let rhs_type = object_type_id(rhs_ptr);
        if rhs_type == TYPE_ID_TUPLE {
            return raise_exception::<Option<(u64, bool)>>(
                _py,
                "TypeError",
                "format requires a mapping",
            );
        }
        let key_ptr = alloc_string(_py, key);
        if key_ptr.is_null() {
            return None;
        }
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        if rhs_type == TYPE_ID_DICT {
            if let Some(bits) = dict_get_in_place(_py, rhs_ptr, key_bits) {
                dec_ref_bits(_py, key_bits);
                return Some((bits, false));
            }
            if exception_pending(_py) {
                dec_ref_bits(_py, key_bits);
                return None;
            }
            raise_key_error_with_key::<()>(_py, key_bits);
            dec_ref_bits(_py, key_bits);
            return None;
        }
        let bits = molt_index(rhs_bits, key_bits);
        dec_ref_bits(_py, key_bits);
        if exception_pending(_py) {
            return None;
        }
        Some((bits, true))
    }
}

/// Mapping fields replace the active argument cursor before option parsing.
/// Keep their returned reference through the next field, including error exits.
struct PercentArguments {
    source: u64,
    current: u64,
    tuple: Option<*mut u8>,
    index: usize,
    consumed: bool,
    mapping: bool,
    mapped_owner: Option<PtrDropGuard>,
}

impl PercentArguments {
    fn next(&mut self, py: &PyToken<'_>) -> Option<u64> {
        if let Some(ptr) = self.tuple {
            let bits = unsafe {
                crate::object::seq_access::with_immutable_tuple_slice(ptr, |elems| {
                    elems.get(self.index).copied()
                })
                .flatten()
            };
            let Some(bits) = bits else {
                return raise_exception::<Option<u64>>(
                    py,
                    "TypeError",
                    "not enough arguments for format string",
                );
            };
            self.index += 1;
            return Some(bits);
        }
        if self.consumed {
            return raise_exception::<Option<u64>>(
                py,
                "TypeError",
                "not enough arguments for format string",
            );
        }
        self.consumed = true;
        Some(self.current)
    }

    fn mapped(&mut self, py: &PyToken<'_>, key: &[u8]) -> Option<()> {
        self.mapped_owner = None;
        self.current = self.source;
        let (bits, owned) = percent_lookup_mapping_arg(py, self.source, key)?;
        self.mapped_owner = obj_from_bits(bits).as_ptr().map(|ptr| {
            if !owned {
                inc_ref_bits(py, bits);
            }
            PtrDropGuard::preserving(ptr)
        });
        self.current = bits;
        self.tuple = None;
        self.consumed = false;
        Some(())
    }
}

fn percent_append(py: &PyToken<'_>, out: &mut FormatWriter<'_, '_>, bytes: &[u8]) -> Option<()> {
    match out.append_bytes(bytes) {
        Ok(()) => Some(()),
        Err(error) => error.raise(py),
    }
}

pub(super) fn string_percent_format_impl(
    _py: &PyToken<'_>,
    text: &[u8],
    receiver_bits: u64,
    rhs_bits: u64,
) -> Option<FormatOutput> {
    let rhs_obj = obj_from_bits(rhs_bits);
    let tuple = rhs_obj
        .as_ptr()
        .filter(|ptr| unsafe { object_type_id(*ptr) == TYPE_ID_TUPLE });
    let mapping = percent_rhs_allows_unused_non_tuple(_py, rhs_obj);
    if exception_pending(_py) {
        return None;
    }
    let mut arguments = PercentArguments {
        source: rhs_bits,
        current: rhs_bits,
        tuple,
        index: 0,
        consumed: false,
        mapping,
        mapped_owner: None,
    };
    let bytes = text;
    let mut out = FormatWriter::new(_py);
    let mut literal_start = 0usize;
    let mut idx = 0usize;
    while idx < bytes.len() {
        if bytes[idx] != b'%' {
            idx += 1;
            continue;
        }
        percent_append(_py, &mut out, &text[literal_start..idx])?;
        idx += 1;
        if idx >= bytes.len() {
            return raise_exception::<Option<FormatOutput>>(_py, "ValueError", "incomplete format");
        }
        if bytes[idx] == b'%' {
            percent_append(_py, &mut out, b"%")?;
            idx += 1;
            literal_start = idx;
            continue;
        }
        if bytes[idx] == b'(' {
            if !arguments.mapping {
                return raise_exception(_py, "TypeError", "format requires a mapping");
            }
            let key_start = idx + 1;
            let mut key_end = key_start;
            let mut nesting = 1usize;
            while key_end < bytes.len() {
                match bytes[key_end] {
                    b'(' => nesting += 1,
                    b')' => nesting -= 1,
                    _ => {}
                }
                if nesting == 0 {
                    break;
                }
                key_end += 1;
            }
            if key_end >= bytes.len() {
                return raise_exception::<Option<FormatOutput>>(
                    _py,
                    "ValueError",
                    "incomplete format key",
                );
            }
            arguments.mapped(_py, &text[key_start..key_end])?;
            idx = key_end + 1;
        }
        let mut flags = PercentFormatFlags::default();
        loop {
            if idx >= bytes.len() {
                return raise_exception::<Option<FormatOutput>>(
                    _py,
                    "ValueError",
                    "incomplete format",
                );
            }
            match bytes[idx] {
                b'-' => flags.left_adjust = true,
                b'+' => flags.sign_plus = true,
                b' ' => flags.sign_space = true,
                b'0' => flags.zero_pad = true,
                b'#' => flags.alternate = true,
                _ => break,
            }
            idx += 1;
        }
        let mut width = if idx < bytes.len() && bytes[idx].is_ascii_digit() {
            percent_parse_usize(_py, bytes, &mut idx, PercentField::Width)
        } else {
            None
        };
        if exception_pending(_py) {
            return None;
        }
        if width.is_none() && idx < bytes.len() && bytes[idx] == b'*' {
            idx += 1;
            let width_bits = arguments.next(_py)?;
            let width_val = percent_star_integer(_py, width_bits, PercentField::Width)?;
            if exception_pending(_py) {
                return None;
            }
            if width_val < 0 {
                flags.left_adjust = true;
                // CPython's signed field cannot represent -PY_SSIZE_T_MIN;
                // that sentinel leaves the natural width rather than padding.
                width = width_val
                    .checked_neg()
                    .and_then(|value| isize::try_from(value).ok())
                    .map(|value| value as usize);
            } else {
                let Ok(width_usize) = usize::try_from(width_val) else {
                    return raise_exception::<Option<FormatOutput>>(
                        _py,
                        "OverflowError",
                        "width too big",
                    );
                };
                width = Some(width_usize);
            }
        }
        let mut precision: Option<usize> = None;
        if idx < bytes.len() && bytes[idx] == b'.' {
            idx += 1;
            if idx < bytes.len() && bytes[idx] == b'*' {
                idx += 1;
                let prec_bits = arguments.next(_py)?;
                let prec_val = percent_star_integer(_py, prec_bits, PercentField::Precision)?;
                if exception_pending(_py) {
                    return None;
                }
                if prec_val <= 0 {
                    precision = Some(0);
                } else {
                    let Ok(prec_usize) = usize::try_from(prec_val) else {
                        return raise_exception::<Option<FormatOutput>>(
                            _py,
                            "OverflowError",
                            "precision too big",
                        );
                    };
                    precision = Some(prec_usize);
                }
            } else {
                precision = Some(
                    percent_parse_usize(_py, bytes, &mut idx, PercentField::Precision).unwrap_or(0),
                );
            }
        }
        if exception_pending(_py) {
            return None;
        }
        if idx < bytes.len() && (bytes[idx] == b'h' || bytes[idx] == b'l' || bytes[idx] == b'L') {
            idx += 1;
        }
        if idx >= bytes.len() {
            return raise_exception::<Option<FormatOutput>>(_py, "ValueError", "incomplete format");
        }
        let conv_idx = idx;
        let conv = bytes[idx];
        let (next_idx, codepoint) =
            wtf8_step(bytes, idx, false).expect("Python format conversion codepoint");
        idx = next_idx;
        let value_bits = arguments.next(_py)?;
        let rendered = match conv {
            b's' => percent_format_text(
                _py,
                format_obj_str_output(_py, obj_from_bits(value_bits)),
                width,
                precision,
                flags,
            ),
            b'r' => percent_format_text(
                _py,
                format_obj_output(_py, obj_from_bits(value_bits)),
                width,
                precision,
                flags,
            ),
            b'a' => percent_format_ascii(_py, value_bits, width, precision, flags),
            b'c' => percent_format_char(_py, value_bits, width, flags),
            b'd' | b'i' | b'u' => {
                percent_format_decimal(_py, value_bits, width, precision, flags, conv)
                    .map(Into::into)
            }
            b'o' | b'x' | b'X' => {
                percent_format_radix(_py, value_bits, width, precision, flags, conv).map(Into::into)
            }
            b'f' | b'F' | b'e' | b'E' | b'g' | b'G' => {
                percent_format_float(_py, value_bits, width, precision, flags, conv).map(Into::into)
            }
            _ => percent_unsupported_char(_py, bytes, conv_idx, codepoint).map(Into::into),
        };
        if exception_pending(_py) {
            return None;
        }
        let rendered = rendered?;
        if let Err(error) = out.append_output(rendered, idx == bytes.len()) {
            return error.raise(_py);
        }
        literal_start = idx;
    }
    if let Err(error) = out.append_literal(&text[literal_start..], Some(receiver_bits), true) {
        return error.raise(_py);
    }
    if let Some(ptr) = arguments.tuple {
        let len = unsafe {
            crate::object::seq_access::with_immutable_tuple_slice(ptr, |elems| elems.len())
                .unwrap_or(0)
        };
        if arguments.index < len {
            return raise_exception::<Option<FormatOutput>>(
                _py,
                "TypeError",
                "not all arguments converted during string formatting",
            );
        }
    } else if !arguments.consumed && !arguments.mapping {
        return raise_exception::<Option<FormatOutput>>(
            _py,
            "TypeError",
            "not all arguments converted during string formatting",
        );
    }
    Some(out.finish())
}
