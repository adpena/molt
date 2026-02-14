use molt_obj_model::MoltObject;

use crate::object::HEADER_FLAG_COROUTINE;
use crate::{
    TYPE_ID_FUNCTION, TYPE_ID_OBJECT, TYPE_ID_STRING, TYPE_ID_TYPE, alloc_dict_with_pairs,
    alloc_string, alloc_tuple, attr_name_bits_from_bytes, clear_exception, dec_ref_bits,
    decode_value_list, exception_pending, int_bits_from_i64, is_truthy, maybe_ptr_from_bits,
    missing_bits, molt_getattr_builtin, obj_from_bits, object_type_id, raise_exception,
    string_obj_to_owned, to_i64, type_of_bits,
};

fn get_attr_optional(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    name: &[u8],
) -> Result<Option<u64>, u64> {
    let Some(name_bits) = attr_name_bits_from_bytes(_py, name) else {
        return Err(MoltObject::none().bits());
    };
    let missing = missing_bits(_py);
    let value_bits = molt_getattr_builtin(obj_bits, name_bits, missing);
    dec_ref_bits(_py, name_bits);
    if exception_pending(_py) {
        if crate::builtins::attr::clear_attribute_error_if_pending(_py) {
            return Ok(None);
        }
        return Err(MoltObject::none().bits());
    }
    if value_bits == missing {
        return Ok(None);
    }
    Ok(Some(value_bits))
}

fn has_attr(_py: &crate::PyToken<'_>, obj_bits: u64, name: &[u8]) -> Result<bool, u64> {
    match get_attr_optional(_py, obj_bits, name)? {
        Some(value_bits) => {
            if !obj_from_bits(value_bits).is_none() {
                dec_ref_bits(_py, value_bits);
            }
            Ok(true)
        }
        None => Ok(false),
    }
}

fn attr_truthy(_py: &crate::PyToken<'_>, obj_bits: u64, name: &[u8]) -> Result<bool, u64> {
    match get_attr_optional(_py, obj_bits, name)? {
        Some(value_bits) => {
            let out = is_truthy(_py, obj_from_bits(value_bits));
            if !obj_from_bits(value_bits).is_none() {
                dec_ref_bits(_py, value_bits);
            }
            Ok(out)
        }
        None => Ok(false),
    }
}

fn code_flags_from_attr(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    code_attr_name: &[u8],
) -> Result<i64, u64> {
    let Some(code_bits) = get_attr_optional(_py, obj_bits, code_attr_name)? else {
        return Ok(0);
    };
    let result = match get_attr_optional(_py, code_bits, b"co_flags")? {
        Some(flags_bits) => {
            let flags = to_i64(obj_from_bits(flags_bits)).unwrap_or(0);
            if !obj_from_bits(flags_bits).is_none() {
                dec_ref_bits(_py, flags_bits);
            }
            Ok(flags)
        }
        None => Ok(0),
    };
    if !obj_from_bits(code_bits).is_none() {
        dec_ref_bits(_py, code_bits);
    }
    result
}

fn state_string(_py: &crate::PyToken<'_>, value: &[u8]) -> u64 {
    let ptr = alloc_string(_py, value);
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(ptr).bits()
}

fn frame_lasti(_py: &crate::PyToken<'_>, frame_bits: u64) -> Result<i64, u64> {
    let result = match get_attr_optional(_py, frame_bits, b"f_lasti")? {
        Some(lasti_bits) => {
            let out = to_i64(obj_from_bits(lasti_bits)).unwrap_or(-1);
            if !obj_from_bits(lasti_bits).is_none() {
                dec_ref_bits(_py, lasti_bits);
            }
            Ok(out)
        }
        None => Ok(-1),
    };
    if !obj_from_bits(frame_bits).is_none() {
        dec_ref_bits(_py, frame_bits);
    }
    result
}

fn alloc_tuple_bits(_py: &crate::PyToken<'_>, elems: &[u64]) -> u64 {
    let ptr = alloc_tuple(_py, elems);
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(ptr).bits()
}

fn alloc_empty_tuple_bits(_py: &crate::PyToken<'_>) -> u64 {
    alloc_tuple_bits(_py, &[])
}

fn alloc_empty_dict_bits(_py: &crate::PyToken<'_>) -> u64 {
    let ptr = alloc_dict_with_pairs(_py, &[]);
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(ptr).bits()
}

fn attr_i64_default(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    name: &[u8],
    default: i64,
) -> Result<i64, u64> {
    let value = match get_attr_optional(_py, obj_bits, name)? {
        Some(bits) => {
            let parsed = to_i64(obj_from_bits(bits)).unwrap_or(default);
            if !obj_from_bits(bits).is_none() {
                dec_ref_bits(_py, bits);
            }
            parsed
        }
        None => default,
    };
    Ok(value)
}

fn dec_owned_bits(_py: &crate::PyToken<'_>, owned: &[u64]) {
    for bits in owned {
        dec_ref_bits(_py, *bits);
    }
}

