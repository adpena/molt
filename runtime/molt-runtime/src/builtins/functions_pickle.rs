use molt_obj_model::MoltObject;
use std::collections::HashMap;

use crate::{
    TYPE_ID_DICT, TYPE_ID_LIST, TYPE_ID_STRING, TYPE_ID_TUPLE, alloc_dict_with_pairs,
    alloc_list_with_capacity, alloc_string, alloc_tuple, attr_name_bits_from_bytes,
    builtin_classes, bytes_like_slice, call_callable0, call_callable1, call_callable2,
    call_callable3, clear_exception, dec_ref_bits, dict_get_in_place, exception_pending,
    format_obj, inc_ref_bits, is_truthy, missing_bits, molt_getattr_builtin, molt_is_callable,
    molt_iter, obj_from_bits, object_class_bits, object_type_id, raise_exception, seq_vec_ref,
    string_obj_to_owned, to_i64, type_name, type_of_bits,
};

fn alloc_string_bits(_py: &crate::PyToken<'_>, value: &str) -> Option<u64> {
    let ptr = alloc_string(_py, value.as_bytes());
    if ptr.is_null() {
        None
    } else {
        Some(MoltObject::from_ptr(ptr).bits())
    }
}

fn iter_next_pair(_py: &crate::PyToken<'_>, iter_bits: u64) -> Result<(u64, bool), u64> {
    let pair_bits = crate::molt_iter_next(iter_bits);
    let pair_obj = obj_from_bits(pair_bits);
    let Some(pair_ptr) = pair_obj.as_ptr() else {
        return Err(MoltObject::none().bits());
    };
    unsafe {
        if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
            return Err(MoltObject::none().bits());
        }
        let elems = seq_vec_ref(pair_ptr);
        if elems.len() < 2 {
            return Err(MoltObject::none().bits());
        }
        let val_bits = elems[0];
        let done_bits = elems[1];
        let done = is_truthy(_py, obj_from_bits(done_bits));
        Ok((val_bits, done))
    }
}

#[derive(Clone, Copy, Debug)]
enum PickleGlobal {
    CodecsEncode,
    Bytearray,
    Slice,
    Set,
    FrozenSet,
    List,
    Tuple,
    Dict,
}

fn pickle_raise(_py: &crate::PyToken<'_>, message: &str) -> u64 {
    raise_exception::<u64>(_py, "RuntimeError", message)
}

fn pickle_decode_latin1(input: &[u8]) -> String {
    input.iter().map(|&b| char::from(b)).collect()
}

