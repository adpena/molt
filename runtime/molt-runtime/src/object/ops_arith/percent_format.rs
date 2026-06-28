// String percent-format runtime shared by `%` modulo dispatch.
// This owns the parser/conversion authority for legacy `%` formatting
// while ops_arith.rs keeps only arithmetic entrypoint dispatch.

use super::*;

#[derive(Clone, Copy, Default)]
struct PercentFormatFlags {
    left_adjust: bool,
    sign_plus: bool,
    sign_space: bool,
    zero_pad: bool,
    alternate: bool,
}

fn percent_object_has_getitem(_py: &PyToken<'_>, ptr: *mut u8) -> bool {
    let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__getitem__") else {
        return false;
    };
    let call_bits = unsafe { attr_lookup_ptr_allow_missing(_py, ptr, name_bits) };
    dec_ref_bits(_py, name_bits);
    if let Some(call_bits) = call_bits {
        dec_ref_bits(_py, call_bits);
        return true;
    }
    false
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
    percent_object_has_getitem(_py, ptr)
}

fn percent_parse_usize(
    _py: &PyToken<'_>,
    bytes: &[u8],
    idx: &mut usize,
    field_name: &str,
) -> Option<usize> {
    let start = *idx;
    let mut out: usize = 0;
    while *idx < bytes.len() && bytes[*idx].is_ascii_digit() {
        let digit = (bytes[*idx] - b'0') as usize;
        out = match out.checked_mul(10).and_then(|v| v.checked_add(digit)) {
            Some(v) => v,
            None => {
                let msg = format!("{field_name} too large in format string");
                return raise_exception::<Option<usize>>(_py, "ValueError", &msg);
            }
        };
        *idx += 1;
    }
    if *idx == start { None } else { Some(out) }
}

fn percent_unsupported_char(_py: &PyToken<'_>, ch: u8, idx: usize) -> Option<String> {
    let ch_display = ch as char;
    let msg = format!("unsupported format character '{ch_display}' (0x{ch:02x}) at index {idx}");
    raise_exception::<Option<String>>(_py, "ValueError", &msg)
}

fn percent_apply_width(
    text: String,
    width: Option<usize>,
    left_adjust: bool,
    pad_char: char,
) -> String {
    let Some(width) = width else {
        return text;
    };
    let text_len = text.chars().count();
    if text_len >= width {
        return text;
    }
    let pad_len = width - text_len;
    let padding = pad_char.to_string().repeat(pad_len);
    if left_adjust {
        format!("{text}{padding}")
    } else {
        format!("{padding}{text}")
    }
}

