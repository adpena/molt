use super::*;
use crate::object::ops_format::{
    FormatOutput, FormatParseError, FormatWriter, parse_format_integer,
};

/// The field grammar is shared by str.format and the lazy _string projections.
/// Offsets and borrowed spans refer to lossless Python WTF-8 storage; stepping
/// consumes a full code point, including lone surrogates.
pub(crate) struct FormatField<'a> {
    pub(crate) field_name: &'a [u8],
    pub(crate) conversion: u32,
    pub(crate) conversion_text: &'a [u8],
    pub(crate) format_spec: &'a [u8],
    pub(crate) needs_expanding: bool,
}

pub(crate) struct FormatMarkup<'a> {
    pub(crate) literal: &'a [u8],
    pub(crate) field: Option<FormatField<'a>>,
}

pub(crate) struct FormatName<'a> {
    pub(crate) text: &'a [u8],
    pub(crate) index: Option<usize>,
}

pub(crate) struct FormatLookup<'a> {
    pub(crate) is_attribute: bool,
    pub(crate) name: FormatName<'a>,
}

fn next_format_code(text: &[u8], offset: &mut usize) -> Option<u32> {
    let (next, code) = crate::object::ops_string::wtf8_step(text, *offset, false)?;
    *offset = next;
    Some(code)
}

fn parse_format_field<'a>(
    text: &'a [u8],
    offset: &mut usize,
) -> Result<FormatField<'a>, &'static str> {
    let start = *offset;
    let (end, delimiter) = loop {
        let end = *offset;
        match next_format_code(text, offset) {
            Some(123) => return Err("unexpected '{' in field name"),
            Some(91) => {
                // A lookup key ends at the first ']', and may contain braces,
                // conversion markers, colons, and further '[' characters.
                while *offset < text.len() && text[*offset] != b']' {
                    next_format_code(text, offset);
                }
            }
            Some(code @ (125 | 58 | 33)) => break (end, code),
            Some(_) => {}
            None => return Err("expected '}' before end of string"),
        }
    };
    let mut field = FormatField {
        field_name: &text[start..end],
        conversion: 0,
        conversion_text: b"",
        format_spec: b"",
        needs_expanding: false,
    };
    if delimiter == 125 {
        return Ok(field);
    }
    if delimiter == 33 {
        let conversion_start = *offset;
        field.conversion = next_format_code(text, offset)
            .ok_or("end of string while looking for conversion specifier")?;
        field.conversion_text = &text[conversion_start..*offset];
        match next_format_code(text, offset) {
            Some(125) => return Ok(field),
            Some(58) | None => {}
            Some(_) => return Err("expected ':' after conversion specifier"),
        }
    }
    let spec_start = *offset;
    let mut depth = 1usize;
    while *offset < text.len() {
        let end = *offset;
        match next_format_code(text, offset) {
            Some(123) => {
                field.needs_expanding = true;
                depth += 1;
            }
            Some(125) => {
                depth -= 1;
                if depth == 0 {
                    field.format_spec = &text[spec_start..end];
                    return Ok(field);
                }
            }
            _ => {}
        }
    }
    Err("unmatched '{' in format spec")
}

/// Consume one literal/field pair. Escaped braces finish a literal chunk,
/// exactly as the public Formatter parser does, without allocating a copy.
pub(crate) fn next_format_markup<'a>(
    text: &'a [u8],
    offset: &mut usize,
) -> Result<Option<FormatMarkup<'a>>, &'static str> {
    if *offset >= text.len() {
        return Ok(None);
    }
    let start = *offset;
    while *offset < text.len() {
        let end = *offset;
        let code = next_format_code(text, offset).expect("Python string code point");
        if code != 123 && code != 125 {
            continue;
        }
        if *offset < text.len() && text[*offset] == code as u8 {
            let literal_end = *offset;
            *offset += 1;
            return Ok(Some(FormatMarkup {
                literal: &text[start..literal_end],
                field: None,
            }));
        }
        if code == 125 {
            return Err("Single '}' encountered in format string");
        }
        if *offset == text.len() {
            return Err("Single '{' encountered in format string");
        }
        return Ok(Some(FormatMarkup {
            literal: &text[start..end],
            field: Some(parse_format_field(text, offset)?),
        }));
    }
    Ok(Some(FormatMarkup {
        literal: &text[start..*offset],
        field: None,
    }))
}

fn format_name_index(text: &[u8]) -> Result<Option<usize>, &'static str> {
    let mut codes = wtf8_from_bytes(text)
        .code_points()
        .map(|code| code.to_u32())
        .peekable();
    // Consume the decimal prefix before deciding whether this is a name:
    // overflow is an error even when a nondecimal suffix follows it.
    let index = parse_format_integer(&mut codes).map_err(|error| match error {
        FormatParseError::Diagnostic(message) => message,
        FormatParseError::InvalidSpecifier => unreachable!("decimal parser diagnostic"),
    })?;
    Ok(if codes.next().is_none() { index } else { None })
}