#[allow(clippy::too_many_arguments)]
fn make_signature_payload(
    _py: &crate::PyToken<'_>,
    arg_names_bits: u64,
    posonly: i64,
    kwonly_names_bits: u64,
    vararg_bits: u64,
    varkw_bits: u64,
    defaults_bits: u64,
    kwdefaults_bits: u64,
) -> u64 {
    let posonly_bits = int_bits_from_i64(_py, posonly.max(0));
    if obj_from_bits(posonly_bits).is_none() {
        return MoltObject::none().bits();
    }
    let out = alloc_tuple_bits(
        _py,
        &[
            arg_names_bits,
            posonly_bits,
            kwonly_names_bits,
            vararg_bits,
            varkw_bits,
            defaults_bits,
            kwdefaults_bits,
        ],
    );
    dec_ref_bits(_py, posonly_bits);
    out
}

fn signature_payload_from_molt(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    arg_names_bits: u64,
) -> Result<u64, u64> {
    let mut owned: Vec<u64> = vec![arg_names_bits];

    let posonly = match attr_i64_default(_py, obj_bits, b"__molt_posonly__", 0) {
        Ok(value) => value,
        Err(err) => {
            dec_owned_bits(_py, &owned);
            return Err(err);
        }
    };

    let kwonly_names_bits = match get_attr_optional(_py, obj_bits, b"__molt_kwonly_names__") {
        Ok(Some(bits)) if !obj_from_bits(bits).is_none() => bits,
        Ok(Some(bits)) => {
            let _ = bits;
            alloc_empty_tuple_bits(_py)
        }
        Ok(None) => alloc_empty_tuple_bits(_py),
        Err(err) => {
            dec_owned_bits(_py, &owned);
            return Err(err);
        }
    };
    if obj_from_bits(kwonly_names_bits).is_none() {
        dec_owned_bits(_py, &owned);
        return Ok(MoltObject::none().bits());
    }
    owned.push(kwonly_names_bits);

    let vararg_bits = match get_attr_optional(_py, obj_bits, b"__molt_vararg__") {
        Ok(Some(bits)) => {
            if !obj_from_bits(bits).is_none() {
                owned.push(bits);
            }
            bits
        }
        Ok(None) => MoltObject::none().bits(),
        Err(err) => {
            dec_owned_bits(_py, &owned);
            return Err(err);
        }
    };

    let varkw_bits = match get_attr_optional(_py, obj_bits, b"__molt_varkw__") {
        Ok(Some(bits)) => {
            if !obj_from_bits(bits).is_none() {
                owned.push(bits);
            }
            bits
        }
        Ok(None) => MoltObject::none().bits(),
        Err(err) => {
            dec_owned_bits(_py, &owned);
            return Err(err);
        }
    };

    let defaults_bits = match get_attr_optional(_py, obj_bits, b"__defaults__") {
        Ok(Some(bits)) if !obj_from_bits(bits).is_none() => bits,
        Ok(Some(bits)) => {
            let _ = bits;
            alloc_empty_tuple_bits(_py)
        }
        Ok(None) => alloc_empty_tuple_bits(_py),
        Err(err) => {
            dec_owned_bits(_py, &owned);
            return Err(err);
        }
    };
    if obj_from_bits(defaults_bits).is_none() {
        dec_owned_bits(_py, &owned);
        return Ok(MoltObject::none().bits());
    }
    owned.push(defaults_bits);

    let kwdefaults_bits = match get_attr_optional(_py, obj_bits, b"__kwdefaults__") {
        Ok(Some(bits)) if !obj_from_bits(bits).is_none() => bits,
        Ok(Some(bits)) => {
            let _ = bits;
            alloc_empty_dict_bits(_py)
        }
        Ok(None) => alloc_empty_dict_bits(_py),
        Err(err) => {
            dec_owned_bits(_py, &owned);
            return Err(err);
        }
    };
    if obj_from_bits(kwdefaults_bits).is_none() {
        dec_owned_bits(_py, &owned);
        return Ok(MoltObject::none().bits());
    }
    owned.push(kwdefaults_bits);

    let out = make_signature_payload(
        _py,
        arg_names_bits,
        posonly,
        kwonly_names_bits,
        vararg_bits,
        varkw_bits,
        defaults_bits,
        kwdefaults_bits,
    );
    dec_owned_bits(_py, &owned);
    Ok(out)
}