fn pickle_parse_string_literal(text: &str) -> Result<String, &'static str> {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() < 2 {
        return Err("pickle.loads: invalid string literal");
    }
    let quote = chars[0];
    if (quote != '\'' && quote != '"') || chars[chars.len() - 1] != quote {
        return Err("pickle.loads: invalid string literal");
    }
    let mut out = String::new();
    let mut idx = 1usize;
    let end = chars.len() - 1;
    while idx < end {
        let ch = chars[idx];
        if ch != '\\' {
            out.push(ch);
            idx += 1;
            continue;
        }
        idx += 1;
        if idx >= end {
            return Err("pickle.loads: invalid escape sequence");
        }
        let esc = chars[idx];
        idx += 1;
        match esc {
            'a' => out.push('\u{0007}'),
            'b' => out.push('\u{0008}'),
            'f' => out.push('\u{000c}'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            'v' => out.push('\u{000b}'),
            '\\' | '\'' | '"' => out.push(esc),
            'x' => {
                if idx + 2 > end {
                    return Err("pickle.loads: invalid hex escape");
                }
                let hex_text: String = chars[idx..idx + 2].iter().collect();
                let Ok(value) = u32::from_str_radix(&hex_text, 16) else {
                    return Err("pickle.loads: invalid hex escape");
                };
                let Some(decoded) = char::from_u32(value) else {
                    return Err("pickle.loads: invalid hex escape");
                };
                out.push(decoded);
                idx += 2;
            }
            'u' => {
                if idx + 4 > end {
                    return Err("pickle.loads: invalid unicode escape");
                }
                let hex_text: String = chars[idx..idx + 4].iter().collect();
                let Ok(value) = u32::from_str_radix(&hex_text, 16) else {
                    return Err("pickle.loads: invalid unicode escape");
                };
                let Some(decoded) = char::from_u32(value) else {
                    return Err("pickle.loads: invalid unicode escape");
                };
                out.push(decoded);
                idx += 4;
            }
            'U' => {
                if idx + 8 > end {
                    return Err("pickle.loads: invalid unicode escape");
                }
                let hex_text: String = chars[idx..idx + 8].iter().collect();
                let Ok(value) = u32::from_str_radix(&hex_text, 16) else {
                    return Err("pickle.loads: invalid unicode escape");
                };
                let Some(decoded) = char::from_u32(value) else {
                    return Err("pickle.loads: invalid unicode escape");
                };
                out.push(decoded);
                idx += 8;
            }
            '0'..='7' => {
                let mut octal = String::new();
                octal.push(esc);
                let limit = (idx + 2).min(end);
                while idx < limit && matches!(chars[idx], '0'..='7') {
                    octal.push(chars[idx]);
                    idx += 1;
                }
                let Ok(value) = u32::from_str_radix(&octal, 8) else {
                    return Err("pickle.loads: invalid escape sequence");
                };
                let Some(decoded) = char::from_u32(value) else {
                    return Err("pickle.loads: invalid escape sequence");
                };
                out.push(decoded);
            }
            _ => return Err("pickle.loads: invalid escape sequence"),
        }
    }
    Ok(out)
}

fn pickle_parse_int_bits(_py: &crate::PyToken<'_>, text: &str) -> Result<u64, u64> {
    if let Ok(value) = text.parse::<i64>() {
        return Ok(MoltObject::from_int(value).bits());
    }
    let Some(text_bits) = alloc_string_bits(_py, text) else {
        return Err(MoltObject::none().bits());
    };
    let out_bits = unsafe { call_callable1(_py, builtin_classes(_py).int, text_bits) };
    dec_ref_bits(_py, text_bits);
    if exception_pending(_py) {
        Err(MoltObject::none().bits())
    } else {
        Ok(out_bits)
    }
}

fn pickle_parse_long_line_bits(_py: &crate::PyToken<'_>, text: &str) -> Result<u64, u64> {
    let trimmed = text.trim_end_matches(['L', 'l']);
    let Some(text_bits) = alloc_string_bits(_py, trimmed) else {
        return Err(MoltObject::none().bits());
    };
    let out_bits = unsafe { call_callable1(_py, builtin_classes(_py).int, text_bits) };
    dec_ref_bits(_py, text_bits);
    if exception_pending(_py) {
        Err(MoltObject::none().bits())
    } else {
        Ok(out_bits)
    }
}

fn pickle_parse_float_bits(_py: &crate::PyToken<'_>, text: &str) -> Result<u64, u64> {
    let Some(text_bits) = alloc_string_bits(_py, text) else {
        return Err(MoltObject::none().bits());
    };
    let out_bits = unsafe { call_callable1(_py, builtin_classes(_py).float, text_bits) };
    dec_ref_bits(_py, text_bits);
    if exception_pending(_py) {
        Err(MoltObject::none().bits())
    } else {
        Ok(out_bits)
    }
}

fn pickle_resolve_global(module: &str, name: &str) -> Option<PickleGlobal> {
    match (module, name) {
        ("_codecs", "encode") => Some(PickleGlobal::CodecsEncode),
        ("builtins", "bytearray") | ("__builtin__", "bytearray") => Some(PickleGlobal::Bytearray),
        ("builtins", "slice") | ("__builtin__", "slice") => Some(PickleGlobal::Slice),
        ("builtins", "set") | ("__builtin__", "set") => Some(PickleGlobal::Set),
        ("builtins", "frozenset") | ("__builtin__", "frozenset") => Some(PickleGlobal::FrozenSet),
        ("builtins", "list") | ("__builtin__", "list") => Some(PickleGlobal::List),
        ("builtins", "tuple") | ("__builtin__", "tuple") => Some(PickleGlobal::Tuple),
        ("builtins", "dict") | ("__builtin__", "dict") => Some(PickleGlobal::Dict),
        _ => None,
    }
}