pub(crate) fn split_format_field_name(
    text: &[u8],
) -> Result<(FormatName<'_>, usize), &'static str> {
    let mut offset = 0;
    while offset < text.len() && !matches!(text[offset], b'.' | b'[') {
        next_format_code(text, &mut offset);
    }
    let first = &text[..offset];
    Ok((
        FormatName {
            text: first,
            index: format_name_index(first)?,
        },
        offset,
    ))
}

pub(crate) fn next_format_lookup<'a>(
    text: &'a [u8],
    offset: &mut usize,
) -> Result<Option<FormatLookup<'a>>, &'static str> {
    let Some(code) = next_format_code(text, offset) else {
        return Ok(None);
    };
    let start = *offset;
    let is_attribute = match code {
        46 => true,
        91 => false,
        _ => return Err("Only '.' or '[' may follow ']' in format field specifier"),
    };
    let end;
    if is_attribute {
        while *offset < text.len() && !matches!(text[*offset], b'.' | b'[') {
            next_format_code(text, offset);
        }
        end = *offset;
    } else {
        while *offset < text.len() && text[*offset] != b']' {
            next_format_code(text, offset);
        }
        if *offset == text.len() {
            return Err("Missing ']' in format string");
        }
        end = *offset;
        *offset += 1;
    }
    let name = &text[start..end];
    let index = if is_attribute {
        None
    } else {
        format_name_index(name)?
    };
    if name.is_empty() {
        return Err("Empty attribute in format string");
    }
    Ok(Some(FormatLookup {
        is_attribute,
        name: FormatName { text: name, index },
    }))
}

struct FormatState {
    next_auto: usize,
    used_auto: bool,
    used_manual: bool,
    allow_positional: bool,
    mapping_mode: bool,
}

fn format_string_impl(
    py: &PyToken<'_>,
    text: &[u8],
    source_bits: Option<u64>,
    args: &[u64],
    kwargs_bits: u64,
    state: &mut FormatState,
    recursion_depth: usize,
) -> Option<FormatOutput> {
    if recursion_depth == 0 {
        return raise_exception(py, "ValueError", "Max string recursion exceeded");
    }
    let mut out = FormatWriter::new(py);
    let mut offset = 0;
    loop {
        let markup = match next_format_markup(text, &mut offset) {
            Ok(Some(markup)) => markup,
            Ok(None) => return Some(out.finish()),
            Err(message) => return raise_exception(py, "ValueError", message),
        };
        if let Err(error) = out.append_literal(
            markup.literal,
            source_bits,
            offset == text.len() && markup.field.is_none(),
        ) {
            return error.raise(py);
        }
        if let Some(field) = markup.field {
            format_field(
                py,
                field,
                FormatArguments {
                    values: args,
                    keywords: kwargs_bits,
                },
                state,
                recursion_depth,
                &mut out,
                offset == text.len(),
            )?;
        }
    }
}