fn signature_payload_from_code(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    code_bits: u64,
) -> Result<Option<u64>, u64> {
    let mut owned: Vec<u64> = Vec::new();
    let posonly = attr_i64_default(_py, code_bits, b"co_posonlyargcount", 0)?;
    let argcount = attr_i64_default(_py, code_bits, b"co_argcount", 0)?;
    let kwonly = attr_i64_default(_py, code_bits, b"co_kwonlyargcount", 0)?;
    let flags = attr_i64_default(_py, code_bits, b"co_flags", 0)?;

    let varnames_bits = match get_attr_optional(_py, code_bits, b"co_varnames") {
        Ok(Some(bits)) if !obj_from_bits(bits).is_none() => bits,
        Ok(Some(bits)) => {
            let _ = bits;
            alloc_empty_tuple_bits(_py)
        }
        Ok(None) => alloc_empty_tuple_bits(_py),
        Err(err) => return Err(err),
    };
    if obj_from_bits(varnames_bits).is_none() {
        return Ok(None);
    }
    owned.push(varnames_bits);

    let Some(varnames) = decode_value_list(obj_from_bits(varnames_bits)) else {
        dec_owned_bits(_py, &owned);
        return Ok(None);
    };

    let total_pos = argcount.max(0) as usize;
    let kwonly_count = kwonly.max(0) as usize;
    if total_pos > varnames.len() {
        dec_owned_bits(_py, &owned);
        return Ok(None);
    }
    let posonly_clamped = posonly.clamp(0, argcount.max(0));

    let arg_names_bits = alloc_tuple_bits(_py, &varnames[..total_pos]);
    if obj_from_bits(arg_names_bits).is_none() {
        dec_owned_bits(_py, &owned);
        return Ok(None);
    }
    owned.push(arg_names_bits);

    let mut offset = total_pos;
    let vararg_bits = if (flags & 0x04) != 0 {
        let Some(bits) = varnames.get(offset).copied() else {
            dec_owned_bits(_py, &owned);
            return Ok(None);
        };
        offset += 1;
        bits
    } else {
        MoltObject::none().bits()
    };

    let Some(kw_end) = offset.checked_add(kwonly_count) else {
        dec_owned_bits(_py, &owned);
        return Ok(None);
    };
    if kw_end > varnames.len() {
        dec_owned_bits(_py, &owned);
        return Ok(None);
    }
    let kwonly_names_bits = alloc_tuple_bits(_py, &varnames[offset..kw_end]);
    if obj_from_bits(kwonly_names_bits).is_none() {
        dec_owned_bits(_py, &owned);
        return Ok(None);
    }
    owned.push(kwonly_names_bits);
    offset = kw_end;

    let varkw_bits = if (flags & 0x08) != 0 {
        let Some(bits) = varnames.get(offset).copied() else {
            dec_owned_bits(_py, &owned);
            return Ok(None);
        };
        bits
    } else {
        MoltObject::none().bits()
    };

    let defaults_bits = match get_attr_optional(_py, obj_bits, b"__defaults__") {
        Ok(Some(bits)) if !obj_from_bits(bits).is_none() => bits,
        Ok(Some(bits)) => {
            let _ = bits;
            alloc_empty_tuple_bits(_py)
        }
        Ok(None) => alloc_empty_tuple_bits(_py),
        Err(err) => {
            dec_owned_bits(_py, &owned);
            return Err(err);
        }
    };
    if obj_from_bits(defaults_bits).is_none() {
        dec_owned_bits(_py, &owned);
        return Ok(None);
    }
    owned.push(defaults_bits);

    let kwdefaults_bits = match get_attr_optional(_py, obj_bits, b"__kwdefaults__") {
        Ok(Some(bits)) if !obj_from_bits(bits).is_none() => bits,
        Ok(Some(bits)) => {
            let _ = bits;
            alloc_empty_dict_bits(_py)
        }
        Ok(None) => alloc_empty_dict_bits(_py),
        Err(err) => {
            dec_owned_bits(_py, &owned);
            return Err(err);
        }
    };
    if obj_from_bits(kwdefaults_bits).is_none() {
        dec_owned_bits(_py, &owned);
        return Ok(None);
    }
    owned.push(kwdefaults_bits);

    let out = make_signature_payload(
        _py,
        arg_names_bits,
        posonly_clamped,
        kwonly_names_bits,
        vararg_bits,
        varkw_bits,
        defaults_bits,
        kwdefaults_bits,
    );
    dec_owned_bits(_py, &owned);
    if obj_from_bits(out).is_none() {
        return Ok(None);
    }
    Ok(Some(out))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TextParamKind {
    PositionalOrKeyword,
    KeywordOnly,
}

enum TextDefaultValue {
    None,
    Bool(bool),
    Ellipsis,
    EmptyTuple,
    Int(i64),
    Float(f64),
    String(String),
}

struct TextParamSpec {
    name: String,
    kind: TextParamKind,
    default: Option<TextDefaultValue>,
}

#[derive(Default)]
struct ParsedTextSignature {
    params: Vec<TextParamSpec>,
    vararg: Option<String>,
    varkw: Option<String>,
    posonly_cut: usize,
}

fn split_text_signature_tokens(payload: &str) -> Option<Vec<String>> {
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut depth: i64 = 0;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for ch in payload.chars() {
        if let Some(active_quote) = quote {
            current.push(ch);
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == active_quote {
                quote = None;
            }
            continue;
        }

        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            current.push(ch);
            continue;
        }
        match ch {
            '(' | '[' | '{' => {
                depth += 1;
                current.push(ch);
            }
            ')' | ']' | '}' => {
                depth = (depth - 1).max(0);
                current.push(ch);
            }
            ',' if depth == 0 => {
                let token = current.trim();
                if !token.is_empty() {
                    tokens.push(token.to_string());
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    if quote.is_some() || depth != 0 {
        return None;
    }
    let tail = current.trim();
    if !tail.is_empty() {
        tokens.push(tail.to_string());
    }
    Some(tokens)
}

fn parse_text_default(value: &str) -> Option<TextDefaultValue> {
    let trimmed = value.trim();
    match trimmed {
        "None" => return Some(TextDefaultValue::None),
        "True" => return Some(TextDefaultValue::Bool(true)),
        "False" => return Some(TextDefaultValue::Bool(false)),
        "..." => return Some(TextDefaultValue::Ellipsis),
        "()" => return Some(TextDefaultValue::EmptyTuple),
        _ => {}
    }

    fn unescape_text_sig_string(value: &str) -> String {
        // Best-effort unescape for CPython `__text_signature__` string literals.
        // This is intentionally minimal: it covers the escapes we rely on for parity
        // in builtin defaults (notably `print(end='\\n')`).
        let mut out = String::with_capacity(value.len());
        let mut chars = value.chars();
        while let Some(ch) = chars.next() {
            if ch != '\\' {
                out.push(ch);
                continue;
            }
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('a') => out.push('\x07'),
                Some('b') => out.push('\x08'),
                Some('f') => out.push('\x0c'),
                Some('v') => out.push('\x0b'),
                Some('\\') => out.push('\\'),
                Some('\'') => out.push('\''),
                Some('\"') => out.push('\"'),
                Some(other) => {
                    // Preserve unknown escapes as literal.
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        }
        out
    }

    if let Some(inner) = trimmed
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    {
        return Some(TextDefaultValue::String(unescape_text_sig_string(inner)));
    }
    if let Some(inner) = trimmed
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
    {
        return Some(TextDefaultValue::String(unescape_text_sig_string(inner)));
    }

    if trimmed.chars().any(|ch| matches!(ch, '.' | 'e' | 'E')) {
        if let Ok(value) = trimmed.parse::<f64>() {
            return Some(TextDefaultValue::Float(value));
        }
    }
    if let Ok(value) = trimmed.parse::<i64>() {
        return Some(TextDefaultValue::Int(value));
    }
    None
}

fn parse_text_signature(text: &str) -> Option<ParsedTextSignature> {
    let trimmed = text.trim();
    if trimmed.is_empty() || !trimmed.starts_with('(') || !trimmed.ends_with(')') {
        return None;
    }
    let inner = trimmed[1..trimmed.len().saturating_sub(1)].trim();
    if inner.is_empty() {
        return Some(ParsedTextSignature::default());
    }

    let tokens = split_text_signature_tokens(inner)?;
    let mut parsed = ParsedTextSignature::default();
    let mut saw_posonly_marker = false;
    let mut kwonly = false;
    for token in tokens {
        if token == "/" {
            if saw_posonly_marker {
                return None;
            }
            saw_posonly_marker = true;
            parsed.posonly_cut = parsed.params.len();
            continue;
        }
        if token == "*" {
            kwonly = true;
            continue;
        }
        if let Some(name) = token.strip_prefix("**") {
            let normalized = name.trim();
            if normalized.is_empty() {
                return None;
            }
            parsed.varkw = Some(normalized.to_string());
            continue;
        }
        if let Some(name) = token.strip_prefix('*') {
            let normalized = name.trim();
            if normalized.is_empty() {
                return None;
            }
            parsed.vararg = Some(normalized.to_string());
            kwonly = true;
            continue;
        }

        let (name, default) = if let Some((head, tail)) = token.split_once('=') {
            (head.trim(), parse_text_default(tail))
        } else {
            (token.trim(), None)
        };
        if name.is_empty() {
            return None;
        }
        let kind = if kwonly {
            TextParamKind::KeywordOnly
        } else {
            TextParamKind::PositionalOrKeyword
        };
        parsed.params.push(TextParamSpec {
            name: name.to_string(),
            kind,
            default,
        });
    }
    Some(parsed)
}

fn alloc_owned_string_bits(
    _py: &crate::PyToken<'_>,
    value: &str,
    owned: &mut Vec<u64>,
) -> Option<u64> {
    let ptr = alloc_string(_py, value.as_bytes());
    if ptr.is_null() {
        return None;
    }
    let bits = MoltObject::from_ptr(ptr).bits();
    owned.push(bits);
    Some(bits)
}

fn alloc_owned_tuple_bits(
    _py: &crate::PyToken<'_>,
    elems: &[u64],
    owned: &mut Vec<u64>,
) -> Option<u64> {
    let bits = alloc_tuple_bits(_py, elems);
    if obj_from_bits(bits).is_none() {
        return None;
    }
    owned.push(bits);
    Some(bits)
}

fn alloc_owned_dict_bits(
    _py: &crate::PyToken<'_>,
    pairs: &[u64],
    owned: &mut Vec<u64>,
) -> Option<u64> {
    let ptr = alloc_dict_with_pairs(_py, pairs);
    if ptr.is_null() {
        return None;
    }
    let bits = MoltObject::from_ptr(ptr).bits();
    owned.push(bits);
    Some(bits)
}

fn default_value_bits(
    _py: &crate::PyToken<'_>,
    default: &TextDefaultValue,
    owned: &mut Vec<u64>,
) -> Option<u64> {
    match default {
        TextDefaultValue::None => Some(MoltObject::none().bits()),
        TextDefaultValue::Bool(value) => Some(MoltObject::from_bool(*value).bits()),
        TextDefaultValue::Ellipsis => {
            let bits = crate::ellipsis_bits(_py);
            if obj_from_bits(bits).is_none() {
                None
            } else {
                Some(bits)
            }
        }
        TextDefaultValue::EmptyTuple => {
            let bits = alloc_empty_tuple_bits(_py);
            if obj_from_bits(bits).is_none() {
                None
            } else {
                owned.push(bits);
                Some(bits)
            }
        }
        TextDefaultValue::Int(value) => Some(int_bits_from_i64(_py, *value)),
        TextDefaultValue::Float(value) => Some(MoltObject::from_float(*value).bits()),
        TextDefaultValue::String(value) => alloc_owned_string_bits(_py, value, owned),
    }
}

fn signature_payload_from_text_signature(_py: &crate::PyToken<'_>, text: &str) -> Option<u64> {
    let parsed = parse_text_signature(text)?;
    let mut owned: Vec<u64> = Vec::new();

    let positional_params: Vec<&TextParamSpec> = parsed
        .params
        .iter()
        .filter(|param| param.kind == TextParamKind::PositionalOrKeyword)
        .collect();
    let kwonly_params: Vec<&TextParamSpec> = parsed
        .params
        .iter()
        .filter(|param| param.kind == TextParamKind::KeywordOnly)
        .collect();

    let mut arg_name_bits: Vec<u64> = Vec::with_capacity(positional_params.len());
    for param in &positional_params {
        let Some(bits) = alloc_owned_string_bits(_py, &param.name, &mut owned) else {
            dec_owned_bits(_py, &owned);
            return None;
        };
        arg_name_bits.push(bits);
    }
    let mut kwonly_name_bits: Vec<u64> = Vec::with_capacity(kwonly_params.len());
    for param in &kwonly_params {
        let Some(bits) = alloc_owned_string_bits(_py, &param.name, &mut owned) else {
            dec_owned_bits(_py, &owned);
            return None;
        };
        kwonly_name_bits.push(bits);
    }

    let vararg_bits = if let Some(name) = parsed.vararg.as_deref() {
        let Some(bits) = alloc_owned_string_bits(_py, name, &mut owned) else {
            dec_owned_bits(_py, &owned);
            return None;
        };
        bits
    } else {
        MoltObject::none().bits()
    };
    let varkw_bits = if let Some(name) = parsed.varkw.as_deref() {
        let Some(bits) = alloc_owned_string_bits(_py, name, &mut owned) else {
            dec_owned_bits(_py, &owned);
            return None;
        };
        bits
    } else {
        MoltObject::none().bits()
    };

    let mut positional_defaults: Vec<Option<u64>> = Vec::with_capacity(positional_params.len());
    for param in &positional_params {
        let Some(default) = param.default.as_ref() else {
            positional_defaults.push(None);
            continue;
        };
        let Some(bits) = default_value_bits(_py, default, &mut owned) else {
            dec_owned_bits(_py, &owned);
            return None;
        };
        positional_defaults.push(Some(bits));
    }
    let mut defaults_bits_vec: Vec<u64> = Vec::new();
    if let Some(default_start) = positional_defaults.iter().position(Option::is_some) {
        if positional_defaults[default_start..]
            .iter()
            .any(Option::is_none)
        {
            dec_owned_bits(_py, &owned);
            return None;
        }
        defaults_bits_vec.extend(
            positional_defaults[default_start..]
                .iter()
                .filter_map(|value| *value),
        );
    }

    let mut kwdefault_pairs: Vec<u64> = Vec::new();
    for (index, param) in kwonly_params.iter().enumerate() {
        let Some(default) = param.default.as_ref() else {
            continue;
        };
        let Some(default_bits) = default_value_bits(_py, default, &mut owned) else {
            dec_owned_bits(_py, &owned);
            return None;
        };
        kwdefault_pairs.push(kwonly_name_bits[index]);
        kwdefault_pairs.push(default_bits);
    }

    let Some(arg_names_bits) = alloc_owned_tuple_bits(_py, &arg_name_bits, &mut owned) else {
        dec_owned_bits(_py, &owned);
        return None;
    };
    let posonly = parsed.posonly_cut.min(positional_params.len()) as i64;
    let Some(kwonly_names_bits) = alloc_owned_tuple_bits(_py, &kwonly_name_bits, &mut owned) else {
        dec_owned_bits(_py, &owned);
        return None;
    };
    let Some(defaults_bits) = alloc_owned_tuple_bits(_py, &defaults_bits_vec, &mut owned) else {
        dec_owned_bits(_py, &owned);
        return None;
    };
    let Some(kwdefaults_bits) = alloc_owned_dict_bits(_py, &kwdefault_pairs, &mut owned) else {
        dec_owned_bits(_py, &owned);
        return None;
    };

    let out = make_signature_payload(
        _py,
        arg_names_bits,
        posonly,
        kwonly_names_bits,
        vararg_bits,
        varkw_bits,
        defaults_bits,
        kwdefaults_bits,
    );
    dec_owned_bits(_py, &owned);
    if obj_from_bits(out).is_none() {
        None
    } else {
        Some(out)
    }
}

fn signature_payload_from_text_attr(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    attr_name: &[u8],
) -> Result<Option<u64>, u64> {
    let Some(attr_bits) = get_attr_optional(_py, obj_bits, attr_name)? else {
        return Ok(None);
    };
    if obj_from_bits(attr_bits).is_none() {
        return Ok(None);
    }
    let Some(text) = string_obj_to_owned(obj_from_bits(attr_bits)) else {
        dec_ref_bits(_py, attr_bits);
        return Ok(None);
    };
    dec_ref_bits(_py, attr_bits);
    Ok(signature_payload_from_text_signature(_py, &text))
}

fn inspect_cleandoc_text(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut expanded = String::with_capacity(text.len());
    let mut col: usize = 0;
    for ch in text.chars() {
        if ch == '\t' {
            let spaces = 8 - (col % 8);
            for _ in 0..spaces {
                expanded.push(' ');
            }
            col += spaces;
        } else if ch == '\r' || ch == '\n' {
            expanded.push(ch);
            col = 0;
        } else {
            expanded.push(ch);
            col += 1;
        }
    }

    let mut lines: Vec<&str> = expanded.lines().collect();
    while !lines.is_empty() && lines.first().is_some_and(|line| line.trim().is_empty()) {
        lines.remove(0);
    }
    while !lines.is_empty() && lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    if lines.is_empty() {
        return String::new();
    }

    let mut indent: Option<usize> = None;
    for line in &lines {
        let stripped = line.trim_start();
        if stripped.is_empty() {
            continue;
        }
        let margin = line.len() - stripped.len();
        indent = Some(match indent {
            Some(current) => current.min(margin),
            None => margin,
        });
    }
    let indent = indent.unwrap_or(0);
    let mut out = String::new();
    for (idx, line) in lines.iter().enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        if indent >= line.len() {
            continue;
        }
        out.push_str(&line[indent..]);
    }
    out
}

fn inspect_cleandoc_impl(_py: &crate::PyToken<'_>, doc_bits: u64) -> u64 {
    if !is_truthy(_py, obj_from_bits(doc_bits)) {
        return state_string(_py, b"");
    }
    let Some(ptr) = maybe_ptr_from_bits(doc_bits) else {
        return raise_exception(_py, "TypeError", "inspect.cleandoc() argument must be str");
    };
    unsafe {
        if crate::object_type_id(ptr) != TYPE_ID_STRING {
            return raise_exception(_py, "TypeError", "inspect.cleandoc() argument must be str");
        }
    }
    let Some(bytes) = string_obj_to_owned(obj_from_bits(doc_bits)) else {
        return raise_exception(_py, "TypeError", "inspect.cleandoc() argument must be str");
    };
    let cleaned = inspect_cleandoc_text(&bytes);
    state_string(_py, cleaned.as_bytes())
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inspect_signature_data(obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let builtins = crate::builtins::classes::builtin_classes(_py);
        let is_builtin_fn = type_of_bits(_py, obj_bits) == builtins.builtin_function_or_method;

        // Prefer `__text_signature__` when present (CPython parity), but only for objects where
        // CPython uses it for signature discovery (builtin functions and types). Avoid inheriting
        // `object.__text_signature__` across arbitrary instances.
        if let Some(ptr) = maybe_ptr_from_bits(obj_bits) {
            let type_id = unsafe { object_type_id(ptr) };
            if type_id == TYPE_ID_FUNCTION || type_id == TYPE_ID_TYPE {
                let sig_from_text =
                    match signature_payload_from_text_attr(_py, obj_bits, b"__text_signature__") {
                        Ok(value) => value,
                        Err(err) => return err,
                    };
                if let Some(sig_bits) = sig_from_text {
                    return sig_bits;
                }
            }
        }

        // For Molt-defined (non-builtin) functions, prefer compiler-provided signature metadata.
        // For runtime-created builtins, do not use Molt metadata or `__code__` fallbacks.
        if !is_builtin_fn {
            let molt_args = match get_attr_optional(_py, obj_bits, b"__molt_arg_names__") {
                Ok(value) => value,
                Err(err) => return err,
            };
            if let Some(arg_names_bits) = molt_args {
                if !obj_from_bits(arg_names_bits).is_none() {
                    return match signature_payload_from_molt(_py, obj_bits, arg_names_bits) {
                        Ok(bits) => bits,
                        Err(err) => err,
                    };
                }
            }
        }

        let code_bits_opt = match get_attr_optional(_py, obj_bits, b"__code__") {
            Ok(value) => value,
            Err(err) => return err,
        };
        if let Some(code_bits) = code_bits_opt {
            if !obj_from_bits(code_bits).is_none() {
                if !is_builtin_fn {
                    let out = match signature_payload_from_code(_py, obj_bits, code_bits) {
                        Ok(Some(bits)) => bits,
                        Ok(None) => MoltObject::none().bits(),
                        Err(err) => err,
                    };
                    dec_ref_bits(_py, code_bits);
                    if !obj_from_bits(out).is_none() {
                        return out;
                    }
                } else {
                    dec_ref_bits(_py, code_bits);
                }
            }
        }

        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inspect_cleandoc(doc_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { inspect_cleandoc_impl(_py, doc_bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inspect_currentframe() -> u64 {
    crate::with_gil_entry!(_py, {
        // Skip intrinsic + inspect.currentframe wrapper frames to match CPython.
        let depth_bits = int_bits_from_i64(_py, 2);
        if obj_from_bits(depth_bits).is_none() {
            return MoltObject::none().bits();
        }
        let frame_bits = crate::molt_getframe(depth_bits);
        dec_ref_bits(_py, depth_bits);
        if exception_pending(_py) {
            clear_exception(_py);
            return MoltObject::none().bits();
        }
        frame_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inspect_getdoc(obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(doc_bits) = (match get_attr_optional(_py, obj_bits, b"__doc__") {
            Ok(value) => value,
            Err(err) => return err,
        }) else {
            return MoltObject::none().bits();
        };
        if obj_from_bits(doc_bits).is_none() {
            return MoltObject::none().bits();
        }
        let out = inspect_cleandoc_impl(_py, doc_bits);
        dec_ref_bits(_py, doc_bits);
        out
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inspect_isfunction(obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let has_code = match has_attr(_py, obj_bits, b"__code__") {
            Ok(value) => value,
            Err(err) => return err,
        };
        if has_code {
            return MoltObject::from_bool(true).bits();
        }
        let has_molt_args = match has_attr(_py, obj_bits, b"__molt_arg_names__") {
            Ok(value) => value,
            Err(err) => return err,
        };
        MoltObject::from_bool(has_molt_args).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inspect_isclass(obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let has_mro = match has_attr(_py, obj_bits, b"__mro__") {
            Ok(value) => value,
            Err(err) => return err,
        };
        MoltObject::from_bool(has_mro).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inspect_ismodule(obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let has_dict = match has_attr(_py, obj_bits, b"__dict__") {
            Ok(value) => value,
            Err(err) => return err,
        };
        if !has_dict {
            return MoltObject::from_bool(false).bits();
        }
        let has_name = match has_attr(_py, obj_bits, b"__name__") {
            Ok(value) => value,
            Err(err) => return err,
        };
        MoltObject::from_bool(has_name).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inspect_iscoroutine(obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let marker = match attr_truthy(_py, obj_bits, b"__molt_is_coroutine__") {
            Ok(value) => value,
            Err(err) => return err,
        };
        if marker {
            return MoltObject::from_bool(true).bits();
        }

        if let Some(ptr) = maybe_ptr_from_bits(obj_bits) {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_OBJECT {
                    let header = crate::header_from_obj_ptr(ptr);
                    if ((*header).flags & HEADER_FLAG_COROUTINE) != 0 {
                        return MoltObject::from_bool(true).bits();
                    }
                }
            }
        }

        let has_cr_code = match has_attr(_py, obj_bits, b"cr_code") {
            Ok(value) => value,
            Err(err) => return err,
        };
        if !has_cr_code {
            return MoltObject::from_bool(false).bits();
        }
        let has_cr_frame = match has_attr(_py, obj_bits, b"cr_frame") {
            Ok(value) => value,
            Err(err) => return err,
        };
        MoltObject::from_bool(has_cr_frame).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inspect_iscoroutinefunction(obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let marker = match attr_truthy(_py, obj_bits, b"__molt_is_coroutine__") {
            Ok(value) => value,
            Err(err) => return err,
        };
        if marker {
            return MoltObject::from_bool(true).bits();
        }
        let flags = match code_flags_from_attr(_py, obj_bits, b"__code__") {
            Ok(value) => value,
            Err(err) => return err,
        };
        MoltObject::from_bool((flags & 0x80) != 0).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inspect_isasyncgenfunction(obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let marker = match attr_truthy(_py, obj_bits, b"__molt_is_async_generator__") {
            Ok(value) => value,
            Err(err) => return err,
        };
        if marker {
            return MoltObject::from_bool(true).bits();
        }
        let flags = match code_flags_from_attr(_py, obj_bits, b"__code__") {
            Ok(value) => value,
            Err(err) => return err,
        };
        MoltObject::from_bool((flags & 0x200) != 0).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inspect_isgeneratorfunction(obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let marker = match attr_truthy(_py, obj_bits, b"__molt_is_generator__") {
            Ok(value) => value,
            Err(err) => return err,
        };
        if marker {
            return MoltObject::from_bool(true).bits();
        }
        let flags = match code_flags_from_attr(_py, obj_bits, b"__code__") {
            Ok(value) => value,
            Err(err) => return err,
        };
        MoltObject::from_bool((flags & 0x20) != 0).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inspect_isawaitable(obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let marker = match attr_truthy(_py, obj_bits, b"__molt_is_coroutine__") {
            Ok(value) => value,
            Err(err) => return err,
        };
        if marker {
            return MoltObject::from_bool(true).bits();
        }
        let has_await = match has_attr(_py, obj_bits, b"__await__") {
            Ok(value) => value,
            Err(err) => return err,
        };
        if has_await {
            return MoltObject::from_bool(true).bits();
        }
        let flags = match code_flags_from_attr(_py, obj_bits, b"gi_code") {
            Ok(value) => value,
            Err(err) => return err,
        };
        MoltObject::from_bool((flags & 0x100) != 0).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inspect_getgeneratorstate(gen_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let running = match attr_truthy(_py, gen_bits, b"gi_running") {
            Ok(value) => value,
            Err(err) => return err,
        };
        if running {
            return state_string(_py, b"GEN_RUNNING");
        }
        let Some(frame_bits) = (match get_attr_optional(_py, gen_bits, b"gi_frame") {
            Ok(value) => value,
            Err(err) => return err,
        }) else {
            return state_string(_py, b"GEN_CLOSED");
        };
        if obj_from_bits(frame_bits).is_none() {
            return state_string(_py, b"GEN_CLOSED");
        }
        let lasti = match frame_lasti(_py, frame_bits) {
            Ok(value) => value,
            Err(err) => return err,
        };
        if lasti == -1 {
            return state_string(_py, b"GEN_CREATED");
        }
        state_string(_py, b"GEN_SUSPENDED")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inspect_getasyncgenstate(agen_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let running = match attr_truthy(_py, agen_bits, b"ag_running") {
            Ok(value) => value,
            Err(err) => return err,
        };
        if running {
            return state_string(_py, b"AGEN_RUNNING");
        }
        let Some(frame_bits) = (match get_attr_optional(_py, agen_bits, b"ag_frame") {
            Ok(value) => value,
            Err(err) => return err,
        }) else {
            return state_string(_py, b"AGEN_CLOSED");
        };
        if obj_from_bits(frame_bits).is_none() {
            return state_string(_py, b"AGEN_CLOSED");
        }
        let lasti = match frame_lasti(_py, frame_bits) {
            Ok(value) => value,
            Err(err) => return err,
        };
        if lasti == -1 {
            return state_string(_py, b"AGEN_CREATED");
        }
        state_string(_py, b"AGEN_SUSPENDED")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inspect_getcoroutinestate(coro_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let running = match attr_truthy(_py, coro_bits, b"cr_running") {
            Ok(value) => value,
            Err(err) => return err,
        };
        if running {
            return state_string(_py, b"CORO_RUNNING");
        }

        let frame_bits = match get_attr_optional(_py, coro_bits, b"cr_frame") {
            Ok(value) => value,
            Err(err) => return err,
        };

        let selected_frame = if let Some(frame_bits) = frame_bits {
            if obj_from_bits(frame_bits).is_none() {
                None
            } else {
                Some(frame_bits)
            }
        } else {
            let gi_running = match attr_truthy(_py, coro_bits, b"gi_running") {
                Ok(value) => value,
                Err(err) => return err,
            };
            if gi_running {
                return state_string(_py, b"CORO_RUNNING");
            }
            match get_attr_optional(_py, coro_bits, b"gi_frame") {
                Ok(value) => value,
                Err(err) => return err,
            }
            .and_then(|bits| {
                if obj_from_bits(bits).is_none() {
                    None
                } else {
                    Some(bits)
                }
            })
        };

        let Some(frame_bits) = selected_frame else {
            return state_string(_py, b"CORO_CLOSED");
        };
        let lasti = match frame_lasti(_py, frame_bits) {
            Ok(value) => value,
            Err(err) => return err,
        };
        if lasti == -1 {
            return state_string(_py, b"CORO_CREATED");
        }
        state_string(_py, b"CORO_SUSPENDED")
    })
}