fn pickle_global_callable_bits(_py: &crate::PyToken<'_>, global: PickleGlobal) -> Result<u64, u64> {
    match global {
        PickleGlobal::CodecsEncode => Err(pickle_raise(
            _py,
            "pickle.loads: _codecs.encode cannot be materialized as a standalone callable",
        )),
        PickleGlobal::Bytearray => Ok(builtin_classes(_py).bytearray),
        PickleGlobal::Slice => Ok(builtin_classes(_py).slice),
        PickleGlobal::Set => Ok(builtin_classes(_py).set),
        PickleGlobal::FrozenSet => Ok(builtin_classes(_py).frozenset),
        PickleGlobal::List => Ok(builtin_classes(_py).list),
        PickleGlobal::Tuple => Ok(builtin_classes(_py).tuple),
        PickleGlobal::Dict => Ok(builtin_classes(_py).dict),
    }
}

fn pickle_call_with_args(_py: &crate::PyToken<'_>, callable_bits: u64, args: &[u64]) -> u64 {
    match args.len() {
        0 => unsafe { call_callable0(_py, callable_bits) },
        1 => unsafe { call_callable1(_py, callable_bits, args[0]) },
        2 => unsafe { call_callable2(_py, callable_bits, args[0], args[1]) },
        3 => unsafe { call_callable3(_py, callable_bits, args[0], args[1], args[2]) },
        _ => {
            let builder_bits = crate::molt_callargs_new(args.len() as u64, 0);
            for &arg_bits in args {
                let _ = unsafe { crate::molt_callargs_push_pos(builder_bits, arg_bits) };
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
            }
            crate::molt_call_bind(callable_bits, builder_bits)
        }
    }
}

fn pickle_encode_text(_py: &crate::PyToken<'_>, text: &str, encoding: &str) -> Result<u64, u64> {
    let normalized = encoding.to_ascii_lowercase();
    let bytes: Vec<u8> = match normalized.as_str() {
        "utf-8" | "utf8" => text.as_bytes().to_vec(),
        "latin1" | "latin-1" => {
            let mut out: Vec<u8> = Vec::with_capacity(text.chars().count());
            for ch in text.chars() {
                let code = ch as u32;
                if code > 0xff {
                    return Err(pickle_raise(
                        _py,
                        "pickle.loads: latin1 encoding failed for _codecs.encode payload",
                    ));
                }
                out.push(code as u8);
            }
            out
        }
        "ascii" => {
            let mut out: Vec<u8> = Vec::with_capacity(text.chars().count());
            for ch in text.chars() {
                let code = ch as u32;
                if code > 0x7f {
                    return Err(pickle_raise(
                        _py,
                        "pickle.loads: ascii encoding failed for _codecs.encode payload",
                    ));
                }
                out.push(code as u8);
            }
            out
        }
        _ => {
            let message = format!(
                "pickle.loads: unsupported encoding {:?} for _codecs.encode",
                encoding
            );
            return Err(pickle_raise(_py, &message));
        }
    };
    let out_ptr = crate::alloc_bytes(_py, &bytes);
    if out_ptr.is_null() {
        Err(MoltObject::none().bits())
    } else {
        Ok(MoltObject::from_ptr(out_ptr).bits())
    }
}

mod protocol01;
pub use protocol01::{
    molt_pickle_dumps_protocol01, molt_pickle_encode_protocol0, molt_pickle_loads_protocol01,
};
mod binary;
pub(crate) use binary::pickle_resolve_global_bits;
pub use binary::{
    molt_multiprocessing_codec_dumps, molt_multiprocessing_codec_loads, molt_pickle_dumps_core,
    molt_pickle_loads_core,
};