fn resolve_format_field(
    py: &PyToken<'_>,
    field_name: &[u8],
    args: &[u64],
    kwargs_bits: u64,
    state: &mut FormatState,
) -> Option<u64> {
    let (first, mut offset) = match split_format_field_name(field_name) {
        Ok(first) => first,
        Err(message) => return raise_exception(py, "ValueError", message),
    };
    let positional = if first.text.is_empty() {
        if state.used_manual {
            return raise_exception(
                py,
                "ValueError",
                "cannot switch from manual field specification to automatic field numbering",
            );
        }
        state.used_auto = true;
        let index = state.next_auto;
        state.next_auto += 1;
        Some(index)
    } else if let Some(index) = first.index {
        if state.used_auto {
            return raise_exception(
                py,
                "ValueError",
                "cannot switch from automatic field numbering to manual field specification",
            );
        }
        state.used_manual = true;
        Some(index)
    } else {
        None
    };
    let mut current_bits = if let Some(index) = positional {
        if !state.allow_positional {
            return raise_exception(py, "ValueError", "Format string contains positional fields");
        }
        let Some(&bits) = args.get(index) else {
            let message =
                format!("Replacement index {index} out of range for positional args tuple");
            return raise_exception(py, "IndexError", &message);
        };
        inc_ref_bits(py, bits);
        bits
    } else {
        let key_ptr = alloc_string(py, first.text);
        if key_ptr.is_null() {
            return None;
        }
        let _key_owner = PtrDropGuard::new(key_ptr);
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        if state.mapping_mode {
            let bits = molt_index(kwargs_bits, key_bits);
            if exception_pending(py) {
                molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, bits));
                return None;
            }
            bits
        } else {
            let value = obj_from_bits(kwargs_bits).as_ptr().and_then(|ptr| unsafe {
                if object_type_id(ptr) == TYPE_ID_DICT {
                    dict_get_in_place(py, ptr, key_bits)
                } else {
                    None
                }
            });
            if exception_pending(py) {
                return None;
            }
            let Some(value) = value else {
                return raise_key_error_with_key(py, key_bits);
            };
            inc_ref_bits(py, value);
            value
        }
    };
    let mut current_owner = PtrDropGuard::preserving(
        obj_from_bits(current_bits)
            .as_ptr()
            .unwrap_or(std::ptr::null_mut()),
    );
    loop {
        let lookup = match next_format_lookup(field_name, &mut offset) {
            Ok(Some(lookup)) => lookup,
            Ok(None) => break,
            Err(message) => return raise_exception(py, "ValueError", message),
        };
        let key_bits = if let Some(index) = lookup.name.index {
            int_bits_from_i64(py, index as i64)
        } else {
            let key_ptr = alloc_string(py, lookup.name.text);
            if key_ptr.is_null() {
                return None;
            }
            MoltObject::from_ptr(key_ptr).bits()
        };
        let _key_owner = PtrDropGuard::new(
            obj_from_bits(key_bits)
                .as_ptr()
                .unwrap_or(std::ptr::null_mut()),
        );
        if exception_pending(py) {
            return None;
        }
        current_bits = if lookup.is_attribute {
            molt_get_attr_name(current_bits, key_bits)
        } else {
            molt_index(current_bits, key_bits)
        };
        current_owner = PtrDropGuard::preserving(
            obj_from_bits(current_bits)
                .as_ptr()
                .unwrap_or(std::ptr::null_mut()),
        );
        if exception_pending(py) {
            return None;
        }
    }
    current_owner.release();
    Some(current_bits)
}

struct FormatArguments<'a> {
    values: &'a [u64],
    keywords: u64,
}

