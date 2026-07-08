use super::*;

#[derive(Copy, Clone)]
enum FormatContext {
    FormatString,
    FormatSpec,
}

struct FormatState {
    next_auto: usize,
    used_auto: bool,
    used_manual: bool,
    allow_positional: bool,
    mapping_mode: bool,
}

struct FormatField<'a> {
    field_name: &'a str,
    conversion: Option<char>,
    format_spec: &'a str,
}

fn format_raise_value_error_str(_py: &PyToken<'_>, msg: &str) -> Option<String> {
    raise_exception::<_>(_py, "ValueError", msg)
}

fn format_raise_value_error_bits(_py: &PyToken<'_>, msg: &str) -> Option<u64> {
    raise_exception::<_>(_py, "ValueError", msg)
}

fn format_raise_index_error_bits(_py: &PyToken<'_>, msg: &str) -> Option<u64> {
    raise_exception::<_>(_py, "IndexError", msg)
}

fn parse_format_field<'a>(
    _py: &PyToken<'_>,
    text: &'a str,
    start: usize,
    context: FormatContext,
) -> Option<(FormatField<'a>, usize)> {
    let bytes = text.as_bytes();
    let len = bytes.len();
    if start >= len {
        let msg = match context {
            FormatContext::FormatSpec => "unmatched '{' in format spec",
            FormatContext::FormatString => "Single '{' encountered in format string",
        };
        return raise_exception::<_>(_py, "ValueError", msg);
    }
    let mut idx = start;
    while idx < len {
        let b = bytes[idx];
        if b == b'!' || b == b':' || b == b'}' {
            break;
        }
        idx += 1;
    }
    let field_name = &text[start..idx];
    let mut conversion = None;
    if idx < len && bytes[idx] == b'!' {
        idx += 1;
        if idx >= len {
            let msg = match context {
                FormatContext::FormatSpec => "unmatched '{' in format spec",
                FormatContext::FormatString => "expected '}' before end of string",
            };
            return raise_exception::<_>(_py, "ValueError", msg);
        }
        let conv = bytes[idx] as char;
        if conv != 'r' && conv != 's' && conv != 'a' {
            if conv == '}' {
                return raise_exception::<_>(_py, "ValueError", "unmatched '{' in format spec");
            }
            let msg = format!("Unknown conversion specifier {conv}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        conversion = Some(conv);
        idx += 1;
    }
    let mut format_spec = "";
    if idx < len && bytes[idx] == b':' {
        idx += 1;
        let spec_start = idx;
        while idx < len {
            let b = bytes[idx];
            if b == b'{' {
                if idx + 1 < len && bytes[idx + 1] == b'{' {
                    idx += 2;
                    continue;
                }
                let (_, next_idx) =
                    parse_format_field(_py, text, idx + 1, FormatContext::FormatSpec)?;
                idx = next_idx;
                continue;
            }
            if b == b'}' {
                if idx + 1 < len && bytes[idx + 1] == b'}' {
                    idx += 2;
                    continue;
                }
                break;
            }
            idx += 1;
        }
        if idx >= len {
            let msg = match context {
                FormatContext::FormatSpec => "unmatched '{' in format spec",
                FormatContext::FormatString => "expected '}' before end of string",
            };
            return raise_exception::<_>(_py, "ValueError", msg);
        }
        format_spec = &text[spec_start..idx];
    }
    if idx >= len || bytes[idx] != b'}' {
        let msg = match context {
            FormatContext::FormatSpec => "unmatched '{' in format spec",
            FormatContext::FormatString => "expected '}' before end of string",
        };
        return raise_exception::<_>(_py, "ValueError", msg);
    }
    let next_idx = idx + 1;
    Some((
        FormatField {
            field_name,
            conversion,
            format_spec,
        },
        next_idx,
    ))
}

fn format_string_impl(
    _py: &PyToken<'_>,
    text: &str,
    args: &[u64],
    kwargs_bits: u64,
    state: &mut FormatState,
    context: FormatContext,
) -> Option<String> {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(text.len());
    let mut idx = 0usize;
    while idx < len {
        let b = bytes[idx];
        if b == b'{' {
            if idx + 1 < len && bytes[idx + 1] == b'{' {
                out.push('{');
                idx += 2;
                continue;
            }
            let (field, next_idx) = parse_format_field(_py, text, idx + 1, context)?;
            let rendered = format_field(_py, field, args, kwargs_bits, state)?;
            out.push_str(&rendered);
            idx = next_idx;
            continue;
        }
        if b == b'}' {
            if idx + 1 < len && bytes[idx + 1] == b'}' {
                out.push('}');
                idx += 2;
                continue;
            }
            return format_raise_value_error_str(_py, "Single '}' encountered in format string");
        }
        let start = idx;
        idx += 1;
        while idx < len && bytes[idx] != b'{' && bytes[idx] != b'}' {
            idx += 1;
        }
        out.push_str(&text[start..idx]);
    }
    Some(out)
}

fn resolve_format_field(
    _py: &PyToken<'_>,
    field_name: &str,
    args: &[u64],
    kwargs_bits: u64,
    state: &mut FormatState,
) -> Option<u64> {
    let bytes = field_name.as_bytes();
    let len = bytes.len();
    let mut idx = 0usize;
    while idx < len && bytes[idx] != b'.' && bytes[idx] != b'[' {
        idx += 1;
    }
    let base = &field_name[..idx];
    let base_bits = if base.is_empty() {
        if !state.allow_positional {
            return format_raise_value_error_bits(_py, "Format string contains positional fields");
        }
        if state.used_manual {
            return format_raise_value_error_bits(
                _py,
                "cannot switch from manual field specification to automatic field numbering",
            );
        }
        state.used_auto = true;
        let index = state.next_auto;
        state.next_auto += 1;
        if index >= args.len() {
            let msg = format!("Replacement index {index} out of range for positional args tuple");
            return format_raise_index_error_bits(_py, &msg);
        }
        args[index]
    } else if base.as_bytes().iter().all(|b| b.is_ascii_digit()) {
        if !state.allow_positional {
            return format_raise_value_error_bits(_py, "Format string contains positional fields");
        }
        if state.used_auto {
            return format_raise_value_error_bits(
                _py,
                "cannot switch from automatic field numbering to manual field specification",
            );
        }
        state.used_manual = true;
        let index = match base.parse::<usize>() {
            Ok(val) => val,
            Err(_) => {
                return format_raise_value_error_bits(
                    _py,
                    "Too many decimal digits in format string",
                );
            }
        };
        if index >= args.len() {
            let msg = format!("Replacement index {base} out of range for positional args tuple");
            return format_raise_index_error_bits(_py, &msg);
        }
        args[index]
    } else {
        if state.used_auto {
            return format_raise_value_error_bits(
                _py,
                "cannot switch from automatic field numbering to manual field specification",
            );
        }
        state.used_manual = true;
        let key_ptr = alloc_string(_py, base.as_bytes());
        if key_ptr.is_null() {
            return None;
        }
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        let val_bits = if state.mapping_mode {
            let looked_up = molt_index(kwargs_bits, key_bits);
            if exception_pending(_py) {
                dec_ref_bits(_py, key_bits);
                return None;
            }
            Some(looked_up)
        } else {
            let kwargs_obj = obj_from_bits(kwargs_bits);
            let mut looked_up = None;
            if let Some(dict_ptr) = kwargs_obj.as_ptr() {
                unsafe {
                    if object_type_id(dict_ptr) == TYPE_ID_DICT {
                        looked_up = dict_get_in_place(_py, dict_ptr, key_bits);
                    }
                }
            }
            if looked_up.is_none() {
                raise_key_error_with_key::<()>(_py, key_bits);
                dec_ref_bits(_py, key_bits);
                return None;
            }
            looked_up
        };
        dec_ref_bits(_py, key_bits);
        val_bits.unwrap()
    };
    let mut current_bits = base_bits;
    while idx < len {
        if bytes[idx] == b'.' {
            idx += 1;
            if idx >= len {
                return format_raise_value_error_bits(_py, "Empty attribute in format string");
            }
            let start = idx;
            while idx < len && bytes[idx] != b'.' && bytes[idx] != b'[' {
                idx += 1;
            }
            let attr = &field_name[start..idx];
            if attr.is_empty() {
                return format_raise_value_error_bits(_py, "Empty attribute in format string");
            }
            let attr_ptr = alloc_string(_py, attr.as_bytes());
            if attr_ptr.is_null() {
                return None;
            }
            let attr_bits = MoltObject::from_ptr(attr_ptr).bits();
            current_bits = molt_get_attr_name(current_bits, attr_bits);
            dec_ref_bits(_py, attr_bits);
            if exception_pending(_py) {
                return None;
            }
            continue;
        }
        if bytes[idx] == b'[' {
            idx += 1;
            if idx >= len {
                return format_raise_value_error_bits(_py, "expected '}' before end of string");
            }
            let start = idx;
            while idx < len && bytes[idx] != b']' {
                idx += 1;
            }
            if idx >= len {
                return format_raise_value_error_bits(_py, "expected '}' before end of string");
            }
            let key = &field_name[start..idx];
            if key.is_empty() {
                return format_raise_value_error_bits(_py, "Empty attribute in format string");
            }
            idx += 1;
            if idx < len && bytes[idx] != b'.' && bytes[idx] != b'[' {
                return format_raise_value_error_bits(
                    _py,
                    "Only '.' or '[' may follow ']' in format field specifier",
                );
            }
            let (key_bits, drop_key) = if key.as_bytes().iter().all(|b| b.is_ascii_digit()) {
                let val = match key.parse::<i64>() {
                    Ok(num) => num,
                    Err(_) => {
                        return format_raise_value_error_bits(
                            _py,
                            "Too many decimal digits in format string",
                        );
                    }
                };
                (MoltObject::from_int(val).bits(), false)
            } else {
                let key_ptr = alloc_string(_py, key.as_bytes());
                if key_ptr.is_null() {
                    return None;
                }
                (MoltObject::from_ptr(key_ptr).bits(), true)
            };
            current_bits = molt_index(current_bits, key_bits);
            if drop_key {
                dec_ref_bits(_py, key_bits);
            }
            if exception_pending(_py) {
                return None;
            }
            continue;
        }
        break;
    }
    Some(current_bits)
}

fn format_field(
    _py: &PyToken<'_>,
    field: FormatField,
    args: &[u64],
    kwargs_bits: u64,
    state: &mut FormatState,
) -> Option<String> {
    let mut value_bits = resolve_format_field(_py, field.field_name, args, kwargs_bits, state)?;
    if exception_pending(_py) {
        return None;
    }
    let mut drop_value = false;
    if let Some(conv) = field.conversion {
        value_bits = match conv {
            'r' => {
                drop_value = true;
                molt_repr_from_obj(value_bits)
            }
            's' => {
                drop_value = true;
                molt_str_from_obj(value_bits)
            }
            'a' => {
                drop_value = true;
                molt_ascii_from_obj(value_bits)
            }
            _ => value_bits,
        };
        if exception_pending(_py) {
            return None;
        }
    }
    let spec_text = if field.format_spec.is_empty() {
        String::new()
    } else {
        format_string_impl(
            _py,
            field.format_spec,
            args,
            kwargs_bits,
            state,
            FormatContext::FormatSpec,
        )?
    };
    let spec_ptr = alloc_string(_py, spec_text.as_bytes());
    if spec_ptr.is_null() {
        return None;
    }
    let spec_bits = MoltObject::from_ptr(spec_ptr).bits();
    let formatted_bits = molt_format_builtin(value_bits, spec_bits);
    dec_ref_bits(_py, spec_bits);
    if drop_value {
        dec_ref_bits(_py, value_bits);
    }
    if exception_pending(_py) {
        return None;
    }
    let formatted_obj = obj_from_bits(formatted_bits);
    let rendered =
        string_obj_to_owned(formatted_obj).unwrap_or_else(|| format_obj_str(_py, formatted_obj));
    dec_ref_bits(_py, formatted_bits);
    Some(rendered)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_format_method(
    self_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let self_obj = obj_from_bits(self_bits);
        let Some(self_ptr) = self_obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(self_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "format requires a string");
            }
            let text = string_obj_to_owned(self_obj).unwrap_or_default();
            let args_obj = obj_from_bits(args_bits);
            let Some(args_ptr) = args_obj.as_ptr() else {
                return raise_exception::<_>(_py, "TypeError", "format arguments must be a tuple");
            };
            if object_type_id(args_ptr) != TYPE_ID_TUPLE {
                return raise_exception::<_>(_py, "TypeError", "format arguments must be a tuple");
            }
            let args_vec = seq_vec_ref(args_ptr);
            let mut state = FormatState {
                next_auto: 0,
                used_auto: false,
                used_manual: false,
                allow_positional: true,
                mapping_mode: false,
            };
            let Some(rendered) = format_string_impl(
                _py,
                &text,
                args_vec.as_slice(),
                kwargs_bits,
                &mut state,
                FormatContext::FormatString,
            ) else {
                return MoltObject::none().bits();
            };
            let out_ptr = alloc_string(_py, rendered.as_bytes());
            if out_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_format_map(self_bits: u64, mapping_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let self_obj = obj_from_bits(self_bits);
        let Some(self_ptr) = self_obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(self_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "format_map requires a string");
            }
            let text = string_obj_to_owned(self_obj).unwrap_or_default();
            let args_vec: [u64; 0] = [];
            let mut state = FormatState {
                next_auto: 0,
                used_auto: false,
                used_manual: false,
                allow_positional: false,
                mapping_mode: true,
            };
            let Some(rendered) = format_string_impl(
                _py,
                &text,
                args_vec.as_slice(),
                mapping_bits,
                &mut state,
                FormatContext::FormatString,
            ) else {
                return MoltObject::none().bits();
            };
            let out_ptr = alloc_string(_py, rendered.as_bytes());
            if out_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_format(val_bits: u64, spec_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let spec_obj = obj_from_bits(spec_bits);
        let spec_ptr = match spec_obj.as_ptr() {
            Some(ptr) => ptr,
            None => return raise_exception::<_>(_py, "TypeError", "format spec must be a str"),
        };
        unsafe {
            if object_type_id(spec_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "format spec must be a str");
            }
            let spec_bytes =
                std::slice::from_raw_parts(string_bytes(spec_ptr), string_len(spec_ptr));
            let spec_text = match std::str::from_utf8(spec_bytes) {
                Ok(val) => val,
                Err(_) => {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "format spec must be valid UTF-8",
                    );
                }
            };
            let spec = match parse_format_spec(spec_text) {
                Ok(val) => val,
                Err(msg) => return raise_exception::<_>(_py, "ValueError", msg),
            };
            let obj = obj_from_bits(val_bits);
            let rendered = match format_with_spec(_py, obj, &spec) {
                Ok(val) => val,
                Err((kind, msg)) => return raise_exception::<_>(_py, kind, msg.as_ref()),
            };
            let out_ptr = alloc_string(_py, rendered.as_bytes());
            if out_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}