fn percent_apply_numeric_width(
    prefix: &str,
    body: String,
    width: Option<usize>,
    left_adjust: bool,
    zero_pad: bool,
) -> String {
    let prefix_len = prefix.chars().count();
    let body_len = body.chars().count();
    if zero_pad
        && !left_adjust
        && let Some(width) = width
        && width > prefix_len + body_len
    {
        let mut out = String::with_capacity(width);
        out.push_str(prefix);
        out.push_str(&"0".repeat(width - prefix_len - body_len));
        out.push_str(&body);
        return out;
    }
    let mut text = String::with_capacity(prefix.len() + body.len());
    text.push_str(prefix);
    text.push_str(&body);
    percent_apply_width(text, width, left_adjust, ' ')
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

fn percent_raise_real_type_error_f(_py: &PyToken<'_>, obj: MoltObject) -> Option<f64> {
    let msg = format!("must be real number, not {}", type_name(_py, obj));
    raise_exception::<Option<f64>>(_py, "TypeError", &msg)
}

fn percent_raise_char_type_error(_py: &PyToken<'_>, obj: MoltObject) -> Option<char> {
    let _ = obj;
    raise_exception::<Option<char>>(_py, "TypeError", "%c requires int or char")
}

fn percent_char_from_bigint(_py: &PyToken<'_>, value: BigInt) -> Option<char> {
    let max_code = BigInt::from(0x110000u32);
    if value.sign() == Sign::Minus || value >= max_code {
        return raise_exception::<Option<char>>(
            _py,
            "OverflowError",
            "%c arg not in range(0x110000)",
        );
    }
    let Some(code) = value.to_u32() else {
        return raise_exception::<Option<char>>(
            _py,
            "OverflowError",
            "%c arg not in range(0x110000)",
        );
    };
    let Some(ch) = char::from_u32(code) else {
        return raise_exception::<Option<char>>(
            _py,
            "OverflowError",
            "%c arg not in range(0x110000)",
        );
    };
    Some(ch)
}

fn percent_decimal_from_obj(_py: &PyToken<'_>, value_bits: u64, conv: u8) -> Option<BigInt> {
    let obj = obj_from_bits(value_bits);
    if let Some(i) = to_i64(obj) {
        return Some(BigInt::from(i));
    }
    if let Some(big_ptr) = bigint_ptr_from_bits(value_bits) {
        return Some(unsafe { bigint_ref(big_ptr) }.clone());
    }
    if let Some(f) = as_float_extended(obj) {
        if f.is_nan() {
            return raise_exception::<Option<BigInt>>(
                _py,
                "ValueError",
                "cannot convert float NaN to integer",
            );
        }
        if f.is_infinite() {
            return raise_exception::<Option<BigInt>>(
                _py,
                "OverflowError",
                "cannot convert float infinity to integer",
            );
        }
        return Some(bigint_from_f64_trunc(f));
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
            if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, int_name_bits) {
                let res_bits = call_callable0(_py, call_bits);
                dec_ref_bits(_py, call_bits);
                if exception_pending(_py) {
                    if obj_from_bits(res_bits).as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return None;
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(i) = to_i64(res_obj) {
                    if res_obj.as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return Some(BigInt::from(i));
                }
                if let Some(res_big_ptr) = bigint_ptr_from_bits(res_bits) {
                    let out = bigint_ref(res_big_ptr).clone();
                    dec_ref_bits(_py, res_bits);
                    return Some(out);
                }
                let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                if res_obj.as_ptr().is_some() {
                    dec_ref_bits(_py, res_bits);
                }
                let msg = format!("__int__ returned non-int (type {res_type})");
                return raise_exception::<Option<BigInt>>(_py, "TypeError", &msg);
            }
            if exception_pending(_py) {
                return None;
            }
            let index_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.index_name, b"__index__");
            if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, index_name_bits) {
                let res_bits = call_callable0(_py, call_bits);
                dec_ref_bits(_py, call_bits);
                if exception_pending(_py) {
                    if obj_from_bits(res_bits).as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return None;
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(i) = to_i64(res_obj) {
                    if res_obj.as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return Some(BigInt::from(i));
                }
                if let Some(res_big_ptr) = bigint_ptr_from_bits(res_bits) {
                    let out = bigint_ref(res_big_ptr).clone();
                    dec_ref_bits(_py, res_bits);
                    return Some(out);
                }
                let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                if res_obj.as_ptr().is_some() {
                    dec_ref_bits(_py, res_bits);
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
    if let Some(i) = to_i64(obj) {
        return Some(BigInt::from(i));
    }
    if let Some(big_ptr) = bigint_ptr_from_bits(value_bits) {
        return Some(unsafe { bigint_ref(big_ptr) }.clone());
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
            if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, index_name_bits) {
                let res_bits = call_callable0(_py, call_bits);
                dec_ref_bits(_py, call_bits);
                if exception_pending(_py) {
                    if obj_from_bits(res_bits).as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return None;
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(i) = to_i64(res_obj) {
                    if res_obj.as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return Some(BigInt::from(i));
                }
                if let Some(res_big_ptr) = bigint_ptr_from_bits(res_bits) {
                    let out = bigint_ref(res_big_ptr).clone();
                    dec_ref_bits(_py, res_bits);
                    return Some(out);
                }
                let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                if res_obj.as_ptr().is_some() {
                    dec_ref_bits(_py, res_bits);
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

fn percent_char_from_obj(_py: &PyToken<'_>, value_bits: u64) -> Option<char> {
    let obj = obj_from_bits(value_bits);
    if let Some(text) = string_obj_to_owned(obj) {
        let mut chars = text.chars();
        return match chars.next() {
            Some(ch) if chars.next().is_none() => Some(ch),
            _ => percent_raise_char_type_error(_py, obj),
        };
    }
    if let Some(i) = to_i64(obj) {
        return percent_char_from_bigint(_py, BigInt::from(i));
    }
    if let Some(big_ptr) = bigint_ptr_from_bits(value_bits) {
        return percent_char_from_bigint(_py, unsafe { bigint_ref(big_ptr) }.clone());
    }
    if let Some(ptr) = maybe_ptr_from_bits(value_bits) {
        unsafe {
            let index_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.index_name, b"__index__");
            if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, index_name_bits) {
                let res_bits = call_callable0(_py, call_bits);
                dec_ref_bits(_py, call_bits);
                if exception_pending(_py) {
                    if obj_from_bits(res_bits).as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return None;
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(i) = to_i64(res_obj) {
                    if res_obj.as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return percent_char_from_bigint(_py, BigInt::from(i));
                }
                if let Some(res_big_ptr) = bigint_ptr_from_bits(res_bits) {
                    let out = bigint_ref(res_big_ptr).clone();
                    dec_ref_bits(_py, res_bits);
                    return percent_char_from_bigint(_py, out);
                }
                let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                if res_obj.as_ptr().is_some() {
                    dec_ref_bits(_py, res_bits);
                }
                let msg = format!("__index__ returned non-int (type {res_type})");
                return raise_exception::<Option<char>>(_py, "TypeError", &msg);
            }
            if exception_pending(_py) {
                return None;
            }
        }
    }
    percent_raise_char_type_error(_py, obj)
}

fn percent_float_from_obj(_py: &PyToken<'_>, value_bits: u64) -> Option<f64> {
    let obj = obj_from_bits(value_bits);
    if let Some(f) = as_float_extended(obj) {
        return Some(f);
    }
    if let Some(i) = to_i64(obj) {
        return Some(i as f64);
    }
    if let Some(big_ptr) = bigint_ptr_from_bits(value_bits) {
        return match unsafe { bigint_ref(big_ptr) }.to_f64() {
            Some(v) => Some(v),
            None => raise_exception::<Option<f64>>(
                _py,
                "OverflowError",
                "int too large to convert to float",
            ),
        };
    }
    if let Some(ptr) = maybe_ptr_from_bits(value_bits) {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_COMPLEX
                || type_id == TYPE_ID_STRING
                || type_id == TYPE_ID_BYTES
                || type_id == TYPE_ID_BYTEARRAY
            {
                return percent_raise_real_type_error_f(_py, obj);
            }
            let float_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.float_name, b"__float__");
            if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, float_name_bits) {
                let res_bits = call_callable0(_py, call_bits);
                dec_ref_bits(_py, call_bits);
                if exception_pending(_py) {
                    if obj_from_bits(res_bits).as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return None;
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(f) = res_obj.as_float() {
                    if res_obj.as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return Some(f);
                }
                let owner = class_name_for_error(type_of_bits(_py, value_bits));
                let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                if res_obj.as_ptr().is_some() {
                    dec_ref_bits(_py, res_bits);
                }
                let msg = format!("{owner}.__float__ returned non-float (type {res_type})");
                return raise_exception::<Option<f64>>(_py, "TypeError", &msg);
            }
            if exception_pending(_py) {
                return None;
            }
            let index_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.index_name, b"__index__");
            if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, index_name_bits) {
                let res_bits = call_callable0(_py, call_bits);
                dec_ref_bits(_py, call_bits);
                if exception_pending(_py) {
                    if obj_from_bits(res_bits).as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return None;
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(i) = to_i64(res_obj) {
                    if res_obj.as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return Some(i as f64);
                }
                if let Some(res_big_ptr) = bigint_ptr_from_bits(res_bits) {
                    let out = bigint_ref(res_big_ptr).to_f64();
                    dec_ref_bits(_py, res_bits);
                    return match out {
                        Some(v) => Some(v),
                        None => raise_exception::<Option<f64>>(
                            _py,
                            "OverflowError",
                            "int too large to convert to float",
                        ),
                    };
                }
                let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                if res_obj.as_ptr().is_some() {
                    dec_ref_bits(_py, res_bits);
                }
                let msg = format!("__index__ returned non-int (type {res_type})");
                return raise_exception::<Option<f64>>(_py, "TypeError", &msg);
            }
            if exception_pending(_py) {
                return None;
            }
        }
    }
    percent_raise_real_type_error_f(_py, obj)
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
    text: String,
    width: Option<usize>,
    precision: Option<usize>,
    flags: PercentFormatFlags,
) -> String {
    let rendered = if let Some(precision) = precision {
        text.chars().take(precision).collect::<String>()
    } else {
        text
    };
    percent_apply_width(rendered, width, flags.left_adjust, ' ')
}

fn percent_format_decimal(
    _py: &PyToken<'_>,
    value_bits: u64,
    width: Option<usize>,
    precision: Option<usize>,
    flags: PercentFormatFlags,
    conv: u8,
) -> Option<String> {
    let value = percent_decimal_from_obj(_py, value_bits, conv)?;
    let negative = value.is_negative();
    let mut body = value.abs().to_string();
    if let Some(precision) = precision
        && body.len() < precision
    {
        body = format!("{}{}", "0".repeat(precision - body.len()), body);
    }
    let mut prefix = String::new();
    if let Some(sign) = percent_numeric_prefix(negative, flags) {
        prefix.push(sign);
    }
    let zero_pad = flags.zero_pad && !flags.left_adjust;
    Some(percent_apply_numeric_width(
        prefix.as_str(),
        body,
        width,
        flags.left_adjust,
        zero_pad,
    ))
}

fn percent_format_radix(
    _py: &PyToken<'_>,
    value_bits: u64,
    width: Option<usize>,
    precision: Option<usize>,
    flags: PercentFormatFlags,
    conv: u8,
) -> Option<String> {
    let value = percent_integer_from_obj(_py, value_bits, conv)?;
    let negative = value.is_negative();
    let mut body = match conv {
        b'o' => value.abs().to_str_radix(8),
        b'x' | b'X' => value.abs().to_str_radix(16),
        _ => value.abs().to_string(),
    };
    if conv == b'X' {
        body = body.to_uppercase();
    }
    if let Some(precision) = precision
        && body.len() < precision
    {
        body = format!("{}{}", "0".repeat(precision - body.len()), body);
    }
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
    Some(percent_apply_numeric_width(
        prefix.as_str(),
        body,
        width,
        flags.left_adjust,
        flags.zero_pad && !flags.left_adjust,
    ))
}

fn percent_format_float(
    _py: &PyToken<'_>,
    value_bits: u64,
    width: Option<usize>,
    precision: Option<usize>,
    flags: PercentFormatFlags,
    conv: u8,
) -> Option<String> {
    let value = percent_float_from_obj(_py, value_bits)?;
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
        fill: if flags.zero_pad && !flags.left_adjust {
            '0'
        } else {
            ' '
        },
        align,
        zero_flag: flags.zero_pad && !flags.left_adjust,
        sign,
        alternate: flags.alternate,
        width,
        grouping: None,
        precision,
        ty: Some(conv as char),
    };
    match format_float_with_spec(MoltObject::from_float(value), &spec) {
        Ok(text) => Some(text),
        Err((kind, msg)) => raise_exception::<Option<String>>(_py, kind, msg.as_ref()),
    }
}

fn percent_format_ascii(
    _py: &PyToken<'_>,
    value_bits: u64,
    width: Option<usize>,
    precision: Option<usize>,
    flags: PercentFormatFlags,
) -> Option<String> {
    let rendered_bits = molt_ascii_from_obj(value_bits);
    if exception_pending(_py) {
        if obj_from_bits(rendered_bits).as_ptr().is_some() {
            dec_ref_bits(_py, rendered_bits);
        }
        return None;
    }
    let rendered = string_obj_to_owned(obj_from_bits(rendered_bits));
    if obj_from_bits(rendered_bits).as_ptr().is_some() {
        dec_ref_bits(_py, rendered_bits);
    }
    let rendered = rendered.unwrap_or_default();
    Some(percent_format_text(rendered, width, precision, flags))
}

fn percent_format_char(
    _py: &PyToken<'_>,
    value_bits: u64,
    width: Option<usize>,
    flags: PercentFormatFlags,
) -> Option<String> {
    let ch = percent_char_from_obj(_py, value_bits)?;
    Some(percent_apply_width(
        ch.to_string(),
        width,
        flags.left_adjust,
        ' ',
    ))
}

fn percent_lookup_mapping_arg(_py: &PyToken<'_>, rhs_bits: u64, key: &str) -> Option<(u64, bool)> {
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
        let key_ptr = alloc_string(_py, key.as_bytes());
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
        if !percent_object_has_getitem(_py, rhs_ptr) {
            dec_ref_bits(_py, key_bits);
            return raise_exception::<Option<(u64, bool)>>(
                _py,
                "TypeError",
                "format requires a mapping",
            );
        }
        let bits = molt_index(rhs_bits, key_bits);
        dec_ref_bits(_py, key_bits);
        if exception_pending(_py) {
            return None;
        }
        Some((bits, true))
    }
}

fn percent_consume_next_arg(
    _py: &PyToken<'_>,
    rhs_bits: u64,
    tuple_ptr: Option<*mut u8>,
    tuple_idx: &mut usize,
    single_consumed: &mut bool,
) -> Option<u64> {
    if let Some(ptr) = tuple_ptr {
        let elems = unsafe { seq_vec_ref(ptr) };
        if *tuple_idx >= elems.len() {
            return raise_exception::<Option<u64>>(
                _py,
                "TypeError",
                "not enough arguments for format string",
            );
        }
        let bits = elems[*tuple_idx];
        *tuple_idx += 1;
        return Some(bits);
    }
    if *single_consumed {
        return raise_exception::<Option<u64>>(
            _py,
            "TypeError",
            "not enough arguments for format string",
        );
    }
    *single_consumed = true;
    Some(rhs_bits)
}

pub(super) fn string_percent_format_impl(
    _py: &PyToken<'_>,
    text: &str,
    rhs_bits: u64,
) -> Option<String> {
    let rhs_obj = obj_from_bits(rhs_bits);
    let tuple_ptr = rhs_obj
        .as_ptr()
        .filter(|ptr| unsafe { object_type_id(*ptr) == TYPE_ID_TUPLE });
    let mut tuple_idx = 0usize;
    let mut single_consumed = false;
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len() + 16);
    let mut literal_start = 0usize;
    let mut idx = 0usize;
    while idx < bytes.len() {
        if bytes[idx] != b'%' {
            idx += 1;
            continue;
        }
        out.push_str(&text[literal_start..idx]);
        idx += 1;
        if idx >= bytes.len() {
            return raise_exception::<Option<String>>(_py, "ValueError", "incomplete format");
        }
        if bytes[idx] == b'%' {
            out.push('%');
            idx += 1;
            literal_start = idx;
            continue;
        }
        let mut key: Option<&str> = None;
        if bytes[idx] == b'(' {
            let key_start = idx + 1;
            let mut key_end = key_start;
            while key_end < bytes.len() && bytes[key_end] != b')' {
                key_end += 1;
            }
            if key_end >= bytes.len() {
                return raise_exception::<Option<String>>(
                    _py,
                    "ValueError",
                    "incomplete format key",
                );
            }
            key = Some(&text[key_start..key_end]);
            idx = key_end + 1;
        }
        let mut flags = PercentFormatFlags::default();
        loop {
            if idx >= bytes.len() {
                return raise_exception::<Option<String>>(_py, "ValueError", "incomplete format");
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
            percent_parse_usize(_py, bytes, &mut idx, "width")
        } else {
            None
        };
        if idx < bytes.len() && bytes[idx] == b'*' {
            idx += 1;
            let width_bits = percent_consume_next_arg(
                _py,
                rhs_bits,
                tuple_ptr,
                &mut tuple_idx,
                &mut single_consumed,
            )?;
            let width_val = index_i64_from_obj(_py, width_bits, "* wants int");
            if exception_pending(_py) {
                return None;
            }
            if width_val < 0 {
                flags.left_adjust = true;
                let abs = width_val.checked_abs().unwrap_or(i64::MAX);
                let Ok(width_usize) = usize::try_from(abs) else {
                    return raise_exception::<Option<String>>(
                        _py,
                        "OverflowError",
                        "width too big",
                    );
                };
                width = Some(width_usize);
            } else {
                let Ok(width_usize) = usize::try_from(width_val) else {
                    return raise_exception::<Option<String>>(
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
                let prec_bits = percent_consume_next_arg(
                    _py,
                    rhs_bits,
                    tuple_ptr,
                    &mut tuple_idx,
                    &mut single_consumed,
                )?;
                let prec_val = index_i64_from_obj(_py, prec_bits, "* wants int");
                if exception_pending(_py) {
                    return None;
                }
                if prec_val <= 0 {
                    precision = Some(0);
                } else {
                    let Ok(prec_usize) = usize::try_from(prec_val) else {
                        return raise_exception::<Option<String>>(
                            _py,
                            "OverflowError",
                            "precision too big",
                        );
                    };
                    precision = Some(prec_usize);
                }
            } else {
                precision =
                    Some(percent_parse_usize(_py, bytes, &mut idx, "precision").unwrap_or(0));
            }
        }
        if idx < bytes.len() && (bytes[idx] == b'h' || bytes[idx] == b'l' || bytes[idx] == b'L') {
            let first = bytes[idx];
            idx += 1;
            if idx < bytes.len() && (first == b'h' || first == b'l') && bytes[idx] == first {
                idx += 1;
            }
        }
        if idx >= bytes.len() {
            return raise_exception::<Option<String>>(_py, "ValueError", "incomplete format");
        }
        let conv_idx = idx;
        let conv = bytes[idx];
        idx += 1;
        let (value_bits, drop_value) = if let Some(key) = key {
            percent_lookup_mapping_arg(_py, rhs_bits, key)?
        } else {
            (
                percent_consume_next_arg(
                    _py,
                    rhs_bits,
                    tuple_ptr,
                    &mut tuple_idx,
                    &mut single_consumed,
                )?,
                false,
            )
        };
        let rendered = match conv {
            b's' => Some(percent_format_text(
                format_obj_str(_py, obj_from_bits(value_bits)),
                width,
                precision,
                flags,
            )),
            b'r' => Some(percent_format_text(
                format_obj(_py, obj_from_bits(value_bits)),
                width,
                precision,
                flags,
            )),
            b'a' => percent_format_ascii(_py, value_bits, width, precision, flags),
            b'c' => percent_format_char(_py, value_bits, width, flags),
            b'd' | b'i' | b'u' => {
                percent_format_decimal(_py, value_bits, width, precision, flags, conv)
            }
            b'o' | b'x' | b'X' => {
                percent_format_radix(_py, value_bits, width, precision, flags, conv)
            }
            b'f' | b'F' | b'e' | b'E' | b'g' | b'G' => {
                percent_format_float(_py, value_bits, width, precision, flags, conv)
            }
            _ => percent_unsupported_char(_py, conv, conv_idx),
        };
        if drop_value {
            dec_ref_bits(_py, value_bits);
        }
        let rendered = rendered?;
        out.push_str(&rendered);
        literal_start = idx;
    }
    out.push_str(&text[literal_start..]);
    if let Some(ptr) = tuple_ptr {
        let elems = unsafe { seq_vec_ref(ptr) };
        if tuple_idx < elems.len() {
            return raise_exception::<Option<String>>(
                _py,
                "TypeError",
                "not all arguments converted during string formatting",
            );
        }
    } else if !single_consumed && !percent_rhs_allows_unused_non_tuple(_py, rhs_obj) {
        return raise_exception::<Option<String>>(
            _py,
            "TypeError",
            "not all arguments converted during string formatting",
        );
    }
    Some(out)
}