fn format_field(
    py: &PyToken<'_>,
    field: FormatField<'_>,
    arguments: FormatArguments<'_>,
    state: &mut FormatState,
    recursion_depth: usize,
    out: &mut FormatWriter<'_, '_>,
    final_piece: bool,
) -> Option<()> {
    let FormatArguments {
        values: args,
        keywords: kwargs_bits,
    } = arguments;
    let mut value_bits = resolve_format_field(py, field.field_name, args, kwargs_bits, state)?;
    let mut value_owner = PtrDropGuard::preserving(
        obj_from_bits(value_bits)
            .as_ptr()
            .unwrap_or(std::ptr::null_mut()),
    );
    // Lookup errors and its callbacks precede conversion validation. NUL is
    // the native parser's absent-conversion sentinel, including an explicit !\0.
    if field.conversion != 0 {
        value_bits = match field.conversion {
            114 => molt_repr_from_obj(value_bits),
            115 => molt_str_from_obj(value_bits),
            97 => molt_ascii_from_obj(value_bits),
            code => {
                let message = if (33..127).contains(&code) {
                    format!("Unknown conversion specifier {}", code as u8 as char)
                } else {
                    format!("Unknown conversion specifier \\x{code:x}")
                };
                return raise_exception(py, "ValueError", &message);
            }
        };
        value_owner = PtrDropGuard::preserving(
            obj_from_bits(value_bits)
                .as_ptr()
                .unwrap_or(std::ptr::null_mut()),
        );
        if exception_pending(py) {
            return None;
        }
    }
    let mut expanded_owner = None;
    let spec_bits = if field.needs_expanding {
        let expanded_bits = format_string_impl(
            py,
            field.format_spec,
            None,
            args,
            kwargs_bits,
            state,
            recursion_depth - 1,
        )?
        .into_bits(py);
        expanded_owner = Some(PtrDropGuard::preserving(
            obj_from_bits(expanded_bits)
                .as_ptr()
                .unwrap_or(std::ptr::null_mut()),
        ));
        if exception_pending(py) {
            drop(value_owner);
            drop(expanded_owner);
            return None;
        }
        // __format__ receives an exact str projection, even when the nested
        // writer retained a subclass. Never redispatch its __str__ override.
        crate::object::ops_format::string_str_slot(expanded_bits)
    } else {
        let spec_ptr = alloc_string(py, field.format_spec);
        if spec_ptr.is_null() {
            return None;
        }
        MoltObject::from_ptr(spec_ptr).bits()
    };
    let spec_owner = PtrDropGuard::new(
        obj_from_bits(spec_bits)
            .as_ptr()
            .unwrap_or(std::ptr::null_mut()),
    );
    if exception_pending(py) {
        drop(spec_owner);
        drop(value_owner);
        drop(expanded_owner);
        return None;
    }
    let formatted_bits = molt_format_builtin(value_bits, spec_bits);
    drop(spec_owner);
    let result = if exception_pending(py) {
        molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, formatted_bits));
        None
    } else {
        match out.append_output(FormatOutput::OwnedString(formatted_bits), final_piece) {
            Ok(()) => Some(()),
            Err(error) => error.raise(py),
        }
    };
    // Consume/release the rendered result before the temporary field value;
    // an expanded spec outlives that value on success and failure alike.
    drop(value_owner);
    drop(expanded_owner);
    result
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_format_method(
    self_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(self_ptr) = obj_from_bits(self_bits)
            .as_ptr()
            .filter(|ptr| unsafe { object_type_id(*ptr) == TYPE_ID_STRING })
        else {
            return raise_exception(py, "TypeError", "format requires a string");
        };
        let Some(args_ptr) = obj_from_bits(args_bits)
            .as_ptr()
            .filter(|ptr| unsafe { object_type_id(*ptr) == TYPE_ID_TUPLE })
        else {
            return raise_exception(py, "TypeError", "format arguments must be a tuple");
        };
        // Immutable receiver and tuple storage remain pinned across every
        // lookup/conversion/format callback; no infallible snapshot copies.
        inc_ref_bits(py, self_bits);
        let _self_owner = PtrDropGuard::preserving(self_ptr);
        inc_ref_bits(py, args_bits);
        let _args_owner = PtrDropGuard::preserving(args_ptr);
        inc_ref_bits(py, kwargs_bits);
        let _kwargs_owner = PtrDropGuard::preserving(
            obj_from_bits(kwargs_bits)
                .as_ptr()
                .unwrap_or(std::ptr::null_mut()),
        );
        let mut state = FormatState {
            next_auto: 0,
            used_auto: false,
            used_manual: false,
            allow_positional: true,
            mapping_mode: false,
        };
        let rendered = unsafe {
            let text = std::slice::from_raw_parts(string_bytes(self_ptr), string_len(self_ptr));
            crate::object::seq_access::with_immutable_tuple_slice(args_ptr, |args| {
                format_string_impl(py, text, Some(self_bits), args, kwargs_bits, &mut state, 2)
            })
            .flatten()
        };
        let Some(rendered) = rendered else {
            return MoltObject::none().bits();
        };
        rendered.into_bits(py)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_format_map(self_bits: u64, mapping_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(self_ptr) = obj_from_bits(self_bits)
            .as_ptr()
            .filter(|ptr| unsafe { object_type_id(*ptr) == TYPE_ID_STRING })
        else {
            return raise_exception(py, "TypeError", "format_map requires a string");
        };
        inc_ref_bits(py, self_bits);
        let _self_owner = PtrDropGuard::preserving(self_ptr);
        inc_ref_bits(py, mapping_bits);
        let _mapping_owner = PtrDropGuard::preserving(
            obj_from_bits(mapping_bits)
                .as_ptr()
                .unwrap_or(std::ptr::null_mut()),
        );
        let text =
            unsafe { std::slice::from_raw_parts(string_bytes(self_ptr), string_len(self_ptr)) };
        let mut state = FormatState {
            next_auto: 0,
            used_auto: false,
            used_manual: false,
            allow_positional: false,
            mapping_mode: true,
        };
        let Some(rendered) =
            format_string_impl(py, text, Some(self_bits), &[], mapping_bits, &mut state, 2)
        else {
            return MoltObject::none().bits();
        };
        rendered.into_bits(py)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_format(val_bits: u64, spec_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let spec_obj = obj_from_bits(spec_bits);
        let Some(spec_ptr) = spec_obj
            .as_ptr()
            .filter(|ptr| unsafe { object_type_id(*ptr) } == TYPE_ID_STRING)
        else {
            let message = format!(
                "__format__() argument must be str, not {}",
                type_name(_py, spec_obj)
            );
            return raise_exception::<_>(_py, "TypeError", &message);
        };
        unsafe {
            let spec_bytes =
                std::slice::from_raw_parts(string_bytes(spec_ptr), string_len(spec_ptr));
            // CPython's native advanced-format writers delegate an empty spec
            // to str(obj). Nonempty specs use the declaring native formatter;
            // integer floating presentations invoke __float__, while explicit
            // base calls never redispatch the receiver's __format__ override.
            if spec_bytes.is_empty() {
                return molt_str_from_obj(val_bits);
            }
            let spec = match parse_format_spec(
                spec_bytes,
                crate::object::ops_sys::runtime_target_at_least(_py, 3, 14),
            ) {
                Ok(val) => val,
                Err(error) => return error.raise(_py, obj_from_bits(val_bits), spec_bytes),
            };
            let obj = obj_from_bits(val_bits);
            let rendered = match format_with_spec(_py, obj, &spec) {
                Ok(val) => val,
                Err(error) => return error.raise(_py),
            };
            rendered.into_bits(_py)
        }
    })
}
