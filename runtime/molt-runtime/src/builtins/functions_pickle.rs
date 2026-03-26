use molt_obj_model::MoltObject;
use std::collections::HashMap;

use crate::{
    TYPE_ID_BYTES, TYPE_ID_DICT, TYPE_ID_LIST, TYPE_ID_STRING, TYPE_ID_TUPLE,
    alloc_bytes, alloc_dict_with_pairs, alloc_list_with_capacity, alloc_string, alloc_tuple,
    attr_name_bits_from_bytes, builtin_classes, bytes_like_slice,
    call_callable0, call_callable1, call_callable2, call_callable3,
    clear_exception, dec_ref_bits, dict_get_in_place, exception_pending,
    format_obj, inc_ref_bits, is_truthy, missing_bits,
    molt_getattr_builtin, molt_is_callable, molt_iter,
    obj_from_bits, object_class_bits, object_type_id,
    raise_exception, seq_vec_ref, string_obj_to_owned, to_i64, type_name, type_of_bits,
};

#[unsafe(no_mangle)]
pub extern "C" fn molt_pickle_encode_protocol0(parts_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let parts_obj = obj_from_bits(parts_bits);
        let Some(parts_ptr) = parts_obj.as_ptr() else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "pickle opcode chunks must be a sequence",
            );
        };
        let parts_type = unsafe { object_type_id(parts_ptr) };
        if parts_type != TYPE_ID_LIST && parts_type != TYPE_ID_TUPLE {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "pickle opcode chunks must be a sequence",
            );
        }
        let elems = unsafe { seq_vec_ref(parts_ptr) };
        let mut joined = String::new();
        for &elem_bits in elems.iter() {
            let Some(chunk) = string_obj_to_owned(obj_from_bits(elem_bits)) else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "pickle opcode chunks must contain str values",
                );
            };
            joined.push_str(&chunk);
        }
        let bytes_ptr = crate::alloc_bytes(_py, joined.as_bytes());
        if bytes_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(bytes_ptr).bits()
        }
    })
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

#[derive(Clone, Debug)]
enum PickleStackItem {
    Value(u64),
    Mark,
    Global(PickleGlobal),
}

fn pickle_raise(_py: &crate::PyToken<'_>, message: &str) -> u64 {
    raise_exception::<u64>(_py, "RuntimeError", message)
}

fn pickle_dump_global(out: &mut String, module: &str, name: &str) {
    out.push('c');
    out.push_str(module);
    out.push('\n');
    out.push_str(name);
    out.push('\n');
}

fn pickle_decode_latin1(input: &[u8]) -> String {
    input.iter().map(|&b| char::from(b)).collect()
}

fn pickle_string_repr(_py: &crate::PyToken<'_>, value: &str) -> Result<String, u64> {
    let Some(value_bits) = alloc_string_bits(_py, value) else {
        return Err(MoltObject::none().bits());
    };
    let rendered = format_obj(_py, obj_from_bits(value_bits));
    dec_ref_bits(_py, value_bits);
    Ok(rendered)
}

fn pickle_dump_list_payload(
    _py: &crate::PyToken<'_>,
    values: &[u64],
    protocol: i64,
    out: &mut String,
) -> Result<(), u64> {
    out.push('(');
    out.push('l');
    for &item_bits in values {
        pickle_dump_obj(_py, item_bits, protocol, out)?;
        out.push('a');
    }
    Ok(())
}

fn pickle_dump_obj(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    protocol: i64,
    out: &mut String,
) -> Result<(), u64> {
    let obj = obj_from_bits(obj_bits);
    if obj.is_none() {
        out.push('N');
        return Ok(());
    }
    if let Some(value) = obj.as_bool() {
        if value {
            out.push_str("I01\n");
        } else {
            out.push_str("I00\n");
        }
        return Ok(());
    }
    if let Some(value) = obj.as_int() {
        out.push('I');
        out.push_str(value.to_string().as_str());
        out.push('\n');
        return Ok(());
    }
    if let Some(value) = obj.as_float() {
        out.push('F');
        out.push_str(value.to_string().as_str());
        out.push('\n');
        return Ok(());
    }
    let Some(ptr) = obj.as_ptr() else {
        let message = format!("pickle.dumps: unsupported type: {}", type_name(_py, obj));
        return Err(pickle_raise(_py, &message));
    };
    let type_id = unsafe { object_type_id(ptr) };
    if type_id == crate::TYPE_ID_BIGINT {
        out.push('I');
        out.push_str(format_obj(_py, obj).as_str());
        out.push('\n');
        return Ok(());
    }
    if type_id == TYPE_ID_STRING {
        out.push('S');
        out.push_str(format_obj(_py, obj).as_str());
        out.push('\n');
        return Ok(());
    }
    if type_id == crate::TYPE_ID_BYTES {
        let Some(raw) = (unsafe { bytes_like_slice(ptr) }) else {
            return Err(pickle_raise(
                _py,
                "pickle.dumps: internal error reading bytes payload",
            ));
        };
        pickle_dump_global(out, "_codecs", "encode");
        out.push('(');
        let latin1 = pickle_decode_latin1(raw);
        let latin1_repr = pickle_string_repr(_py, &latin1)?;
        out.push('S');
        out.push_str(&latin1_repr);
        out.push('\n');
        let encoding_repr = pickle_string_repr(_py, "latin1")?;
        out.push('S');
        out.push_str(&encoding_repr);
        out.push('\n');
        out.push('t');
        out.push('R');
        return Ok(());
    }
    if type_id == crate::TYPE_ID_BYTEARRAY {
        let Some(raw) = (unsafe { bytes_like_slice(ptr) }) else {
            return Err(pickle_raise(
                _py,
                "pickle.dumps: internal error reading bytearray payload",
            ));
        };
        pickle_dump_global(out, "builtins", "bytearray");
        out.push('(');
        let bytes_ptr = crate::alloc_bytes(_py, raw);
        if bytes_ptr.is_null() {
            return Err(MoltObject::none().bits());
        }
        let bytes_bits = MoltObject::from_ptr(bytes_ptr).bits();
        let dumped = pickle_dump_obj(_py, bytes_bits, protocol, out);
        dec_ref_bits(_py, bytes_bits);
        dumped?;
        out.push('t');
        out.push('R');
        return Ok(());
    }
    if type_id == TYPE_ID_TUPLE {
        out.push('(');
        for &item_bits in unsafe { seq_vec_ref(ptr) }.iter() {
            pickle_dump_obj(_py, item_bits, protocol, out)?;
        }
        out.push('t');
        return Ok(());
    }
    if type_id == TYPE_ID_LIST {
        let values = unsafe { seq_vec_ref(ptr).clone() };
        pickle_dump_list_payload(_py, values.as_slice(), protocol, out)?;
        return Ok(());
    }
    if type_id == TYPE_ID_DICT {
        out.push('(');
        out.push('d');
        let pairs = unsafe { crate::dict_order(ptr).clone() };
        let mut idx = 0usize;
        while idx + 1 < pairs.len() {
            pickle_dump_obj(_py, pairs[idx], protocol, out)?;
            pickle_dump_obj(_py, pairs[idx + 1], protocol, out)?;
            out.push('s');
            idx += 2;
        }
        return Ok(());
    }
    if type_id == crate::TYPE_ID_SET {
        pickle_dump_global(out, "builtins", "set");
        out.push('(');
        let values = unsafe { crate::set_order(ptr).clone() };
        pickle_dump_list_payload(_py, values.as_slice(), protocol, out)?;
        out.push('t');
        out.push('R');
        return Ok(());
    }
    if type_id == crate::TYPE_ID_FROZENSET {
        pickle_dump_global(out, "builtins", "frozenset");
        out.push('(');
        let values = unsafe { crate::set_order(ptr).clone() };
        pickle_dump_list_payload(_py, values.as_slice(), protocol, out)?;
        out.push('t');
        out.push('R');
        return Ok(());
    }
    if type_id == crate::TYPE_ID_SLICE {
        pickle_dump_global(out, "builtins", "slice");
        out.push('(');
        pickle_dump_obj(_py, unsafe { crate::slice_start_bits(ptr) }, protocol, out)?;
        pickle_dump_obj(_py, unsafe { crate::slice_stop_bits(ptr) }, protocol, out)?;
        pickle_dump_obj(_py, unsafe { crate::slice_step_bits(ptr) }, protocol, out)?;
        out.push('t');
        out.push('R');
        return Ok(());
    }
    let message = format!("pickle.dumps: unsupported type: {}", type_name(_py, obj));
    Err(pickle_raise(_py, &message))
}

fn pickle_read_line<'a>(
    _py: &crate::PyToken<'_>,
    text: &'a str,
    idx: &mut usize,
) -> Result<&'a str, u64> {
    let bytes = text.as_bytes();
    if *idx > bytes.len() {
        return Err(pickle_raise(_py, "pickle.loads: unexpected end of stream"));
    }
    let start = *idx;
    let Some(rel_end) = bytes[start..].iter().position(|b| *b == b'\n') else {
        return Err(pickle_raise(_py, "pickle.loads: unexpected end of stream"));
    };
    let end = start + rel_end;
    *idx = end + 1;
    Ok(&text[start..end])
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

fn pickle_parse_memo_key(_py: &crate::PyToken<'_>, text: &str) -> Result<i64, u64> {
    text.parse::<i64>()
        .map_err(|_| pickle_raise(_py, "pickle.loads: invalid memo key"))
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

fn pickle_stack_item_to_value(
    _py: &crate::PyToken<'_>,
    item: &PickleStackItem,
) -> Result<u64, u64> {
    match item {
        PickleStackItem::Value(bits) => Ok(*bits),
        PickleStackItem::Global(global) => pickle_global_callable_bits(_py, *global),
        PickleStackItem::Mark => Err(pickle_raise(_py, "pickle.loads: mark not found")),
    }
}

fn pickle_pop_mark_items(
    _py: &crate::PyToken<'_>,
    stack: &mut Vec<PickleStackItem>,
) -> Result<Vec<PickleStackItem>, u64> {
    let mut out: Vec<PickleStackItem> = Vec::new();
    while let Some(item) = stack.pop() {
        if matches!(item, PickleStackItem::Mark) {
            out.reverse();
            return Ok(out);
        }
        out.push(item);
    }
    Err(pickle_raise(_py, "pickle.loads: mark not found"))
}

fn pickle_items_to_value_bits(
    _py: &crate::PyToken<'_>,
    items: &[PickleStackItem],
) -> Result<Vec<u64>, u64> {
    let mut out: Vec<u64> = Vec::with_capacity(items.len());
    for item in items {
        out.push(pickle_stack_item_to_value(_py, item)?);
    }
    Ok(out)
}

fn pickle_pop_stack_item(
    _py: &crate::PyToken<'_>,
    stack: &mut Vec<PickleStackItem>,
    message: &'static str,
) -> Result<PickleStackItem, u64> {
    stack.pop().ok_or_else(|| pickle_raise(_py, message))
}

fn pickle_pop_value(
    _py: &crate::PyToken<'_>,
    stack: &mut Vec<PickleStackItem>,
    message: &'static str,
) -> Result<u64, u64> {
    let item = pickle_pop_stack_item(_py, stack, message)?;
    pickle_stack_item_to_value(_py, &item)
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

fn pickle_apply_reduce(
    _py: &crate::PyToken<'_>,
    func_item: PickleStackItem,
    args_bits: u64,
) -> Result<u64, u64> {
    let Some(args_ptr) = obj_from_bits(args_bits).as_ptr() else {
        return Err(pickle_raise(_py, "pickle.loads: reduce args must be tuple"));
    };
    if unsafe { object_type_id(args_ptr) } != TYPE_ID_TUPLE {
        return Err(pickle_raise(_py, "pickle.loads: reduce args must be tuple"));
    }
    let args: Vec<u64> = unsafe { seq_vec_ref(args_ptr).to_vec() };
    match func_item {
        PickleStackItem::Mark => Err(pickle_raise(_py, "pickle.loads: mark cannot be called")),
        PickleStackItem::Global(PickleGlobal::CodecsEncode) => {
            if args.is_empty() || args.len() > 2 {
                return Err(pickle_raise(
                    _py,
                    "pickle.loads: _codecs.encode expects 1 or 2 arguments",
                ));
            }
            let Some(text) = string_obj_to_owned(obj_from_bits(args[0])) else {
                return Err(raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "pickle.loads: _codecs.encode text must be str",
                ));
            };
            let encoding = if args.len() == 2 {
                let Some(name) = string_obj_to_owned(obj_from_bits(args[1])) else {
                    return Err(raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "pickle.loads: _codecs.encode encoding must be str",
                    ));
                };
                name
            } else {
                "utf-8".to_string()
            };
            pickle_encode_text(_py, &text, &encoding)
        }
        PickleStackItem::Global(global) => {
            let callable_bits = pickle_global_callable_bits(_py, global)?;
            let out_bits = pickle_call_with_args(_py, callable_bits, args.as_slice());
            if exception_pending(_py) {
                Err(MoltObject::none().bits())
            } else {
                Ok(out_bits)
            }
        }
        PickleStackItem::Value(callable_bits) => {
            let out_bits = pickle_call_with_args(_py, callable_bits, args.as_slice());
            if exception_pending(_py) {
                Err(MoltObject::none().bits())
            } else {
                Ok(out_bits)
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_pickle_dumps_protocol01(obj_bits: u64, protocol_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(protocol) = to_i64(obj_from_bits(protocol_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pickle protocol must be int");
        };
        if protocol != 0 && protocol != 1 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "only pickle protocols 0 and 1 are supported",
            );
        }
        let mut out = String::new();
        if let Err(err_bits) = pickle_dump_obj(_py, obj_bits, protocol, &mut out) {
            return err_bits;
        }
        out.push('.');
        let out_ptr = crate::alloc_bytes(_py, out.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_pickle_loads_protocol01(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(data_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pickle data must be str");
        };
        let bytes = text.as_bytes();
        let mut idx: usize = 0;
        let mut stack: Vec<PickleStackItem> = Vec::new();
        let mut memo: HashMap<i64, PickleStackItem> = HashMap::new();
        while idx < bytes.len() {
            let op = bytes[idx] as char;
            idx += 1;
            match op {
                '.' => break,
                'N' => stack.push(PickleStackItem::Value(MoltObject::none().bits())),
                'I' => {
                    let line = match pickle_read_line(_py, &text, &mut idx) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    if line == "01" {
                        stack.push(PickleStackItem::Value(MoltObject::from_bool(true).bits()));
                    } else if line == "00" {
                        stack.push(PickleStackItem::Value(MoltObject::from_bool(false).bits()));
                    } else {
                        let int_bits = match pickle_parse_int_bits(_py, line) {
                            Ok(value) => value,
                            Err(err_bits) => return err_bits,
                        };
                        stack.push(PickleStackItem::Value(int_bits));
                    }
                }
                'L' => {
                    let line = match pickle_read_line(_py, &text, &mut idx) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let int_bits = match pickle_parse_long_line_bits(_py, line) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    stack.push(PickleStackItem::Value(int_bits));
                }
                'F' => {
                    let line = match pickle_read_line(_py, &text, &mut idx) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let float_bits = match pickle_parse_float_bits(_py, line) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    stack.push(PickleStackItem::Value(float_bits));
                }
                'S' => {
                    let line = match pickle_read_line(_py, &text, &mut idx) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let parsed = match pickle_parse_string_literal(line) {
                        Ok(value) => value,
                        Err(message) => return pickle_raise(_py, message),
                    };
                    let out_ptr = alloc_string(_py, parsed.as_bytes());
                    if out_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleStackItem::Value(MoltObject::from_ptr(out_ptr).bits()));
                }
                'V' => {
                    let line = match pickle_read_line(_py, &text, &mut idx) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let out_ptr = alloc_string(_py, line.as_bytes());
                    if out_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleStackItem::Value(MoltObject::from_ptr(out_ptr).bits()));
                }
                '(' => stack.push(PickleStackItem::Mark),
                't' => {
                    let items = match pickle_pop_mark_items(_py, &mut stack) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let values = match pickle_items_to_value_bits(_py, items.as_slice()) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let tuple_ptr = alloc_tuple(_py, values.as_slice());
                    if tuple_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleStackItem::Value(
                        MoltObject::from_ptr(tuple_ptr).bits(),
                    ));
                }
                'l' => {
                    let items = match pickle_pop_mark_items(_py, &mut stack) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let values = match pickle_items_to_value_bits(_py, items.as_slice()) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let list_ptr = alloc_list_with_capacity(_py, values.as_slice(), values.len());
                    if list_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleStackItem::Value(
                        MoltObject::from_ptr(list_ptr).bits(),
                    ));
                }
                'd' => {
                    let items = match pickle_pop_mark_items(_py, &mut stack) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let values = match pickle_items_to_value_bits(_py, items.as_slice()) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    if values.len() % 2 != 0 {
                        return pickle_raise(_py, "pickle.loads: dict has odd number of items");
                    }
                    let dict_ptr = alloc_dict_with_pairs(_py, values.as_slice());
                    if dict_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleStackItem::Value(
                        MoltObject::from_ptr(dict_ptr).bits(),
                    ));
                }
                'a' => {
                    let item_bits =
                        match pickle_pop_value(_py, &mut stack, "pickle.loads: stack underflow") {
                            Ok(value) => value,
                            Err(err_bits) => return err_bits,
                        };
                    let target_bits =
                        match pickle_pop_value(_py, &mut stack, "pickle.loads: stack underflow") {
                            Ok(value) => value,
                            Err(err_bits) => return err_bits,
                        };
                    let Some(target_ptr) = obj_from_bits(target_bits).as_ptr() else {
                        return pickle_raise(_py, "pickle.loads: append target is not list");
                    };
                    if unsafe { object_type_id(target_ptr) } != TYPE_ID_LIST {
                        return pickle_raise(_py, "pickle.loads: append target is not list");
                    }
                    let _ = crate::molt_list_append(target_bits, item_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleStackItem::Value(target_bits));
                }
                's' => {
                    let value_bits =
                        match pickle_pop_value(_py, &mut stack, "pickle.loads: stack underflow") {
                            Ok(value) => value,
                            Err(err_bits) => return err_bits,
                        };
                    let key_bits =
                        match pickle_pop_value(_py, &mut stack, "pickle.loads: stack underflow") {
                            Ok(value) => value,
                            Err(err_bits) => return err_bits,
                        };
                    let target_bits =
                        match pickle_pop_value(_py, &mut stack, "pickle.loads: stack underflow") {
                            Ok(value) => value,
                            Err(err_bits) => return err_bits,
                        };
                    let Some(target_ptr) = obj_from_bits(target_bits).as_ptr() else {
                        return pickle_raise(_py, "pickle.loads: setitem target is not dict");
                    };
                    if unsafe { object_type_id(target_ptr) } != TYPE_ID_DICT {
                        return pickle_raise(_py, "pickle.loads: setitem target is not dict");
                    }
                    unsafe {
                        crate::dict_set_in_place(_py, target_ptr, key_bits, value_bits);
                    }
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleStackItem::Value(target_bits));
                }
                'c' => {
                    let module = match pickle_read_line(_py, &text, &mut idx) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let name = match pickle_read_line(_py, &text, &mut idx) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let Some(global) = pickle_resolve_global(module, name) else {
                        let message =
                            format!("pickle.loads: unsupported global {}.{}", module, name);
                        return pickle_raise(_py, &message);
                    };
                    stack.push(PickleStackItem::Global(global));
                }
                'R' => {
                    let args_item = match pickle_pop_stack_item(
                        _py,
                        &mut stack,
                        "pickle.loads: stack underflow",
                    ) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let func_item = match pickle_pop_stack_item(
                        _py,
                        &mut stack,
                        "pickle.loads: stack underflow",
                    ) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let args_bits = match pickle_stack_item_to_value(_py, &args_item) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let out_bits = match pickle_apply_reduce(_py, func_item, args_bits) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    stack.push(PickleStackItem::Value(out_bits));
                }
                'p' => {
                    let line = match pickle_read_line(_py, &text, &mut idx) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let key = match pickle_parse_memo_key(_py, line) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let item = match pickle_pop_stack_item(
                        _py,
                        &mut stack,
                        "pickle.loads: stack underflow",
                    ) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    memo.insert(key, item.clone());
                    stack.push(item);
                }
                'g' => {
                    let line = match pickle_read_line(_py, &text, &mut idx) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let key = match pickle_parse_memo_key(_py, line) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let Some(item) = memo.get(&key).cloned() else {
                        let message = format!("pickle.loads: memo key {} missing", key);
                        return pickle_raise(_py, &message);
                    };
                    stack.push(item);
                }
                _ => {
                    let message = format!("pickle.loads: unsupported opcode {:?}", op);
                    return pickle_raise(_py, &message);
                }
            }
        }
        let Some(item) = stack.last() else {
            return pickle_raise(_py, "pickle.loads: pickle stack empty");
        };
        match pickle_stack_item_to_value(_py, item) {
            Ok(value) => value,
            Err(err_bits) => err_bits,
        }
    })
}

const PICKLE_PROTO_3: i64 = 3;
const PICKLE_PROTO_4: i64 = 4;
const PICKLE_PROTO_5: i64 = 5;

const PICKLE_OP_PROTO: u8 = 0x80;
const PICKLE_OP_STOP: u8 = b'.';
const PICKLE_OP_POP: u8 = b'0';
const PICKLE_OP_POP_MARK: u8 = b'1';
const PICKLE_OP_MARK: u8 = b'(';
const PICKLE_OP_NONE: u8 = b'N';
const PICKLE_OP_NEWTRUE: u8 = 0x88;
const PICKLE_OP_NEWFALSE: u8 = 0x89;
const PICKLE_OP_INT: u8 = b'I';
const PICKLE_OP_LONG: u8 = b'L';
const PICKLE_OP_BININT: u8 = b'J';
const PICKLE_OP_BININT1: u8 = b'K';
const PICKLE_OP_BININT2: u8 = b'M';
const PICKLE_OP_LONG1: u8 = 0x8a;
const PICKLE_OP_LONG4: u8 = 0x8b;
const PICKLE_OP_FLOAT: u8 = b'F';
const PICKLE_OP_BINFLOAT: u8 = b'G';
const PICKLE_OP_STRING: u8 = b'S';
const PICKLE_OP_BINUNICODE: u8 = b'X';
const PICKLE_OP_SHORT_BINUNICODE: u8 = 0x8c;
const PICKLE_OP_UNICODE: u8 = b'V';
const PICKLE_OP_BINBYTES: u8 = b'B';
const PICKLE_OP_SHORT_BINBYTES: u8 = b'C';
const PICKLE_OP_BINBYTES8: u8 = 0x8e;
const PICKLE_OP_BYTEARRAY8: u8 = 0x96;
const PICKLE_OP_EMPTY_TUPLE: u8 = b')';
const PICKLE_OP_TUPLE: u8 = b't';
const PICKLE_OP_TUPLE1: u8 = 0x85;
const PICKLE_OP_TUPLE2: u8 = 0x86;
const PICKLE_OP_TUPLE3: u8 = 0x87;
const PICKLE_OP_EMPTY_LIST: u8 = b']';
const PICKLE_OP_LIST: u8 = b'l';
const PICKLE_OP_APPEND: u8 = b'a';
const PICKLE_OP_APPENDS: u8 = b'e';
const PICKLE_OP_EMPTY_DICT: u8 = b'}';
const PICKLE_OP_DICT: u8 = b'd';
const PICKLE_OP_SETITEM: u8 = b's';
const PICKLE_OP_SETITEMS: u8 = b'u';
const PICKLE_OP_EMPTY_SET: u8 = 0x8f;
const PICKLE_OP_ADDITEMS: u8 = 0x90;
const PICKLE_OP_FROZENSET: u8 = 0x91;
const PICKLE_OP_GLOBAL: u8 = b'c';
const PICKLE_OP_STACK_GLOBAL: u8 = 0x93;
const PICKLE_OP_REDUCE: u8 = b'R';
const PICKLE_OP_BUILD: u8 = b'b';
const PICKLE_OP_NEWOBJ: u8 = 0x81;
const PICKLE_OP_NEWOBJ_EX: u8 = 0x92;
const PICKLE_OP_PUT: u8 = b'p';
const PICKLE_OP_BINPUT: u8 = b'q';
const PICKLE_OP_LONG_BINPUT: u8 = b'r';
const PICKLE_OP_GET: u8 = b'g';
const PICKLE_OP_BINGET: u8 = b'h';
const PICKLE_OP_LONG_BINGET: u8 = b'j';
const PICKLE_OP_MEMOIZE: u8 = 0x94;
const PICKLE_OP_PERSID: u8 = b'P';
const PICKLE_OP_BINPERSID: u8 = b'Q';
const PICKLE_OP_EXT1: u8 = 0x82;
const PICKLE_OP_EXT2: u8 = 0x83;
const PICKLE_OP_EXT4: u8 = 0x84;
const PICKLE_OP_FRAME: u8 = 0x95;
const PICKLE_OP_NEXT_BUFFER: u8 = 0x97;
const PICKLE_OP_READONLY_BUFFER: u8 = 0x98;

const PICKLE_RECURSION_LIMIT: usize = 1_000;

#[derive(Clone, Debug)]
enum PickleVmItem {
    Value(u64),
    Global(PickleGlobal),
    Mark,
}

struct PickleDumpState {
    protocol: i64,
    out: Vec<u8>,
    memo: HashMap<u64, u32>,
    next_memo: u32,
    depth: usize,
    persistent_id_bits: Option<u64>,
    buffer_callback_bits: Option<u64>,
    dispatch_table_bits: Option<u64>,
}

impl PickleDumpState {
    fn new(
        protocol: i64,
        persistent_id_bits: Option<u64>,
        buffer_callback_bits: Option<u64>,
        dispatch_table_bits: Option<u64>,
    ) -> Self {
        Self {
            protocol,
            out: Vec::with_capacity(256),
            memo: HashMap::new(),
            next_memo: 0,
            depth: 0,
            persistent_id_bits,
            buffer_callback_bits,
            dispatch_table_bits,
        }
    }

    fn push(&mut self, op: u8) {
        self.out.push(op);
    }

    fn extend(&mut self, bytes: &[u8]) {
        self.out.extend_from_slice(bytes);
    }
}

fn pickle_option_callable_bits(
    _py: &crate::PyToken<'_>,
    maybe_bits: u64,
    name: &str,
) -> Result<Option<u64>, u64> {
    if obj_from_bits(maybe_bits).is_none() {
        return Ok(None);
    }
    if !is_truthy(_py, obj_from_bits(molt_is_callable(maybe_bits))) {
        let message = format!("pickle {name} must be callable");
        return Err(raise_exception::<u64>(_py, "TypeError", &message));
    }
    Ok(Some(maybe_bits))
}

fn pickle_input_to_bytes(_py: &crate::PyToken<'_>, data_bits: u64) -> Result<Vec<u8>, u64> {
    if let Some(ptr) = obj_from_bits(data_bits).as_ptr()
        && let Some(raw) = unsafe { bytes_like_slice(ptr) }
    {
        return Ok(raw.to_vec());
    }
    if let Some(text) = string_obj_to_owned(obj_from_bits(data_bits)) {
        return Ok(text.into_bytes());
    }
    Err(raise_exception::<u64>(
        _py,
        "TypeError",
        "pickle data must be bytes, bytearray, or str",
    ))
}

fn pickle_read_u8(data: &[u8], idx: &mut usize, _py: &crate::PyToken<'_>) -> Result<u8, u64> {
    if *idx >= data.len() {
        return Err(pickle_raise(_py, "pickle.loads: unexpected end of stream"));
    }
    let byte = data[*idx];
    *idx += 1;
    Ok(byte)
}

fn pickle_read_exact<'a>(
    data: &'a [u8],
    idx: &mut usize,
    n: usize,
    _py: &crate::PyToken<'_>,
) -> Result<&'a [u8], u64> {
    if data.len().saturating_sub(*idx) < n {
        return Err(pickle_raise(_py, "pickle.loads: unexpected end of stream"));
    }
    let start = *idx;
    let end = start + n;
    *idx = end;
    Ok(&data[start..end])
}

fn pickle_read_line_bytes<'a>(
    data: &'a [u8],
    idx: &mut usize,
    _py: &crate::PyToken<'_>,
) -> Result<&'a [u8], u64> {
    if *idx > data.len() {
        return Err(pickle_raise(_py, "pickle.loads: unexpected end of stream"));
    }
    let start = *idx;
    let Some(rel_end) = data[start..].iter().position(|b| *b == b'\n') else {
        return Err(pickle_raise(_py, "pickle.loads: unexpected end of stream"));
    };
    let end = start + rel_end;
    *idx = end + 1;
    Ok(&data[start..end])
}

fn pickle_read_u16_le(data: &[u8], idx: &mut usize, _py: &crate::PyToken<'_>) -> Result<u16, u64> {
    let raw = pickle_read_exact(data, idx, 2, _py)?;
    Ok(u16::from_le_bytes([raw[0], raw[1]]))
}

fn pickle_read_u32_le(data: &[u8], idx: &mut usize, _py: &crate::PyToken<'_>) -> Result<u32, u64> {
    let raw = pickle_read_exact(data, idx, 4, _py)?;
    Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

fn pickle_read_u64_le(data: &[u8], idx: &mut usize, _py: &crate::PyToken<'_>) -> Result<u64, u64> {
    let raw = pickle_read_exact(data, idx, 8, _py)?;
    Ok(u64::from_le_bytes([
        raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
    ]))
}

fn pickle_parse_long_bytes_bits(_py: &crate::PyToken<'_>, raw: &[u8]) -> Result<u64, u64> {
    if raw.is_empty() {
        return Ok(MoltObject::from_int(0).bits());
    }
    if raw.len() > 8 {
        return Err(pickle_raise(
            _py,
            "pickle.loads: LONG payload exceeds Molt int range",
        ));
    }
    let negative = (raw[raw.len() - 1] & 0x80) != 0;
    let mut bytes = if negative { [0xff; 8] } else { [0u8; 8] };
    bytes[..raw.len()].copy_from_slice(raw);
    Ok(MoltObject::from_int(i64::from_le_bytes(bytes)).bits())
}

fn pickle_read_f64_be(data: &[u8], idx: &mut usize, _py: &crate::PyToken<'_>) -> Result<f64, u64> {
    let raw = pickle_read_exact(data, idx, 8, _py)?;
    Ok(f64::from_bits(u64::from_be_bytes([
        raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
    ])))
}

fn pickle_decode_utf8(_py: &crate::PyToken<'_>, raw: &[u8], ctx: &str) -> Result<String, u64> {
    String::from_utf8(raw.to_vec()).map_err(|_| {
        let msg = format!("pickle.loads: invalid UTF-8 while decoding {ctx}");
        pickle_raise(_py, &msg)
    })
}

fn pickle_attr_optional(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    name: &[u8],
) -> Result<Option<u64>, u64> {
    urllib_request_attr_optional(_py, obj_bits, name)
}

fn pickle_attr_required(_py: &crate::PyToken<'_>, obj_bits: u64, name: &[u8]) -> Result<u64, u64> {
    match pickle_attr_optional(_py, obj_bits, name)? {
        Some(bits) => Ok(bits),
        None => {
            let name_text = std::str::from_utf8(name).unwrap_or("attribute");
            let msg = format!("pickle: missing required attribute {name_text}");
            Err(pickle_raise(_py, &msg))
        }
    }
}

fn pickle_emit_u32_le(state: &mut PickleDumpState, value: u32) {
    state.extend(&value.to_le_bytes());
}

fn pickle_emit_u64_le(state: &mut PickleDumpState, value: u64) {
    state.extend(&value.to_le_bytes());
}

fn pickle_emit_memo_put(state: &mut PickleDumpState, index: u32) {
    if state.protocol >= PICKLE_PROTO_4 {
        state.push(PICKLE_OP_MEMOIZE);
        return;
    }
    if index <= u8::MAX as u32 {
        state.push(PICKLE_OP_BINPUT);
        state.push(index as u8);
    } else {
        state.push(PICKLE_OP_LONG_BINPUT);
        pickle_emit_u32_le(state, index);
    }
}

fn pickle_emit_memo_get(state: &mut PickleDumpState, index: u32) {
    if index <= u8::MAX as u32 {
        state.push(PICKLE_OP_BINGET);
        state.push(index as u8);
    } else {
        state.push(PICKLE_OP_LONG_BINGET);
        pickle_emit_u32_le(state, index);
    }
}

fn pickle_memo_key(bits: u64) -> Option<u64> {
    let obj = obj_from_bits(bits);
    if obj.as_ptr().is_some() {
        Some(bits)
    } else {
        None
    }
}

fn pickle_memo_lookup(state: &PickleDumpState, bits: u64) -> Option<u32> {
    let key = pickle_memo_key(bits)?;
    state.memo.get(&key).copied()
}

fn pickle_memo_store(state: &mut PickleDumpState, bits: u64) -> Option<u32> {
    let key = pickle_memo_key(bits)?;
    if let Some(found) = state.memo.get(&key).copied() {
        return Some(found);
    }
    let index = state.next_memo;
    state.next_memo = state.next_memo.saturating_add(1);
    state.memo.insert(key, index);
    pickle_emit_memo_put(state, index);
    Some(index)
}

fn pickle_memo_store_if_absent(state: &mut PickleDumpState, bits: u64) -> Option<u32> {
    if let Some(found) = pickle_memo_lookup(state, bits) {
        return Some(found);
    }
    pickle_memo_store(state, bits)
}

fn pickle_emit_proto_header(state: &mut PickleDumpState) {
    state.push(PICKLE_OP_PROTO);
    state.push(state.protocol as u8);
}

fn pickle_emit_global_opcode(state: &mut PickleDumpState, module: &str, name: &str) {
    state.push(PICKLE_OP_GLOBAL);
    state.extend(module.as_bytes());
    state.push(b'\n');
    state.extend(name.as_bytes());
    state.push(b'\n');
}

fn pickle_lookup_extension_code(
    _py: &crate::PyToken<'_>,
    module: &str,
    name: &str,
) -> Result<Option<i64>, u64> {
    let registry_bits = pickle_resolve_global_bits(_py, "copyreg", "_extension_registry")?;
    let Some(registry_ptr) = obj_from_bits(registry_bits).as_ptr() else {
        dec_ref_bits(_py, registry_bits);
        return Ok(None);
    };
    if unsafe { object_type_id(registry_ptr) } != TYPE_ID_DICT {
        dec_ref_bits(_py, registry_bits);
        return Ok(None);
    }
    let Some(module_bits) = alloc_string_bits(_py, module) else {
        dec_ref_bits(_py, registry_bits);
        return Err(MoltObject::none().bits());
    };
    let Some(name_bits) = alloc_string_bits(_py, name) else {
        dec_ref_bits(_py, module_bits);
        dec_ref_bits(_py, registry_bits);
        return Err(MoltObject::none().bits());
    };
    let key_ptr = alloc_tuple(_py, &[module_bits, name_bits]);
    dec_ref_bits(_py, module_bits);
    dec_ref_bits(_py, name_bits);
    let Some(key_ptr) = (!key_ptr.is_null()).then_some(key_ptr) else {
        dec_ref_bits(_py, registry_bits);
        return Err(MoltObject::none().bits());
    };
    let key_bits = MoltObject::from_ptr(key_ptr).bits();
    let code_bits = unsafe { dict_get_in_place(_py, registry_ptr, key_bits) };
    dec_ref_bits(_py, key_bits);
    if exception_pending(_py) {
        dec_ref_bits(_py, registry_bits);
        return Err(MoltObject::none().bits());
    }
    let Some(code_bits) = code_bits else {
        dec_ref_bits(_py, registry_bits);
        return Ok(None);
    };
    let Some(code) = to_i64(obj_from_bits(code_bits)) else {
        dec_ref_bits(_py, registry_bits);
        return Ok(None);
    };
    dec_ref_bits(_py, registry_bits);
    if code <= 0 {
        return Ok(None);
    }
    Ok(Some(code))
}

fn pickle_emit_global_ref(
    _py: &crate::PyToken<'_>,
    state: &mut PickleDumpState,
    obj_bits: u64,
) -> Result<bool, u64> {
    let Some(module_bits) = pickle_attr_optional(_py, obj_bits, b"__module__")? else {
        return Ok(false);
    };
    let Some(name_bits) = pickle_attr_optional(_py, obj_bits, b"__name__")? else {
        dec_ref_bits(_py, module_bits);
        return Ok(false);
    };
    let Some(module_name) = string_obj_to_owned(obj_from_bits(module_bits)) else {
        dec_ref_bits(_py, module_bits);
        dec_ref_bits(_py, name_bits);
        return Ok(false);
    };
    let Some(attr_name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
        dec_ref_bits(_py, module_bits);
        dec_ref_bits(_py, name_bits);
        return Ok(false);
    };
    dec_ref_bits(_py, module_bits);
    dec_ref_bits(_py, name_bits);
    if state.protocol >= 2
        && let Some(code) = pickle_lookup_extension_code(_py, &module_name, &attr_name)?
    {
        if code <= u8::MAX as i64 {
            state.push(PICKLE_OP_EXT1);
            state.push(code as u8);
            return Ok(true);
        }
        if code <= u16::MAX as i64 {
            state.push(PICKLE_OP_EXT2);
            state.extend(&(code as u16).to_le_bytes());
            return Ok(true);
        }
        if code <= u32::MAX as i64 {
            state.push(PICKLE_OP_EXT4);
            state.extend(&(code as u32).to_le_bytes());
            return Ok(true);
        }
    }
    if state.protocol >= PICKLE_PROTO_4 {
        pickle_dump_unicode_binary(_py, state, module_name.as_str())?;
        pickle_dump_unicode_binary(_py, state, attr_name.as_str())?;
        state.push(PICKLE_OP_STACK_GLOBAL);
        return Ok(true);
    }
    pickle_emit_global_opcode(state, module_name.as_str(), attr_name.as_str());
    Ok(true)
}

fn pickle_dump_unicode_binary(
    _py: &crate::PyToken<'_>,
    state: &mut PickleDumpState,
    text: &str,
) -> Result<(), u64> {
    let raw = text.as_bytes();
    if raw.len() <= u8::MAX as usize && state.protocol >= PICKLE_PROTO_4 {
        state.push(PICKLE_OP_SHORT_BINUNICODE);
        state.push(raw.len() as u8);
        state.extend(raw);
        return Ok(());
    }
    if raw.len() <= u32::MAX as usize {
        state.push(PICKLE_OP_BINUNICODE);
        pickle_emit_u32_le(state, raw.len() as u32);
        state.extend(raw);
        return Ok(());
    }
    state.push(0x8d);
    pickle_emit_u64_le(state, raw.len() as u64);
    state.extend(raw);
    Ok(())
}

fn pickle_dump_bytes_binary(
    _py: &crate::PyToken<'_>,
    state: &mut PickleDumpState,
    raw: &[u8],
) -> Result<(), u64> {
    if raw.len() <= u8::MAX as usize {
        state.push(PICKLE_OP_SHORT_BINBYTES);
        state.push(raw.len() as u8);
        state.extend(raw);
        return Ok(());
    }
    if raw.len() <= u32::MAX as usize {
        state.push(PICKLE_OP_BINBYTES);
        pickle_emit_u32_le(state, raw.len() as u32);
        state.extend(raw);
        return Ok(());
    }
    state.push(PICKLE_OP_BINBYTES8);
    pickle_emit_u64_le(state, raw.len() as u64);
    state.extend(raw);
    Ok(())
}

fn pickle_dump_bytearray_binary(
    _py: &crate::PyToken<'_>,
    state: &mut PickleDumpState,
    raw: &[u8],
) -> Result<(), u64> {
    if state.protocol >= PICKLE_PROTO_5 {
        state.push(PICKLE_OP_BYTEARRAY8);
        pickle_emit_u64_le(state, raw.len() as u64);
        state.extend(raw);
        return Ok(());
    }
    // Protocols 2-4: bytearray(bytes(...)) reduce path.
    pickle_emit_global_opcode(state, "builtins", "bytearray");
    let bytes_ptr = crate::alloc_bytes(_py, raw);
    if bytes_ptr.is_null() {
        return Err(MoltObject::none().bits());
    }
    let bytes_bits = MoltObject::from_ptr(bytes_ptr).bits();
    let dumped = pickle_dump_obj_binary(_py, state, bytes_bits, true);
    dec_ref_bits(_py, bytes_bits);
    dumped?;
    state.push(PICKLE_OP_TUPLE1);
    state.push(PICKLE_OP_REDUCE);
    Ok(())
}

fn pickle_long_bytes_from_i64(value: i64) -> Vec<u8> {
    let mut raw = value.to_le_bytes().to_vec();
    while raw.len() > 1 {
        let last = raw[raw.len() - 1];
        let prev = raw[raw.len() - 2];
        let drop_zero = last == 0x00 && (prev & 0x80) == 0;
        let drop_ff = last == 0xff && (prev & 0x80) != 0;
        if drop_zero || drop_ff {
            raw.pop();
        } else {
            break;
        }
    }
    raw
}

fn pickle_dump_int_binary(state: &mut PickleDumpState, value: i64) {
    if (0..=u8::MAX as i64).contains(&value) {
        state.push(PICKLE_OP_BININT1);
        state.push(value as u8);
        return;
    }
    if (0..=u16::MAX as i64).contains(&value) {
        state.push(PICKLE_OP_BININT2);
        state.extend(&(value as u16).to_le_bytes());
        return;
    }
    if (i32::MIN as i64..=i32::MAX as i64).contains(&value) {
        state.push(PICKLE_OP_BININT);
        state.extend(&(value as i32).to_le_bytes());
        return;
    }
    let raw = pickle_long_bytes_from_i64(value);
    if raw.len() <= u8::MAX as usize {
        state.push(PICKLE_OP_LONG1);
        state.push(raw.len() as u8);
        state.extend(raw.as_slice());
    } else {
        state.push(PICKLE_OP_LONG4);
        pickle_emit_u32_le(state, raw.len() as u32);
        state.extend(raw.as_slice());
    }
}

fn pickle_dump_float_binary(state: &mut PickleDumpState, value: f64) {
    state.push(PICKLE_OP_BINFLOAT);
    state.extend(&value.to_bits().to_be_bytes());
}

fn pickle_dump_maybe_persistent(
    _py: &crate::PyToken<'_>,
    state: &mut PickleDumpState,
    obj_bits: u64,
) -> Result<bool, u64> {
    let Some(callback_bits) = state.persistent_id_bits else {
        return Ok(false);
    };
    let pid_bits = unsafe { call_callable1(_py, callback_bits, obj_bits) };
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if obj_from_bits(pid_bits).is_none() {
        return Ok(false);
    }
    if state.protocol == 0
        && let Some(pid_text) = string_obj_to_owned(obj_from_bits(pid_bits))
    {
        state.push(PICKLE_OP_PERSID);
        state.extend(pid_text.as_bytes());
        state.push(b'\n');
        return Ok(true);
    }
    pickle_dump_obj_binary(_py, state, pid_bits, false)?;
    state.push(PICKLE_OP_BINPERSID);
    Ok(true)
}

fn pickle_buffer_value_to_bytes(
    _py: &crate::PyToken<'_>,
    value_bits: u64,
    context: &str,
) -> Result<u64, u64> {
    if let Some(ptr) = obj_from_bits(value_bits).as_ptr()
        && let Some(raw) = unsafe { bytes_like_slice(ptr) }
    {
        let out_ptr = crate::alloc_bytes(_py, raw);
        if out_ptr.is_null() {
            return Err(MoltObject::none().bits());
        }
        return Ok(MoltObject::from_ptr(out_ptr).bits());
    }
    let msg = format!("pickle.loads: {context} must provide a bytes-like payload");
    Err(pickle_raise(_py, &msg))
}

fn pickle_buffer_value_to_memoryview(
    _py: &crate::PyToken<'_>,
    value_bits: u64,
    context: &str,
) -> Result<u64, u64> {
    let view_bits = crate::molt_memoryview_new(value_bits);
    if exception_pending(_py) {
        let msg = format!("pickle.loads: {context} must provide a bytes-like payload");
        return Err(pickle_raise(_py, &msg));
    }
    Ok(view_bits)
}

fn pickle_external_buffer_to_memoryview(
    _py: &crate::PyToken<'_>,
    item_bits: u64,
) -> Result<u64, u64> {
    if let Ok(bits) = pickle_buffer_value_to_memoryview(_py, item_bits, "out-of-band buffer") {
        return Ok(bits);
    }
    if let Some(raw_method_bits) = pickle_attr_optional(_py, item_bits, b"raw")? {
        let raw_bits = unsafe { call_callable0(_py, raw_method_bits) };
        dec_ref_bits(_py, raw_method_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        return pickle_buffer_value_to_memoryview(_py, raw_bits, "out-of-band buffer");
    }
    Err(pickle_raise(
        _py,
        "pickle.loads: out-of-band buffer must be bytes-like or expose raw()",
    ))
}

fn pickle_next_external_buffer_bits(
    _py: &crate::PyToken<'_>,
    buffers_iter_bits: Option<u64>,
) -> Result<u64, u64> {
    let Some(iter_bits) = buffers_iter_bits else {
        return Err(pickle_raise(
            _py,
            "pickle.loads: NEXT_BUFFER requires buffers argument",
        ));
    };
    let (item_bits, done) = iter_next_pair(_py, iter_bits)?;
    if done {
        return Err(pickle_raise(
            _py,
            "pickle.loads: not enough out-of-band buffers",
        ));
    }
    pickle_external_buffer_to_memoryview(_py, item_bits)
}

fn pickle_dump_maybe_out_of_band_buffer(
    _py: &crate::PyToken<'_>,
    state: &mut PickleDumpState,
    obj_bits: u64,
    readonly: bool,
) -> Result<bool, u64> {
    let Some(callback_bits) = state.buffer_callback_bits else {
        return Ok(false);
    };
    if state.protocol < PICKLE_PROTO_5 {
        return Ok(false);
    }
    let callback_result_bits = unsafe { call_callable1(_py, callback_bits, obj_bits) };
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let in_band = is_truthy(_py, obj_from_bits(callback_result_bits));
    if !obj_from_bits(callback_result_bits).is_none() {
        dec_ref_bits(_py, callback_result_bits);
    }
    if in_band {
        return Ok(false);
    }
    state.push(PICKLE_OP_NEXT_BUFFER);
    if readonly {
        state.push(PICKLE_OP_READONLY_BUFFER);
    }
    // Do NOT memo out-of-band buffers — each reference must emit its own
    // NEXT_BUFFER opcode so every buffer slot is consumed during loads.
    Ok(true)
}

fn pickle_extract_picklebuffer_payload(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
) -> Result<Option<(u64, bool)>, u64> {
    let marker_bits = match pickle_attr_optional(_py, obj_bits, b"__molt_pickle_buffer__")? {
        Some(bits) => bits,
        None => return Ok(None),
    };
    let is_marker = is_truthy(_py, obj_from_bits(marker_bits));
    dec_ref_bits(_py, marker_bits);
    if !is_marker {
        return Ok(None);
    }
    let raw_method_bits = pickle_attr_required(_py, obj_bits, b"raw")?;
    let raw_bits = unsafe { call_callable0(_py, raw_method_bits) };
    dec_ref_bits(_py, raw_method_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let readonly = if let Some(raw_ptr) = obj_from_bits(raw_bits).as_ptr() {
        let raw_type = unsafe { object_type_id(raw_ptr) };
        if raw_type == crate::TYPE_ID_BYTEARRAY {
            false
        } else if raw_type == crate::TYPE_ID_MEMORYVIEW {
            unsafe { crate::memoryview_readonly(raw_ptr) }
        } else {
            true
        }
    } else {
        true
    };
    let payload_bits = pickle_buffer_value_to_bytes(_py, raw_bits, "PickleBuffer.raw() payload");
    if !obj_from_bits(raw_bits).is_none() {
        dec_ref_bits(_py, raw_bits);
    }
    payload_bits.map(|bits| Some((bits, readonly)))
}

fn pickle_dispatch_reducer_from_table(
    _py: &crate::PyToken<'_>,
    dispatch_table_bits: u64,
    obj_bits: u64,
) -> Result<Option<u64>, u64> {
    let Some(ptr) = obj_from_bits(dispatch_table_bits).as_ptr() else {
        return Ok(None);
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_DICT {
        return Ok(None);
    }
    let type_bits = type_of_bits(_py, obj_bits);
    let reducer_bits = unsafe { dict_get_in_place(_py, ptr, type_bits) };
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let Some(reducer_bits) = reducer_bits else {
        return Ok(None);
    };
    let out_bits = unsafe { call_callable1(_py, reducer_bits, obj_bits) };
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(Some(out_bits))
}

fn pickle_reduce_value(
    _py: &crate::PyToken<'_>,
    state: &mut PickleDumpState,
    obj_bits: u64,
) -> Result<Option<u64>, u64> {
    if let Some(dispatch_bits) = state.dispatch_table_bits
        && let Some(reduced) = pickle_dispatch_reducer_from_table(_py, dispatch_bits, obj_bits)?
    {
        return Ok(Some(reduced));
    }
    if let Some(reduce_ex_bits) = pickle_attr_optional(_py, obj_bits, b"__reduce_ex__")? {
        let out_bits = unsafe {
            call_callable1(
                _py,
                reduce_ex_bits,
                MoltObject::from_int(state.protocol).bits(),
            )
        };
        dec_ref_bits(_py, reduce_ex_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        return Ok(Some(out_bits));
    }
    if let Some(reduce_bits) = pickle_attr_optional(_py, obj_bits, b"__reduce__")? {
        let out_bits = unsafe { call_callable0(_py, reduce_bits) };
        dec_ref_bits(_py, reduce_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        return Ok(Some(out_bits));
    }
    Ok(None)
}

fn pickle_dump_items_from_iterable(
    _py: &crate::PyToken<'_>,
    state: &mut PickleDumpState,
    values_bits: u64,
    dict_items: bool,
    iterator_error_prefix: &str,
) -> Result<(), u64> {
    let iter_bits = molt_iter(values_bits);
    if exception_pending(_py) {
        clear_exception(_py);
        let value_type = type_name(_py, obj_from_bits(values_bits));
        let msg = format!("{iterator_error_prefix}{value_type}");
        return Err(pickle_raise(_py, &msg));
    }
    state.push(PICKLE_OP_MARK);
    loop {
        let (item_bits, done) = iter_next_pair(_py, iter_bits)?;
        if done {
            break;
        }
        if dict_items {
            let Some(item_ptr) = obj_from_bits(item_bits).as_ptr() else {
                return Err(raise_exception(
                    _py,
                    "TypeError",
                    "dict items iterator must return 2-tuples",
                ));
            };
            if unsafe { object_type_id(item_ptr) } != TYPE_ID_TUPLE {
                return Err(raise_exception(
                    _py,
                    "TypeError",
                    "dict items iterator must return 2-tuples",
                ));
            }
            let fields = unsafe { seq_vec_ref(item_ptr) };
            if fields.len() != 2 {
                return Err(raise_exception(
                    _py,
                    "TypeError",
                    "dict items iterator must return 2-tuples",
                ));
            }
            pickle_dump_obj_binary(_py, state, fields[0], true)?;
            pickle_dump_obj_binary(_py, state, fields[1], true)?;
        } else {
            pickle_dump_obj_binary(_py, state, item_bits, true)?;
        }
    }
    if dict_items {
        state.push(PICKLE_OP_SETITEMS);
    } else {
        state.push(PICKLE_OP_APPENDS);
    }
    Ok(())
}

fn pickle_dump_reduce_value(
    _py: &crate::PyToken<'_>,
    state: &mut PickleDumpState,
    reduce_bits: u64,
    obj_bits: Option<u64>,
) -> Result<(), u64> {
    let Some(ptr) = obj_from_bits(reduce_bits).as_ptr() else {
        return Err(pickle_raise(
            _py,
            "__reduce__ must return a string or tuple",
        ));
    };
    let reduce_type = unsafe { object_type_id(ptr) };
    if reduce_type == TYPE_ID_STRING {
        let Some(global_name) = string_obj_to_owned(obj_from_bits(reduce_bits)) else {
            return Err(pickle_raise(
                _py,
                "__reduce__ must return a string or tuple",
            ));
        };
        let Some(obj_bits) = obj_bits else {
            return Err(pickle_raise(
                _py,
                "__reduce__ must return a string or tuple",
            ));
        };
        let Some(module_bits) = pickle_attr_optional(_py, obj_bits, b"__module__")? else {
            return Err(pickle_raise(
                _py,
                "__reduce__ must return a string or tuple",
            ));
        };
        let Some(module_name) = string_obj_to_owned(obj_from_bits(module_bits)) else {
            dec_ref_bits(_py, module_bits);
            return Err(pickle_raise(
                _py,
                "__reduce__ must return a string or tuple",
            ));
        };
        dec_ref_bits(_py, module_bits);
        let resolved_bits =
            pickle_resolve_global_bits(_py, module_name.as_str(), global_name.as_str())?;
        let matches = resolved_bits == obj_bits;
        if !obj_from_bits(resolved_bits).is_none() {
            dec_ref_bits(_py, resolved_bits);
        }
        if !matches {
            let obj_type = type_name(_py, obj_from_bits(obj_bits));
            let msg = format!(
                "Can't pickle {obj_type}: it's not the same object as {}.{}",
                module_name, global_name
            );
            return Err(pickle_raise(_py, &msg));
        }
        if state.protocol >= PICKLE_PROTO_4 {
            pickle_dump_unicode_binary(_py, state, module_name.as_str())?;
            pickle_dump_unicode_binary(_py, state, global_name.as_str())?;
            state.push(PICKLE_OP_STACK_GLOBAL);
        } else {
            pickle_emit_global_opcode(state, module_name.as_str(), global_name.as_str());
        }
        let _ = pickle_memo_store_if_absent(state, obj_bits);
        return Ok(());
    }
    if reduce_type != TYPE_ID_TUPLE {
        return Err(pickle_raise(
            _py,
            "__reduce__ must return a string or tuple",
        ));
    }
    let fields = unsafe { seq_vec_ref(ptr) };
    if !(2..=6).contains(&fields.len()) {
        return Err(pickle_raise(
            _py,
            "tuple returned by __reduce__ must contain 2 through 6 elements",
        ));
    }
    let callable_bits = fields[0];
    let callable_check = molt_is_callable(callable_bits);
    if !is_truthy(_py, obj_from_bits(callable_check)) {
        return Err(pickle_raise(
            _py,
            "first item of the tuple returned by __reduce__ must be callable",
        ));
    }
    let args_bits = fields[1];
    let Some(args_ptr) = obj_from_bits(args_bits).as_ptr() else {
        return Err(pickle_raise(
            _py,
            "second item of the tuple returned by __reduce__ must be a tuple",
        ));
    };
    if unsafe { object_type_id(args_ptr) } != TYPE_ID_TUPLE {
        return Err(pickle_raise(
            _py,
            "second item of the tuple returned by __reduce__ must be a tuple",
        ));
    }
    if fields.len() >= 4 && !obj_from_bits(fields[3]).is_none() {
        let iter_bits = molt_iter(fields[3]);
        if exception_pending(_py) {
            clear_exception(_py);
            let value_type = type_name(_py, obj_from_bits(fields[3]));
            let msg = format!(
                "fourth element of the tuple returned by __reduce__ must be an iterator, not {value_type}"
            );
            return Err(pickle_raise(_py, &msg));
        }
        if !obj_from_bits(iter_bits).is_none() {
            dec_ref_bits(_py, iter_bits);
        }
    }
    if fields.len() >= 5 && !obj_from_bits(fields[4]).is_none() {
        let iter_bits = molt_iter(fields[4]);
        if exception_pending(_py) {
            clear_exception(_py);
            let value_type = type_name(_py, obj_from_bits(fields[4]));
            let msg = format!(
                "fifth element of the tuple returned by __reduce__ must be an iterator, not {value_type}"
            );
            return Err(pickle_raise(_py, &msg));
        }
        if !obj_from_bits(iter_bits).is_none() {
            dec_ref_bits(_py, iter_bits);
        }
    }
    if fields.len() >= 6 && !obj_from_bits(fields[5]).is_none() {
        let setter_check = molt_is_callable(fields[5]);
        if !is_truthy(_py, obj_from_bits(setter_check)) {
            let value_type = type_name(_py, obj_from_bits(fields[5]));
            let msg = format!(
                "sixth element of the tuple returned by __reduce__ must be a function, not {value_type}"
            );
            return Err(pickle_raise(_py, &msg));
        }
    }
    pickle_dump_obj_binary(_py, state, callable_bits, true)?;
    pickle_dump_obj_binary(_py, state, args_bits, true)?;
    state.push(PICKLE_OP_REDUCE);
    if let Some(bits) = obj_bits {
        let _ = pickle_memo_store_if_absent(state, bits);
    }
    let state_bits = if fields.len() >= 3 {
        Some(fields[2])
    } else {
        None
    };
    let state_setter_bits = if fields.len() >= 6 {
        Some(fields[5])
    } else {
        None
    };
    if let Some(state_bits) = state_bits
        && !obj_from_bits(state_bits).is_none()
    {
        if let Some(state_setter_bits) = state_setter_bits {
            if !obj_from_bits(state_setter_bits).is_none() {
                let Some(obj_bits) = obj_bits else {
                    return Err(pickle_raise(
                        _py,
                        "pickle reducer state_setter requires object context",
                    ));
                };
                pickle_dump_obj_binary(_py, state, state_setter_bits, true)?;
                pickle_dump_obj_binary(_py, state, obj_bits, true)?;
                pickle_dump_obj_binary(_py, state, state_bits, true)?;
                state.push(PICKLE_OP_TUPLE2);
                state.push(PICKLE_OP_REDUCE);
                state.push(PICKLE_OP_POP);
            } else {
                pickle_dump_obj_binary(_py, state, state_bits, true)?;
                state.push(PICKLE_OP_BUILD);
            }
        } else {
            pickle_dump_obj_binary(_py, state, state_bits, true)?;
            state.push(PICKLE_OP_BUILD);
        }
    }
    if fields.len() >= 4 && !obj_from_bits(fields[3]).is_none() {
        pickle_dump_items_from_iterable(
            _py,
            state,
            fields[3],
            false,
            "fourth element of the tuple returned by __reduce__ must be an iterator, not ",
        )?;
    }
    if fields.len() >= 5 && !obj_from_bits(fields[4]).is_none() {
        pickle_dump_items_from_iterable(
            _py,
            state,
            fields[4],
            true,
            "fifth element of the tuple returned by __reduce__ must be an iterator, not ",
        )?;
    }
    Ok(())
}

fn pickle_empty_tuple_bits(_py: &crate::PyToken<'_>) -> Result<u64, u64> {
    let tuple_ptr = alloc_tuple(_py, &[]);
    if tuple_ptr.is_null() {
        Err(MoltObject::none().bits())
    } else {
        Ok(MoltObject::from_ptr(tuple_ptr).bits())
    }
}

fn pickle_require_tuple_bits(
    _py: &crate::PyToken<'_>,
    bits: u64,
    context: &str,
) -> Result<(), u64> {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        let msg = format!("pickle.dumps: {context} must be tuple");
        return Err(pickle_raise(_py, &msg));
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_TUPLE {
        let msg = format!("pickle.dumps: {context} must be tuple");
        return Err(pickle_raise(_py, &msg));
    }
    Ok(())
}

fn pickle_require_dict_bits(_py: &crate::PyToken<'_>, bits: u64, context: &str) -> Result<(), u64> {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        let msg = format!("pickle.dumps: {context} must be dict");
        return Err(pickle_raise(_py, &msg));
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_DICT {
        let msg = format!("pickle.dumps: {context} must be dict");
        return Err(pickle_raise(_py, &msg));
    }
    Ok(())
}

fn pickle_default_newobj_args(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
) -> Result<(u64, Option<u64>), u64> {
    if let Some(getnewargs_ex_bits) = pickle_attr_optional(_py, obj_bits, b"__getnewargs_ex__")? {
        let out_bits = unsafe { call_callable0(_py, getnewargs_ex_bits) };
        dec_ref_bits(_py, getnewargs_ex_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        let Some(tuple_ptr) = obj_from_bits(out_bits).as_ptr() else {
            if !obj_from_bits(out_bits).is_none() {
                dec_ref_bits(_py, out_bits);
            }
            return Err(pickle_raise(
                _py,
                "pickle.dumps: __getnewargs_ex__ must return tuple(size=2)",
            ));
        };
        if unsafe { object_type_id(tuple_ptr) } != TYPE_ID_TUPLE {
            dec_ref_bits(_py, out_bits);
            return Err(pickle_raise(
                _py,
                "pickle.dumps: __getnewargs_ex__ must return tuple(size=2)",
            ));
        }
        let fields = unsafe { seq_vec_ref(tuple_ptr).to_vec() };
        if fields.len() != 2 {
            dec_ref_bits(_py, out_bits);
            return Err(pickle_raise(
                _py,
                "pickle.dumps: __getnewargs_ex__ must return tuple(size=2)",
            ));
        }
        let args_bits = fields[0];
        let kwargs_bits = fields[1];
        pickle_require_tuple_bits(_py, args_bits, "__getnewargs_ex__ args")?;
        pickle_require_dict_bits(_py, kwargs_bits, "__getnewargs_ex__ kwargs")?;
        inc_ref_bits(_py, args_bits);
        inc_ref_bits(_py, kwargs_bits);
        dec_ref_bits(_py, out_bits);
        return Ok((args_bits, Some(kwargs_bits)));
    }

    if let Some(getnewargs_bits) = pickle_attr_optional(_py, obj_bits, b"__getnewargs__")? {
        let args_bits = unsafe { call_callable0(_py, getnewargs_bits) };
        dec_ref_bits(_py, getnewargs_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        if let Err(err_bits) = pickle_require_tuple_bits(_py, args_bits, "__getnewargs__ value") {
            if !obj_from_bits(args_bits).is_none() {
                dec_ref_bits(_py, args_bits);
            }
            return Err(err_bits);
        }
        return Ok((args_bits, None));
    }

    Ok((pickle_empty_tuple_bits(_py)?, None))
}

fn pickle_dataclass_state_bits(_py: &crate::PyToken<'_>, ptr: *mut u8) -> Result<Option<u64>, u64> {
    let desc_ptr = unsafe { crate::dataclass_desc_ptr(ptr) };
    if desc_ptr.is_null() {
        return Ok(None);
    }

    if unsafe { (*desc_ptr).slots } {
        let slot_state_ptr = alloc_dict_with_pairs(_py, &[]);
        if slot_state_ptr.is_null() {
            return Err(MoltObject::none().bits());
        }
        let slot_state_bits = MoltObject::from_ptr(slot_state_ptr).bits();
        let mut wrote_any = false;
        let field_values = unsafe { crate::dataclass_fields_ref(ptr) };
        let field_names = unsafe { &(*desc_ptr).field_names };
        for (name, value_bits) in field_names.iter().zip(field_values.iter().copied()) {
            let Some(name_bits) = alloc_string_bits(_py, name) else {
                dec_ref_bits(_py, slot_state_bits);
                return Err(MoltObject::none().bits());
            };
            unsafe {
                crate::dict_set_in_place(_py, slot_state_ptr, name_bits, value_bits);
            }
            dec_ref_bits(_py, name_bits);
            if exception_pending(_py) {
                dec_ref_bits(_py, slot_state_bits);
                return Err(MoltObject::none().bits());
            }
            wrote_any = true;
        }
        if !wrote_any {
            dec_ref_bits(_py, slot_state_bits);
            return Ok(None);
        }
        let tuple_ptr = alloc_tuple(_py, &[MoltObject::none().bits(), slot_state_bits]);
        dec_ref_bits(_py, slot_state_bits);
        if tuple_ptr.is_null() {
            return Err(MoltObject::none().bits());
        }
        return Ok(Some(MoltObject::from_ptr(tuple_ptr).bits()));
    }

    if !unsafe { (*desc_ptr).slots } {
        let dict_bits = unsafe { crate::dataclass_dict_bits(ptr) };
        if dict_bits != 0
            && let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
            && unsafe { object_type_id(dict_ptr) } == TYPE_ID_DICT
            && !unsafe { crate::dict_order(dict_ptr).is_empty() }
        {
            inc_ref_bits(_py, dict_bits);
            return Ok(Some(dict_bits));
        }
    }

    let state_ptr = alloc_dict_with_pairs(_py, &[]);
    if state_ptr.is_null() {
        return Err(MoltObject::none().bits());
    }
    let state_bits = MoltObject::from_ptr(state_ptr).bits();
    let mut wrote_any = false;

    let field_values = unsafe { crate::dataclass_fields_ref(ptr) };
    let field_names = unsafe { &(*desc_ptr).field_names };
    for (name, value_bits) in field_names.iter().zip(field_values.iter().copied()) {
        let Some(name_bits) = alloc_string_bits(_py, name) else {
            dec_ref_bits(_py, state_bits);
            return Err(MoltObject::none().bits());
        };
        unsafe {
            crate::dict_set_in_place(_py, state_ptr, name_bits, value_bits);
        }
        dec_ref_bits(_py, name_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, state_bits);
            return Err(MoltObject::none().bits());
        }
        wrote_any = true;
    }

    let extra_bits = unsafe { crate::dataclass_dict_bits(ptr) };
    if extra_bits != 0
        && let Some(extra_ptr) = obj_from_bits(extra_bits).as_ptr()
        && unsafe { object_type_id(extra_ptr) } == TYPE_ID_DICT
    {
        let pairs = unsafe { crate::dict_order(extra_ptr).to_vec() };
        let mut idx = 0usize;
        while idx + 1 < pairs.len() {
            unsafe {
                crate::dict_set_in_place(_py, state_ptr, pairs[idx], pairs[idx + 1]);
            }
            if exception_pending(_py) {
                dec_ref_bits(_py, state_bits);
                return Err(MoltObject::none().bits());
            }
            wrote_any = true;
            idx += 2;
        }
    }

    if !wrote_any {
        dec_ref_bits(_py, state_bits);
        return Ok(None);
    }
    Ok(Some(state_bits))
}

fn pickle_object_slot_state_bits(
    _py: &crate::PyToken<'_>,
    ptr: *mut u8,
) -> Result<Option<u64>, u64> {
    let class_bits = unsafe { object_class_bits(ptr) };
    let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
        return Ok(None);
    };
    if unsafe { object_type_id(class_ptr) } != crate::TYPE_ID_TYPE {
        return Ok(None);
    }

    let class_dict_bits = unsafe { crate::class_dict_bits(class_ptr) };
    let Some(class_dict_ptr) = obj_from_bits(class_dict_bits).as_ptr() else {
        return Ok(None);
    };
    if unsafe { object_type_id(class_dict_ptr) } != TYPE_ID_DICT {
        return Ok(None);
    }

    let Some(offsets_name_bits) = attr_name_bits_from_bytes(_py, b"__molt_field_offsets__") else {
        return Err(MoltObject::none().bits());
    };
    let offsets_bits = unsafe { dict_get_in_place(_py, class_dict_ptr, offsets_name_bits) };
    dec_ref_bits(_py, offsets_name_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let Some(offsets_bits) = offsets_bits else {
        return Ok(None);
    };
    let Some(offsets_ptr) = obj_from_bits(offsets_bits).as_ptr() else {
        return Ok(None);
    };
    if unsafe { object_type_id(offsets_ptr) } != TYPE_ID_DICT {
        return Ok(None);
    }

    let slot_state_ptr = alloc_dict_with_pairs(_py, &[]);
    if slot_state_ptr.is_null() {
        return Err(MoltObject::none().bits());
    }
    let slot_state_bits = MoltObject::from_ptr(slot_state_ptr).bits();
    let mut wrote_any = false;
    let pairs = unsafe { crate::dict_order(offsets_ptr).to_vec() };
    let mut idx = 0usize;
    while idx + 1 < pairs.len() {
        let name_bits = pairs[idx];
        let offset_bits = pairs[idx + 1];
        idx += 2;
        let Some(offset) = to_i64(obj_from_bits(offset_bits)) else {
            continue;
        };
        if offset < 0 {
            continue;
        }
        let value_bits = unsafe { crate::object_field_get_ptr_raw(_py, ptr, offset as usize) };
        if exception_pending(_py) {
            dec_ref_bits(_py, slot_state_bits);
            return Err(MoltObject::none().bits());
        }
        if value_bits == missing_bits(_py) {
            dec_ref_bits(_py, value_bits);
            continue;
        }
        unsafe {
            crate::dict_set_in_place(_py, slot_state_ptr, name_bits, value_bits);
        }
        dec_ref_bits(_py, value_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, slot_state_bits);
            return Err(MoltObject::none().bits());
        }
        wrote_any = true;
    }
    if !wrote_any {
        dec_ref_bits(_py, slot_state_bits);
        return Ok(None);
    }
    Ok(Some(slot_state_bits))
}

fn pickle_object_state_bits(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    ptr: *mut u8,
) -> Result<Option<u64>, u64> {
    let mut dict_state_bits: Option<u64> = None;
    // Try the fast path first: trailing __dict__ slot.
    let dict_bits = unsafe { crate::instance_dict_bits(ptr) };
    if dict_bits != 0
        && let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
        && unsafe { object_type_id(dict_ptr) } == TYPE_ID_DICT
        && !unsafe { crate::dict_order(dict_ptr).is_empty() }
    {
        inc_ref_bits(_py, dict_bits);
        dict_state_bits = Some(dict_bits);
    }
    // Fall back to getattr(__dict__) when the trailing slot is empty/missing.
    // The compiler may store attributes in a dict accessible only through getattr.
    if dict_state_bits.is_none()
        && !exception_pending(_py)
        && let Some(dict_name_bits) = attr_name_bits_from_bytes(_py, b"__dict__")
    {
        let missing = missing_bits(_py);
        let attr_dict_bits = molt_getattr_builtin(obj_bits, dict_name_bits, missing);
        dec_ref_bits(_py, dict_name_bits);
        if !exception_pending(_py)
            && attr_dict_bits != missing
            && let Some(dict_ptr) = obj_from_bits(attr_dict_bits).as_ptr()
            && unsafe { object_type_id(dict_ptr) } == TYPE_ID_DICT
            && !unsafe { crate::dict_order(dict_ptr).is_empty() }
        {
            // attr_dict_bits already carries a reference from getattr.
            dict_state_bits = Some(attr_dict_bits);
        } else if attr_dict_bits != missing && !obj_from_bits(attr_dict_bits).is_none() {
            dec_ref_bits(_py, attr_dict_bits);
        }
        // Clear AttributeError if __dict__ wasn't found.
        if exception_pending(_py) {
            clear_exception(_py);
        }
    }

    let slot_state_bits = pickle_object_slot_state_bits(_py, ptr)?;
    let Some(slot_state_bits) = slot_state_bits else {
        return Ok(dict_state_bits);
    };

    let dict_or_none_bits = dict_state_bits.unwrap_or(MoltObject::none().bits());
    let tuple_ptr = alloc_tuple(_py, &[dict_or_none_bits, slot_state_bits]);
    if let Some(bits) = dict_state_bits {
        dec_ref_bits(_py, bits);
    }
    dec_ref_bits(_py, slot_state_bits);
    if tuple_ptr.is_null() {
        return Err(MoltObject::none().bits());
    }
    Ok(Some(MoltObject::from_ptr(tuple_ptr).bits()))
}

fn pickle_default_instance_state(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    ptr: *mut u8,
    type_id: u32,
) -> Result<Option<u64>, u64> {
    if let Some(getstate_bits) = pickle_attr_optional(_py, obj_bits, b"__getstate__")? {
        let state_bits = unsafe { call_callable0(_py, getstate_bits) };
        dec_ref_bits(_py, getstate_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        return Ok(Some(state_bits));
    }
    if type_id == crate::TYPE_ID_DATACLASS {
        return pickle_dataclass_state_bits(_py, ptr);
    }
    if type_id == crate::TYPE_ID_OBJECT {
        return pickle_object_state_bits(_py, obj_bits, ptr);
    }
    Ok(None)
}

fn pickle_dump_default_instance(
    _py: &crate::PyToken<'_>,
    state: &mut PickleDumpState,
    obj_bits: u64,
    ptr: *mut u8,
    type_id: u32,
) -> Result<bool, u64> {
    if type_id != crate::TYPE_ID_OBJECT && type_id != crate::TYPE_ID_DATACLASS {
        return Ok(false);
    }
    let cls_bits = unsafe { object_class_bits(ptr) };
    if cls_bits == 0 || obj_from_bits(cls_bits).as_ptr().is_none() {
        return Ok(false);
    }

    let (args_bits, kwargs_bits) = pickle_default_newobj_args(_py, obj_bits)?;
    let result = (|| -> Result<(), u64> {
        let mut kwargs_effective = kwargs_bits;
        if let Some(bits) = kwargs_effective {
            let Some(dict_ptr) = obj_from_bits(bits).as_ptr() else {
                return Err(pickle_raise(_py, "pickle.dumps: kwargs must be dict"));
            };
            if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
                return Err(pickle_raise(_py, "pickle.dumps: kwargs must be dict"));
            }
            if unsafe { crate::dict_order(dict_ptr).is_empty() } {
                kwargs_effective = None;
            }
        }

        if let Some(kwargs_bits) = kwargs_effective {
            if state.protocol >= PICKLE_PROTO_4 {
                pickle_dump_obj_binary(_py, state, cls_bits, true)?;
                pickle_dump_obj_binary(_py, state, args_bits, true)?;
                pickle_dump_obj_binary(_py, state, kwargs_bits, true)?;
                state.push(PICKLE_OP_NEWOBJ_EX);
            } else {
                pickle_emit_global_opcode(state, "copyreg", "__newobj_ex__");
                pickle_dump_obj_binary(_py, state, cls_bits, true)?;
                pickle_dump_obj_binary(_py, state, args_bits, true)?;
                pickle_dump_obj_binary(_py, state, kwargs_bits, true)?;
                state.push(PICKLE_OP_TUPLE3);
                state.push(PICKLE_OP_REDUCE);
            }
        } else {
            pickle_dump_obj_binary(_py, state, cls_bits, true)?;
            pickle_dump_obj_binary(_py, state, args_bits, true)?;
            state.push(PICKLE_OP_NEWOBJ);
        }

        let _ = pickle_memo_store_if_absent(state, obj_bits);
        if let Some(state_bits) = pickle_default_instance_state(_py, obj_bits, ptr, type_id)? {
            if !obj_from_bits(state_bits).is_none() {
                pickle_dump_obj_binary(_py, state, state_bits, true)?;
                state.push(PICKLE_OP_BUILD);
            }
            if !obj_from_bits(state_bits).is_none() {
                dec_ref_bits(_py, state_bits);
            }
        }
        Ok(())
    })();

    if !obj_from_bits(args_bits).is_none() {
        dec_ref_bits(_py, args_bits);
    }
    if let Some(bits) = kwargs_bits
        && !obj_from_bits(bits).is_none()
    {
        dec_ref_bits(_py, bits);
    }
    result.map(|()| true)
}

fn pickle_dump_obj_binary(
    _py: &crate::PyToken<'_>,
    state: &mut PickleDumpState,
    obj_bits: u64,
    allow_persistent_id: bool,
) -> Result<(), u64> {
    if state.depth >= PICKLE_RECURSION_LIMIT {
        return Err(pickle_raise(
            _py,
            "pickle.dumps: maximum recursion depth exceeded",
        ));
    }
    state.depth += 1;
    let result = (|| -> Result<(), u64> {
        if allow_persistent_id && pickle_dump_maybe_persistent(_py, state, obj_bits)? {
            return Ok(());
        }
        if let Some(index) = pickle_memo_lookup(state, obj_bits) {
            pickle_emit_memo_get(state, index);
            return Ok(());
        }
        let obj = obj_from_bits(obj_bits);
        if obj.is_none() {
            state.push(PICKLE_OP_NONE);
            return Ok(());
        }
        if let Some(value) = obj.as_bool() {
            state.push(if value {
                PICKLE_OP_NEWTRUE
            } else {
                PICKLE_OP_NEWFALSE
            });
            return Ok(());
        }
        if let Some(value) = obj.as_int() {
            pickle_dump_int_binary(state, value);
            return Ok(());
        }
        if let Some(value) = obj.as_float() {
            pickle_dump_float_binary(state, value);
            return Ok(());
        }
        let Some(ptr) = obj.as_ptr() else {
            let type_name = type_name(_py, obj);
            let msg = format!("pickle.dumps: unsupported type: {type_name}");
            return Err(pickle_raise(_py, &msg));
        };
        let type_id = unsafe { object_type_id(ptr) };
        if type_id == TYPE_ID_STRING {
            let text = string_obj_to_owned(obj)
                .ok_or_else(|| pickle_raise(_py, "pickle.dumps: string conversion failed"))?;
            pickle_dump_unicode_binary(_py, state, text.as_str())?;
            let _ = pickle_memo_store_if_absent(state, obj_bits);
            return Ok(());
        }
        if type_id == crate::TYPE_ID_BYTES {
            let raw = unsafe { bytes_like_slice(ptr) }
                .ok_or_else(|| pickle_raise(_py, "pickle.dumps: bytes conversion failed"))?;
            if state.protocol < PICKLE_PROTO_3 {
                pickle_emit_global_opcode(state, "_codecs", "encode");
                let latin1 = pickle_decode_latin1(raw);
                pickle_dump_unicode_binary(_py, state, &latin1)?;
                pickle_dump_unicode_binary(_py, state, "latin1")?;
                state.push(PICKLE_OP_TUPLE2);
                state.push(PICKLE_OP_REDUCE);
            } else {
                pickle_dump_bytes_binary(_py, state, raw)?;
            }
            let _ = pickle_memo_store_if_absent(state, obj_bits);
            return Ok(());
        }
        if type_id == crate::TYPE_ID_BYTEARRAY {
            let raw = unsafe { bytes_like_slice(ptr) }
                .ok_or_else(|| pickle_raise(_py, "pickle.dumps: bytearray conversion failed"))?;
            pickle_dump_bytearray_binary(_py, state, raw)?;
            let _ = pickle_memo_store_if_absent(state, obj_bits);
            return Ok(());
        }
        if let Some((payload_bits, readonly)) = pickle_extract_picklebuffer_payload(_py, obj_bits)?
        {
            if pickle_dump_maybe_out_of_band_buffer(_py, state, obj_bits, readonly)? {
                if !obj_from_bits(payload_bits).is_none() {
                    dec_ref_bits(_py, payload_bits);
                }
                return Ok(());
            }
            let Some(payload_ptr) = obj_from_bits(payload_bits).as_ptr() else {
                return Err(pickle_raise(
                    _py,
                    "pickle.dumps: PickleBuffer.raw() must be bytes-like",
                ));
            };
            let raw = unsafe { bytes_like_slice(payload_ptr) }.ok_or_else(|| {
                pickle_raise(_py, "pickle.dumps: PickleBuffer.raw() must be bytes-like")
            })?;
            if readonly {
                pickle_dump_bytes_binary(_py, state, raw)?;
            } else {
                pickle_dump_bytearray_binary(_py, state, raw)?;
            }
            if !obj_from_bits(payload_bits).is_none() {
                dec_ref_bits(_py, payload_bits);
            }
            let _ = pickle_memo_store_if_absent(state, obj_bits);
            return Ok(());
        }
        if type_id == TYPE_ID_TUPLE {
            let values = unsafe { seq_vec_ref(ptr).to_vec() };
            match values.len() {
                0 => state.push(PICKLE_OP_EMPTY_TUPLE),
                1 => {
                    pickle_dump_obj_binary(_py, state, values[0], true)?;
                    state.push(PICKLE_OP_TUPLE1);
                }
                2 => {
                    pickle_dump_obj_binary(_py, state, values[0], true)?;
                    pickle_dump_obj_binary(_py, state, values[1], true)?;
                    state.push(PICKLE_OP_TUPLE2);
                }
                3 => {
                    pickle_dump_obj_binary(_py, state, values[0], true)?;
                    pickle_dump_obj_binary(_py, state, values[1], true)?;
                    pickle_dump_obj_binary(_py, state, values[2], true)?;
                    state.push(PICKLE_OP_TUPLE3);
                }
                _ => {
                    state.push(PICKLE_OP_MARK);
                    for entry in values {
                        pickle_dump_obj_binary(_py, state, entry, true)?;
                    }
                    state.push(PICKLE_OP_TUPLE);
                }
            }
            let _ = pickle_memo_store_if_absent(state, obj_bits);
            return Ok(());
        }
        if type_id == TYPE_ID_LIST {
            state.push(PICKLE_OP_EMPTY_LIST);
            let _ = pickle_memo_store_if_absent(state, obj_bits);
            let values = unsafe { seq_vec_ref(ptr).to_vec() };
            if !values.is_empty() {
                state.push(PICKLE_OP_MARK);
                for entry in values {
                    pickle_dump_obj_binary(_py, state, entry, true)?;
                }
                state.push(PICKLE_OP_APPENDS);
            }
            return Ok(());
        }
        if type_id == TYPE_ID_DICT {
            state.push(PICKLE_OP_EMPTY_DICT);
            let _ = pickle_memo_store_if_absent(state, obj_bits);
            let pairs = unsafe { crate::dict_order(ptr).to_vec() };
            if !pairs.is_empty() {
                state.push(PICKLE_OP_MARK);
                let mut idx = 0usize;
                while idx + 1 < pairs.len() {
                    pickle_dump_obj_binary(_py, state, pairs[idx], true)?;
                    pickle_dump_obj_binary(_py, state, pairs[idx + 1], true)?;
                    idx += 2;
                }
                state.push(PICKLE_OP_SETITEMS);
            }
            return Ok(());
        }
        if type_id == crate::TYPE_ID_SET {
            if state.protocol >= PICKLE_PROTO_4 {
                state.push(PICKLE_OP_EMPTY_SET);
                let _ = pickle_memo_store_if_absent(state, obj_bits);
                let values = unsafe { crate::set_order(ptr).to_vec() };
                if !values.is_empty() {
                    state.push(PICKLE_OP_MARK);
                    for entry in values {
                        pickle_dump_obj_binary(_py, state, entry, true)?;
                    }
                    state.push(PICKLE_OP_ADDITEMS);
                }
                return Ok(());
            }
            pickle_emit_global_opcode(state, "builtins", "set");
            state.push(PICKLE_OP_EMPTY_LIST);
            let values = unsafe { crate::set_order(ptr).to_vec() };
            let _ = pickle_memo_store_if_absent(state, obj_bits);
            if !values.is_empty() {
                state.push(PICKLE_OP_MARK);
                for entry in values {
                    pickle_dump_obj_binary(_py, state, entry, true)?;
                }
                state.push(PICKLE_OP_APPENDS);
            }
            state.push(PICKLE_OP_TUPLE1);
            state.push(PICKLE_OP_REDUCE);
            return Ok(());
        }
        if type_id == crate::TYPE_ID_FROZENSET {
            if state.protocol >= PICKLE_PROTO_4 {
                state.push(PICKLE_OP_MARK);
                let values = unsafe { crate::set_order(ptr).to_vec() };
                for entry in values {
                    pickle_dump_obj_binary(_py, state, entry, true)?;
                }
                state.push(PICKLE_OP_FROZENSET);
                let _ = pickle_memo_store_if_absent(state, obj_bits);
                return Ok(());
            }
            pickle_emit_global_opcode(state, "builtins", "frozenset");
            state.push(PICKLE_OP_EMPTY_LIST);
            let values = unsafe { crate::set_order(ptr).to_vec() };
            if !values.is_empty() {
                state.push(PICKLE_OP_MARK);
                for entry in values {
                    pickle_dump_obj_binary(_py, state, entry, true)?;
                }
                state.push(PICKLE_OP_APPENDS);
            }
            state.push(PICKLE_OP_TUPLE1);
            state.push(PICKLE_OP_REDUCE);
            let _ = pickle_memo_store_if_absent(state, obj_bits);
            return Ok(());
        }
        if type_id == crate::TYPE_ID_SLICE {
            pickle_emit_global_opcode(state, "builtins", "slice");
            pickle_dump_obj_binary(_py, state, unsafe { crate::slice_start_bits(ptr) }, true)?;
            pickle_dump_obj_binary(_py, state, unsafe { crate::slice_stop_bits(ptr) }, true)?;
            pickle_dump_obj_binary(_py, state, unsafe { crate::slice_step_bits(ptr) }, true)?;
            state.push(PICKLE_OP_TUPLE3);
            state.push(PICKLE_OP_REDUCE);
            let _ = pickle_memo_store_if_absent(state, obj_bits);
            return Ok(());
        }
        if pickle_emit_global_ref(_py, state, obj_bits)? {
            let _ = pickle_memo_store_if_absent(state, obj_bits);
            return Ok(());
        }
        if let Some(reduce_bits) = pickle_reduce_value(_py, state, obj_bits)? {
            let dumped = pickle_dump_reduce_value(_py, state, reduce_bits, Some(obj_bits));
            if !obj_from_bits(reduce_bits).is_none() {
                dec_ref_bits(_py, reduce_bits);
            }
            return dumped;
        }
        if pickle_dump_default_instance(_py, state, obj_bits, ptr, type_id)? {
            return Ok(());
        }
        let type_name = type_name(_py, obj_from_bits(obj_bits));
        let message = format!("cannot pickle '{type_name}' object");
        Err(raise_exception::<u64>(_py, "TypeError", &message))
    })();
    state.depth = state.depth.saturating_sub(1);
    result
}

fn pickle_apply_dict_state(
    _py: &crate::PyToken<'_>,
    inst_bits: u64,
    dict_state_bits: u64,
) -> Result<(), u64> {
    if obj_from_bits(dict_state_bits).is_none() {
        return Ok(());
    }
    let Some(state_ptr) = obj_from_bits(dict_state_bits).as_ptr() else {
        return Err(pickle_raise(_py, "pickle.loads: BUILD state must be dict"));
    };
    if unsafe { object_type_id(state_ptr) } != TYPE_ID_DICT {
        return Err(pickle_raise(_py, "pickle.loads: BUILD state must be dict"));
    }

    // Use setattr for each state entry. This correctly routes values to typed
    // field slots (TYPE_ID_OBJECT), dataclass descriptor fields
    // (TYPE_ID_DATACLASS), or __dict__ for fully dynamic instances.
    let pairs = unsafe { crate::dict_order(state_ptr).to_vec() };
    let mut idx = 0usize;
    while idx + 1 < pairs.len() {
        let key_bits = pairs[idx];
        let value_bits = pairs[idx + 1];
        idx += 2;
        let _ = crate::molt_object_setattr(inst_bits, key_bits, value_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
    }
    Ok(())
}

fn pickle_vm_item_to_bits(_py: &crate::PyToken<'_>, item: &PickleVmItem) -> Result<u64, u64> {
    match item {
        PickleVmItem::Value(bits) => Ok(*bits),
        PickleVmItem::Global(global) => pickle_global_callable_bits(_py, *global),
        PickleVmItem::Mark => Err(pickle_raise(_py, "pickle.loads: mark not found")),
    }
}

fn pickle_vm_pop_mark_items(
    _py: &crate::PyToken<'_>,
    stack: &mut Vec<PickleVmItem>,
) -> Result<Vec<PickleVmItem>, u64> {
    let mut out: Vec<PickleVmItem> = Vec::new();
    while let Some(item) = stack.pop() {
        if matches!(item, PickleVmItem::Mark) {
            out.reverse();
            return Ok(out);
        }
        out.push(item);
    }
    Err(pickle_raise(_py, "pickle.loads: mark not found"))
}

fn pickle_vm_pop_value(
    _py: &crate::PyToken<'_>,
    stack: &mut Vec<PickleVmItem>,
) -> Result<u64, u64> {
    let item = stack
        .pop()
        .ok_or_else(|| pickle_raise(_py, "pickle.loads: stack underflow"))?;
    pickle_vm_item_to_bits(_py, &item)
}

fn pickle_decode_8bit_string(
    _py: &crate::PyToken<'_>,
    raw: &[u8],
    encoding: &str,
    _errors: &str,
) -> Result<u64, u64> {
    if encoding.eq_ignore_ascii_case("bytes") {
        let ptr = crate::alloc_bytes(_py, raw);
        if ptr.is_null() {
            return Err(MoltObject::none().bits());
        }
        return Ok(MoltObject::from_ptr(ptr).bits());
    }
    let decoded = if encoding.eq_ignore_ascii_case("latin1")
        || encoding.eq_ignore_ascii_case("latin-1")
    {
        raw.iter().map(|&b| char::from(b)).collect::<String>()
    } else {
        String::from_utf8(raw.to_vec())
            .map_err(|_| pickle_raise(_py, "pickle.loads: unable to decode 8-bit string payload"))?
    };
    let ptr = alloc_string(_py, decoded.as_bytes());
    if ptr.is_null() {
        Err(MoltObject::none().bits())
    } else {
        Ok(MoltObject::from_ptr(ptr).bits())
    }
}

fn pickle_resolve_global_bits(
    _py: &crate::PyToken<'_>,
    module: &str,
    name: &str,
) -> Result<u64, u64> {
    let Some(module_bits) = alloc_string_bits(_py, module) else {
        return Err(MoltObject::none().bits());
    };
    let imported_bits = crate::molt_module_import(module_bits);
    dec_ref_bits(_py, module_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let Some(name_bits) = alloc_string_bits(_py, name) else {
        if !obj_from_bits(imported_bits).is_none() {
            dec_ref_bits(_py, imported_bits);
        }
        return Err(MoltObject::none().bits());
    };
    let value_bits = crate::molt_object_getattribute(imported_bits, name_bits);
    dec_ref_bits(_py, name_bits);
    if !obj_from_bits(imported_bits).is_none() {
        dec_ref_bits(_py, imported_bits);
    }
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(value_bits)
}

fn pickle_resolve_global_with_hook(
    _py: &crate::PyToken<'_>,
    module: &str,
    name: &str,
    find_class_bits: Option<u64>,
) -> Result<u64, u64> {
    if let Some(callback_bits) = find_class_bits {
        let Some(module_bits) = alloc_string_bits(_py, module) else {
            return Err(MoltObject::none().bits());
        };
        let Some(name_bits) = alloc_string_bits(_py, name) else {
            dec_ref_bits(_py, module_bits);
            return Err(MoltObject::none().bits());
        };
        let out_bits = unsafe { call_callable2(_py, callback_bits, module_bits, name_bits) };
        dec_ref_bits(_py, module_bits);
        dec_ref_bits(_py, name_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        return Ok(out_bits);
    }
    pickle_resolve_global_bits(_py, module, name)
}

fn pickle_lookup_extension_bits(
    _py: &crate::PyToken<'_>,
    code: i64,
    find_class_bits: Option<u64>,
) -> Result<u64, u64> {
    let copyreg_bits = pickle_resolve_global_bits(_py, "copyreg", "_inverted_registry")?;
    let Some(dict_ptr) = obj_from_bits(copyreg_bits).as_ptr() else {
        dec_ref_bits(_py, copyreg_bits);
        return Err(pickle_raise(
            _py,
            "pickle.loads: extension registry unavailable",
        ));
    };
    if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
        dec_ref_bits(_py, copyreg_bits);
        return Err(pickle_raise(
            _py,
            "pickle.loads: extension registry unavailable",
        ));
    }
    let code_bits = MoltObject::from_int(code).bits();
    let entry_bits = unsafe { dict_get_in_place(_py, dict_ptr, code_bits) };
    if exception_pending(_py) {
        dec_ref_bits(_py, copyreg_bits);
        return Err(MoltObject::none().bits());
    }
    let Some(entry_bits) = entry_bits else {
        dec_ref_bits(_py, copyreg_bits);
        return Err(pickle_raise(_py, "pickle.loads: unknown extension code"));
    };
    let Some(entry_ptr) = obj_from_bits(entry_bits).as_ptr() else {
        dec_ref_bits(_py, copyreg_bits);
        return Err(pickle_raise(_py, "pickle.loads: invalid extension entry"));
    };
    if unsafe { object_type_id(entry_ptr) } != TYPE_ID_TUPLE {
        dec_ref_bits(_py, copyreg_bits);
        return Err(pickle_raise(_py, "pickle.loads: invalid extension entry"));
    }
    let fields = unsafe { seq_vec_ref(entry_ptr) };
    if fields.len() != 2 {
        dec_ref_bits(_py, copyreg_bits);
        return Err(pickle_raise(_py, "pickle.loads: invalid extension entry"));
    }
    let Some(module) = string_obj_to_owned(obj_from_bits(fields[0])) else {
        dec_ref_bits(_py, copyreg_bits);
        return Err(pickle_raise(_py, "pickle.loads: invalid extension entry"));
    };
    let Some(name) = string_obj_to_owned(obj_from_bits(fields[1])) else {
        dec_ref_bits(_py, copyreg_bits);
        return Err(pickle_raise(_py, "pickle.loads: invalid extension entry"));
    };
    dec_ref_bits(_py, copyreg_bits);
    pickle_resolve_global_with_hook(_py, &module, &name, find_class_bits)
}

fn pickle_apply_newobj(
    _py: &crate::PyToken<'_>,
    cls_bits: u64,
    args_bits: u64,
    kwargs_bits: Option<u64>,
) -> Result<u64, u64> {
    let new_bits = pickle_attr_required(_py, cls_bits, b"__new__")?;
    let Some(args_ptr) = obj_from_bits(args_bits).as_ptr() else {
        dec_ref_bits(_py, new_bits);
        return Err(pickle_raise(_py, "pickle.loads: NEWOBJ args must be tuple"));
    };
    if unsafe { object_type_id(args_ptr) } != TYPE_ID_TUPLE {
        dec_ref_bits(_py, new_bits);
        return Err(pickle_raise(_py, "pickle.loads: NEWOBJ args must be tuple"));
    }
    let args = unsafe { seq_vec_ref(args_ptr).to_vec() };
    let kw_len = if let Some(kw_bits) = kwargs_bits {
        let Some(kw_ptr) = obj_from_bits(kw_bits).as_ptr() else {
            dec_ref_bits(_py, new_bits);
            return Err(pickle_raise(
                _py,
                "pickle.loads: NEWOBJ_EX kwargs must be dict",
            ));
        };
        if unsafe { object_type_id(kw_ptr) } != TYPE_ID_DICT {
            dec_ref_bits(_py, new_bits);
            return Err(pickle_raise(
                _py,
                "pickle.loads: NEWOBJ_EX kwargs must be dict",
            ));
        }
        unsafe { crate::dict_order(kw_ptr).len() / 2 }
    } else {
        0
    };
    let builder_bits = crate::molt_callargs_new((args.len() + 1) as u64, kw_len as u64);
    let _ = unsafe { crate::molt_callargs_push_pos(builder_bits, cls_bits) };
    if exception_pending(_py) {
        dec_ref_bits(_py, new_bits);
        return Err(MoltObject::none().bits());
    }
    for arg in args {
        let _ = unsafe { crate::molt_callargs_push_pos(builder_bits, arg) };
        if exception_pending(_py) {
            dec_ref_bits(_py, new_bits);
            return Err(MoltObject::none().bits());
        }
    }
    if let Some(kw_bits) = kwargs_bits {
        let kw_ptr = obj_from_bits(kw_bits).as_ptr().expect("checked above");
        let pairs = unsafe { crate::dict_order(kw_ptr).to_vec() };
        let mut idx = 0usize;
        while idx + 1 < pairs.len() {
            let key_bits = pairs[idx];
            let val_bits = pairs[idx + 1];
            let _ = unsafe { crate::molt_callargs_push_kw(builder_bits, key_bits, val_bits) };
            if exception_pending(_py) {
                dec_ref_bits(_py, new_bits);
                return Err(MoltObject::none().bits());
            }
            idx += 2;
        }
    }
    let out_bits = crate::molt_call_bind(new_bits, builder_bits);
    dec_ref_bits(_py, new_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    // Initialize typed field slots to the missing sentinel so that
    // uninitialized fields (from __new__ without __init__) are properly
    // recognized as absent by hasattr/getattr.
    pickle_init_missing_fields(_py, out_bits);
    Ok(out_bits)
}

/// Initialize all typed field slots (and dataclass field values) to the missing
/// sentinel. Called after NEWOBJ to ensure fields not populated by BUILD are
/// correctly absent.
fn pickle_init_missing_fields(_py: &crate::PyToken<'_>, inst_bits: u64) {
    let Some(inst_ptr) = obj_from_bits(inst_bits).as_ptr() else {
        return;
    };
    let type_id = unsafe { object_type_id(inst_ptr) };
    let missing = missing_bits(_py);

    if type_id == crate::TYPE_ID_OBJECT {
        // Initialize typed field offsets to missing.
        let class_bits = unsafe { object_class_bits(inst_ptr) };
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            return;
        };
        if unsafe { object_type_id(class_ptr) } != crate::TYPE_ID_TYPE {
            return;
        }
        let cd_bits = unsafe { crate::class_dict_bits(class_ptr) };
        let Some(cd_ptr) = obj_from_bits(cd_bits).as_ptr() else {
            return;
        };
        if unsafe { object_type_id(cd_ptr) } != TYPE_ID_DICT {
            return;
        }
        let Some(offsets_name) = attr_name_bits_from_bytes(_py, b"__molt_field_offsets__") else {
            return;
        };
        let offsets_bits = unsafe { crate::dict_get_in_place(_py, cd_ptr, offsets_name) };
        dec_ref_bits(_py, offsets_name);
        if exception_pending(_py) {
            clear_exception(_py);
            return;
        }
        let Some(offsets_bits) = offsets_bits else {
            return;
        };
        let Some(offsets_ptr) = obj_from_bits(offsets_bits).as_ptr() else {
            return;
        };
        if unsafe { object_type_id(offsets_ptr) } != TYPE_ID_DICT {
            return;
        }
        let pairs = unsafe { crate::dict_order(offsets_ptr).to_vec() };
        let mut idx = 0usize;
        while idx + 1 < pairs.len() {
            let offset_bits = pairs[idx + 1];
            idx += 2;
            if let Some(offset) = to_i64(obj_from_bits(offset_bits)).filter(|&v| v >= 0) {
                unsafe {
                    let slot = inst_ptr.add(offset as usize) as *mut u64;
                    let old = *slot;
                    if old != missing {
                        inc_ref_bits(_py, missing);
                        if obj_from_bits(old).as_ptr().is_some() {
                            dec_ref_bits(_py, old);
                        }
                        *slot = missing;
                    }
                }
            }
        }
    } else if type_id == crate::TYPE_ID_DATACLASS {
        // Initialize dataclass field values to missing.
        let desc_ptr = unsafe { crate::dataclass_desc_ptr(inst_ptr) };
        if desc_ptr.is_null() {
            return;
        }
        let fields = unsafe { crate::dataclass_fields_mut(inst_ptr) };
        for val in fields.iter_mut() {
            if *val != missing {
                inc_ref_bits(_py, missing);
                if obj_from_bits(*val).as_ptr().is_some() {
                    dec_ref_bits(_py, *val);
                }
                *val = missing;
            }
        }
    }
}

fn pickle_apply_build(
    _py: &crate::PyToken<'_>,
    inst_bits: u64,
    state_bits: u64,
) -> Result<u64, u64> {
    if obj_from_bits(state_bits).is_none() {
        return Ok(inst_bits);
    }
    if let Some(setstate_bits) = pickle_attr_optional(_py, inst_bits, b"__setstate__")? {
        let _ = unsafe { call_callable1(_py, setstate_bits, state_bits) };
        dec_ref_bits(_py, setstate_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        return Ok(inst_bits);
    }
    let mut dict_state_bits = state_bits;
    let mut slot_state_bits: Option<u64> = None;
    if let Some(state_ptr) = obj_from_bits(state_bits).as_ptr()
        && unsafe { object_type_id(state_ptr) } == TYPE_ID_TUPLE
    {
        let fields = unsafe { seq_vec_ref(state_ptr) };
        if fields.len() == 2 {
            dict_state_bits = fields[0];
            slot_state_bits = Some(fields[1]);
        }
    }
    pickle_apply_dict_state(_py, inst_bits, dict_state_bits)?;
    if let Some(slot_bits) = slot_state_bits
        && !obj_from_bits(slot_bits).is_none()
    {
        let Some(slot_ptr) = obj_from_bits(slot_bits).as_ptr() else {
            return Err(pickle_raise(
                _py,
                "pickle.loads: BUILD slot state must be dict",
            ));
        };
        if unsafe { object_type_id(slot_ptr) } != TYPE_ID_DICT {
            return Err(pickle_raise(
                _py,
                "pickle.loads: BUILD slot state must be dict",
            ));
        }
        let pairs = unsafe { crate::dict_order(slot_ptr).to_vec() };
        let mut idx = 0usize;
        while idx + 1 < pairs.len() {
            let key_bits = pairs[idx];
            let value_bits = pairs[idx + 1];
            let _ = crate::molt_object_setattr(inst_bits, key_bits, value_bits);
            if exception_pending(_py) {
                return Err(MoltObject::none().bits());
            }
            idx += 2;
        }
    }
    Ok(inst_bits)
}

fn pickle_apply_reduce_vm(
    _py: &crate::PyToken<'_>,
    callable: PickleVmItem,
    args_bits: u64,
) -> Result<u64, u64> {
    let Some(args_ptr) = obj_from_bits(args_bits).as_ptr() else {
        return Err(pickle_raise(_py, "pickle.loads: reduce args must be tuple"));
    };
    if unsafe { object_type_id(args_ptr) } != TYPE_ID_TUPLE {
        return Err(pickle_raise(_py, "pickle.loads: reduce args must be tuple"));
    }
    let args = unsafe { seq_vec_ref(args_ptr).to_vec() };
    let out_bits = match callable {
        PickleVmItem::Mark => {
            return Err(pickle_raise(_py, "pickle.loads: mark cannot be called"));
        }
        PickleVmItem::Global(PickleGlobal::CodecsEncode) => {
            if args.is_empty() || args.len() > 2 {
                return Err(pickle_raise(
                    _py,
                    "pickle.loads: _codecs.encode expects 1 or 2 arguments",
                ));
            }
            let Some(text) = string_obj_to_owned(obj_from_bits(args[0])) else {
                return Err(pickle_raise(
                    _py,
                    "pickle.loads: _codecs.encode text must be str",
                ));
            };
            let encoding = if args.len() == 1 {
                "utf-8".to_string()
            } else {
                let Some(enc) = string_obj_to_owned(obj_from_bits(args[1])) else {
                    return Err(pickle_raise(
                        _py,
                        "pickle.loads: _codecs.encode encoding must be str",
                    ));
                };
                enc
            };
            pickle_encode_text(_py, &text, &encoding)?
        }
        PickleVmItem::Global(global) => {
            let callable_bits = pickle_global_callable_bits(_py, global)?;
            let out_bits = pickle_call_with_args(_py, callable_bits, args.as_slice());
            if exception_pending(_py) {
                return Err(MoltObject::none().bits());
            }
            out_bits
        }
        PickleVmItem::Value(callable_bits) => {
            let out_bits = pickle_call_with_args(_py, callable_bits, args.as_slice());
            if exception_pending(_py) {
                return Err(MoltObject::none().bits());
            }
            out_bits
        }
    };
    Ok(out_bits)
}

fn pickle_apply_reduce_bits(
    _py: &crate::PyToken<'_>,
    callable_bits: u64,
    args_bits: u64,
) -> Result<u64, u64> {
    let Some(args_ptr) = obj_from_bits(args_bits).as_ptr() else {
        return Err(pickle_raise(_py, "pickle.loads: reduce args must be tuple"));
    };
    if unsafe { object_type_id(args_ptr) } != TYPE_ID_TUPLE {
        return Err(pickle_raise(_py, "pickle.loads: reduce args must be tuple"));
    }
    let args = unsafe { seq_vec_ref(args_ptr).to_vec() };
    let out_bits = pickle_call_with_args(_py, callable_bits, args.as_slice());
    if exception_pending(_py) {
        Err(MoltObject::none().bits())
    } else {
        Ok(out_bits)
    }
}

fn pickle_memo_set(
    _py: &crate::PyToken<'_>,
    memo: &mut Vec<Option<PickleVmItem>>,
    index: usize,
    item: PickleVmItem,
) {
    if memo.len() <= index {
        memo.resize(index + 1, None);
    }
    memo[index] = Some(item);
}

fn pickle_memo_get(
    _py: &crate::PyToken<'_>,
    memo: &[Option<PickleVmItem>],
    index: usize,
) -> Result<PickleVmItem, u64> {
    if let Some(Some(item)) = memo.get(index) {
        return Ok(item.clone());
    }
    let msg = format!("pickle.loads: memo key {} missing", index);
    Err(pickle_raise(_py, &msg))
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_pickle_dumps_core(
    obj_bits: u64,
    protocol_bits: u64,
    _fix_imports_bits: u64,
    persistent_id_bits: u64,
    buffer_callback_bits: u64,
    dispatch_table_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(protocol) = to_i64(obj_from_bits(protocol_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pickle protocol must be int");
        };
        if !(-1..=PICKLE_PROTO_5).contains(&protocol) {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "pickle protocol must be in range -1..5",
            );
        }
        let actual_protocol = if protocol < 0 {
            PICKLE_PROTO_5
        } else {
            protocol
        };
        if actual_protocol <= 1 {
            return molt_pickle_dumps_protocol01(
                obj_bits,
                MoltObject::from_int(actual_protocol).bits(),
            );
        }
        let persistent_id =
            match pickle_option_callable_bits(_py, persistent_id_bits, "persistent_id") {
                Ok(bits) => bits,
                Err(err_bits) => return err_bits,
            };
        let buffer_callback =
            match pickle_option_callable_bits(_py, buffer_callback_bits, "buffer_callback") {
                Ok(bits) => bits,
                Err(err_bits) => return err_bits,
            };
        let dispatch_table = if obj_from_bits(dispatch_table_bits).is_none() {
            None
        } else {
            Some(dispatch_table_bits)
        };
        let mut state = PickleDumpState::new(
            actual_protocol,
            persistent_id,
            buffer_callback,
            dispatch_table,
        );
        if state.buffer_callback_bits.is_some() && actual_protocol < PICKLE_PROTO_5 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "buffer_callback requires protocol 5 or higher",
            );
        }
        pickle_emit_proto_header(&mut state);
        if let Err(err_bits) = pickle_dump_obj_binary(_py, &mut state, obj_bits, true) {
            return err_bits;
        }
        state.push(PICKLE_OP_STOP);
        let out_ptr = crate::alloc_bytes(_py, state.out.as_slice());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_pickle_loads_core(
    data_bits: u64,
    _fix_imports_bits: u64,
    encoding_bits: u64,
    errors_bits: u64,
    persistent_load_bits: u64,
    find_class_bits: u64,
    buffers_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let encoding = if let Some(text) = string_obj_to_owned(obj_from_bits(encoding_bits)) {
            text
        } else {
            return raise_exception::<_>(_py, "TypeError", "pickle encoding must be str");
        };
        let errors = if let Some(text) = string_obj_to_owned(obj_from_bits(errors_bits)) {
            text
        } else {
            return raise_exception::<_>(_py, "TypeError", "pickle errors must be str");
        };
        let persistent_load =
            match pickle_option_callable_bits(_py, persistent_load_bits, "persistent_load") {
                Ok(bits) => bits,
                Err(err_bits) => return err_bits,
            };
        let find_class = match pickle_option_callable_bits(_py, find_class_bits, "find_class") {
            Ok(bits) => bits,
            Err(err_bits) => return err_bits,
        };
        let data = match pickle_input_to_bytes(_py, data_bits) {
            Ok(bytes) => bytes,
            Err(err_bits) => return err_bits,
        };
        let buffers_iter = if obj_from_bits(buffers_bits).is_none() {
            None
        } else {
            let iter_bits = molt_iter(buffers_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            Some(iter_bits)
        };
        if data.first().is_none_or(|op| *op != PICKLE_OP_PROTO) {
            let text = match String::from_utf8(data) {
                Ok(value) => value,
                Err(_) => {
                    return raise_exception::<_>(
                        _py,
                        "RuntimeError",
                        "pickle.loads: protocol 0/1 payload must be UTF-8",
                    );
                }
            };
            let text_ptr = alloc_string(_py, text.as_bytes());
            if text_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let text_bits = MoltObject::from_ptr(text_ptr).bits();
            let out_bits = molt_pickle_loads_protocol01(text_bits);
            dec_ref_bits(_py, text_bits);
            return out_bits;
        }

        let mut idx: usize = 0;
        let mut stack: Vec<PickleVmItem> = Vec::new();
        let mut memo: Vec<Option<PickleVmItem>> = Vec::new();
        while idx < data.len() {
            let op = match pickle_read_u8(data.as_slice(), &mut idx, _py) {
                Ok(value) => value,
                Err(err_bits) => return err_bits,
            };
            match op {
                PICKLE_OP_STOP => break,
                PICKLE_OP_POP => {
                    if stack.pop().is_none() {
                        return pickle_raise(_py, "pickle.loads: stack underflow");
                    }
                }
                PICKLE_OP_POP_MARK => {
                    let mut found_mark = false;
                    while let Some(item) = stack.pop() {
                        if matches!(item, PickleVmItem::Mark) {
                            found_mark = true;
                            break;
                        }
                    }
                    if !found_mark {
                        return pickle_raise(_py, "pickle.loads: mark not found");
                    }
                }
                PICKLE_OP_PROTO => {
                    let version = match pickle_read_u8(data.as_slice(), &mut idx, _py) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    if version > PICKLE_PROTO_5 as u8 {
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "unsupported pickle protocol",
                        );
                    }
                }
                PICKLE_OP_FRAME => {
                    if pickle_read_u64_le(data.as_slice(), &mut idx, _py).is_err() {
                        return MoltObject::none().bits();
                    }
                }
                PICKLE_OP_NEXT_BUFFER => {
                    let bits = match pickle_next_external_buffer_bits(_py, buffers_iter) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    stack.push(PickleVmItem::Value(bits));
                }
                PICKLE_OP_READONLY_BUFFER => {
                    let value_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let view_bits =
                        match pickle_buffer_value_to_memoryview(_py, value_bits, "READONLY_BUFFER")
                        {
                            Ok(v) => v,
                            Err(err_bits) => return err_bits,
                        };
                    let readonly_bits = if let Some(toreadonly_bits) =
                        match pickle_attr_optional(_py, view_bits, b"toreadonly") {
                            Ok(bits) => bits,
                            Err(err_bits) => return err_bits,
                        } {
                        let out_bits = unsafe { call_callable0(_py, toreadonly_bits) };
                        dec_ref_bits(_py, toreadonly_bits);
                        if exception_pending(_py) {
                            return MoltObject::none().bits();
                        }
                        out_bits
                    } else {
                        view_bits
                    };
                    stack.push(PickleVmItem::Value(readonly_bits));
                }
                PICKLE_OP_MARK => stack.push(PickleVmItem::Mark),
                PICKLE_OP_NONE => stack.push(PickleVmItem::Value(MoltObject::none().bits())),
                PICKLE_OP_NEWTRUE => {
                    stack.push(PickleVmItem::Value(MoltObject::from_bool(true).bits()))
                }
                PICKLE_OP_NEWFALSE => {
                    stack.push(PickleVmItem::Value(MoltObject::from_bool(false).bits()))
                }
                PICKLE_OP_INT => {
                    let line = match pickle_read_line_bytes(data.as_slice(), &mut idx, _py) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let line_text = match std::str::from_utf8(line) {
                        Ok(text) => text,
                        Err(_) => return pickle_raise(_py, "pickle.loads: invalid INT payload"),
                    };
                    let bits = match pickle_parse_int_bits(_py, line_text) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    stack.push(PickleVmItem::Value(bits));
                }
                PICKLE_OP_BININT => {
                    let raw = match pickle_read_exact(data.as_slice(), &mut idx, 4, _py) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let value = i32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
                    stack.push(PickleVmItem::Value(
                        MoltObject::from_int(value as i64).bits(),
                    ));
                }
                PICKLE_OP_BININT1 => {
                    let value = match pickle_read_u8(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v as i64,
                        Err(err_bits) => return err_bits,
                    };
                    stack.push(PickleVmItem::Value(MoltObject::from_int(value).bits()));
                }
                PICKLE_OP_BININT2 => {
                    let value = match pickle_read_u16_le(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v as i64,
                        Err(err_bits) => return err_bits,
                    };
                    stack.push(PickleVmItem::Value(MoltObject::from_int(value).bits()));
                }
                PICKLE_OP_LONG => {
                    let line = match pickle_read_line_bytes(data.as_slice(), &mut idx, _py) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let line_text = match std::str::from_utf8(line) {
                        Ok(text) => text,
                        Err(_) => return pickle_raise(_py, "pickle.loads: invalid LONG payload"),
                    };
                    let bits = match pickle_parse_long_line_bits(_py, line_text) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    stack.push(PickleVmItem::Value(bits));
                }
                PICKLE_OP_LONG1 | PICKLE_OP_LONG4 => {
                    let size = if op == PICKLE_OP_LONG1 {
                        match pickle_read_u8(data.as_slice(), &mut idx, _py) {
                            Ok(v) => v as usize,
                            Err(err_bits) => return err_bits,
                        }
                    } else {
                        match pickle_read_u32_le(data.as_slice(), &mut idx, _py) {
                            Ok(v) => v as usize,
                            Err(err_bits) => return err_bits,
                        }
                    };
                    let raw = match pickle_read_exact(data.as_slice(), &mut idx, size, _py) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let bits = match pickle_parse_long_bytes_bits(_py, raw) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    stack.push(PickleVmItem::Value(bits));
                }
                PICKLE_OP_FLOAT => {
                    let line = match pickle_read_line_bytes(data.as_slice(), &mut idx, _py) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let line_text = match std::str::from_utf8(line) {
                        Ok(text) => text,
                        Err(_) => return pickle_raise(_py, "pickle.loads: invalid FLOAT payload"),
                    };
                    let bits = match pickle_parse_float_bits(_py, line_text) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    stack.push(PickleVmItem::Value(bits));
                }
                PICKLE_OP_BINFLOAT => {
                    let value = match pickle_read_f64_be(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    stack.push(PickleVmItem::Value(MoltObject::from_float(value).bits()));
                }
                PICKLE_OP_STRING => {
                    let line = match pickle_read_line_bytes(data.as_slice(), &mut idx, _py) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let text = match std::str::from_utf8(line) {
                        Ok(v) => v,
                        Err(_) => return pickle_raise(_py, "pickle.loads: invalid STRING payload"),
                    };
                    let parsed = match pickle_parse_string_literal(text) {
                        Ok(v) => v,
                        Err(message) => return pickle_raise(_py, message),
                    };
                    let ptr = alloc_string(_py, parsed.as_bytes());
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(MoltObject::from_ptr(ptr).bits()));
                }
                PICKLE_OP_UNICODE => {
                    let line = match pickle_read_line_bytes(data.as_slice(), &mut idx, _py) {
                        Ok(value) => value,
                        Err(err_bits) => return err_bits,
                    };
                    let text = match pickle_decode_utf8(_py, line, "UNICODE payload") {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let ptr = alloc_string(_py, text.as_bytes());
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(MoltObject::from_ptr(ptr).bits()));
                }
                PICKLE_OP_BINUNICODE | PICKLE_OP_SHORT_BINUNICODE => {
                    let size = if op == PICKLE_OP_SHORT_BINUNICODE {
                        match pickle_read_u8(data.as_slice(), &mut idx, _py) {
                            Ok(v) => v as usize,
                            Err(err_bits) => return err_bits,
                        }
                    } else {
                        match pickle_read_u32_le(data.as_slice(), &mut idx, _py) {
                            Ok(v) => v as usize,
                            Err(err_bits) => return err_bits,
                        }
                    };
                    let raw = match pickle_read_exact(data.as_slice(), &mut idx, size, _py) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let text = match pickle_decode_utf8(_py, raw, "BINUNICODE payload") {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let ptr = alloc_string(_py, text.as_bytes());
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(MoltObject::from_ptr(ptr).bits()));
                }
                PICKLE_OP_SHORT_BINBYTES | PICKLE_OP_BINBYTES | PICKLE_OP_BINBYTES8 => {
                    let size = match op {
                        PICKLE_OP_SHORT_BINBYTES => {
                            match pickle_read_u8(data.as_slice(), &mut idx, _py) {
                                Ok(v) => v as usize,
                                Err(err_bits) => return err_bits,
                            }
                        }
                        PICKLE_OP_BINBYTES => {
                            match pickle_read_u32_le(data.as_slice(), &mut idx, _py) {
                                Ok(v) => v as usize,
                                Err(err_bits) => return err_bits,
                            }
                        }
                        _ => match pickle_read_u64_le(data.as_slice(), &mut idx, _py) {
                            Ok(v) => v as usize,
                            Err(err_bits) => return err_bits,
                        },
                    };
                    let raw = match pickle_read_exact(data.as_slice(), &mut idx, size, _py) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let ptr = crate::alloc_bytes(_py, raw);
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(MoltObject::from_ptr(ptr).bits()));
                }
                PICKLE_OP_BYTEARRAY8 => {
                    let size = match pickle_read_u64_le(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v as usize,
                        Err(err_bits) => return err_bits,
                    };
                    let raw = match pickle_read_exact(data.as_slice(), &mut idx, size, _py) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let bytes_ptr = crate::alloc_bytes(_py, raw);
                    if bytes_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    let bytes_bits = MoltObject::from_ptr(bytes_ptr).bits();
                    let out_bits =
                        pickle_call_with_args(_py, builtin_classes(_py).bytearray, &[bytes_bits]);
                    dec_ref_bits(_py, bytes_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(out_bits));
                }
                PICKLE_OP_EMPTY_TUPLE => {
                    let ptr = alloc_tuple(_py, &[]);
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(MoltObject::from_ptr(ptr).bits()));
                }
                PICKLE_OP_TUPLE1 | PICKLE_OP_TUPLE2 | PICKLE_OP_TUPLE3 => {
                    let needed = if op == PICKLE_OP_TUPLE1 {
                        1
                    } else if op == PICKLE_OP_TUPLE2 {
                        2
                    } else {
                        3
                    };
                    let mut items: Vec<u64> = Vec::with_capacity(needed);
                    for _ in 0..needed {
                        let bits = match pickle_vm_pop_value(_py, &mut stack) {
                            Ok(v) => v,
                            Err(err_bits) => return err_bits,
                        };
                        items.push(bits);
                    }
                    items.reverse();
                    let ptr = alloc_tuple(_py, items.as_slice());
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(MoltObject::from_ptr(ptr).bits()));
                }
                PICKLE_OP_TUPLE => {
                    let items = match pickle_vm_pop_mark_items(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let mut values: Vec<u64> = Vec::with_capacity(items.len());
                    for item in items {
                        let bits = match pickle_vm_item_to_bits(_py, &item) {
                            Ok(v) => v,
                            Err(err_bits) => return err_bits,
                        };
                        values.push(bits);
                    }
                    let ptr = alloc_tuple(_py, values.as_slice());
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(MoltObject::from_ptr(ptr).bits()));
                }
                PICKLE_OP_EMPTY_LIST => {
                    let ptr = alloc_list_with_capacity(_py, &[], 0);
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(MoltObject::from_ptr(ptr).bits()));
                }
                PICKLE_OP_LIST => {
                    let items = match pickle_vm_pop_mark_items(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let mut values: Vec<u64> = Vec::with_capacity(items.len());
                    for item in items {
                        let bits = match pickle_vm_item_to_bits(_py, &item) {
                            Ok(v) => v,
                            Err(err_bits) => return err_bits,
                        };
                        values.push(bits);
                    }
                    let ptr = alloc_list_with_capacity(_py, values.as_slice(), values.len());
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(MoltObject::from_ptr(ptr).bits()));
                }
                PICKLE_OP_APPEND => {
                    let item_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let list_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let _ = crate::molt_list_append(list_bits, item_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(list_bits));
                }
                PICKLE_OP_APPENDS => {
                    let items = match pickle_vm_pop_mark_items(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let list_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    for item in items {
                        let bits = match pickle_vm_item_to_bits(_py, &item) {
                            Ok(v) => v,
                            Err(err_bits) => return err_bits,
                        };
                        let _ = crate::molt_list_append(list_bits, bits);
                        if exception_pending(_py) {
                            return MoltObject::none().bits();
                        }
                    }
                    stack.push(PickleVmItem::Value(list_bits));
                }
                PICKLE_OP_EMPTY_DICT => {
                    let ptr = alloc_dict_with_pairs(_py, &[]);
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(MoltObject::from_ptr(ptr).bits()));
                }
                PICKLE_OP_DICT => {
                    let items = match pickle_vm_pop_mark_items(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let mut values: Vec<u64> = Vec::with_capacity(items.len());
                    for item in items {
                        let bits = match pickle_vm_item_to_bits(_py, &item) {
                            Ok(v) => v,
                            Err(err_bits) => return err_bits,
                        };
                        values.push(bits);
                    }
                    if !values.len().is_multiple_of(2) {
                        return pickle_raise(_py, "pickle.loads: dict has odd number of items");
                    }
                    let ptr = alloc_dict_with_pairs(_py, values.as_slice());
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(MoltObject::from_ptr(ptr).bits()));
                }
                PICKLE_OP_SETITEM => {
                    let value_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let key_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let dict_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                        return pickle_raise(_py, "pickle.loads: setitem target is not dict");
                    };
                    if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
                        return pickle_raise(_py, "pickle.loads: setitem target is not dict");
                    }
                    unsafe {
                        crate::dict_set_in_place(_py, dict_ptr, key_bits, value_bits);
                    }
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(dict_bits));
                }
                PICKLE_OP_SETITEMS => {
                    let items = match pickle_vm_pop_mark_items(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let dict_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                        return pickle_raise(_py, "pickle.loads: setitems target is not dict");
                    };
                    if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
                        return pickle_raise(_py, "pickle.loads: setitems target is not dict");
                    }
                    let mut values: Vec<u64> = Vec::with_capacity(items.len());
                    for item in items {
                        let bits = match pickle_vm_item_to_bits(_py, &item) {
                            Ok(v) => v,
                            Err(err_bits) => return err_bits,
                        };
                        values.push(bits);
                    }
                    if !values.len().is_multiple_of(2) {
                        return pickle_raise(
                            _py,
                            "pickle.loads: setitems has odd number of values",
                        );
                    }
                    let mut pair_idx = 0usize;
                    while pair_idx + 1 < values.len() {
                        unsafe {
                            crate::dict_set_in_place(
                                _py,
                                dict_ptr,
                                values[pair_idx],
                                values[pair_idx + 1],
                            );
                        }
                        if exception_pending(_py) {
                            return MoltObject::none().bits();
                        }
                        pair_idx += 2;
                    }
                    stack.push(PickleVmItem::Value(dict_bits));
                }
                PICKLE_OP_EMPTY_SET => {
                    let bits = crate::molt_set_new(0);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(bits));
                }
                PICKLE_OP_ADDITEMS => {
                    let items = match pickle_vm_pop_mark_items(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let set_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let Some(set_ptr) = obj_from_bits(set_bits).as_ptr() else {
                        return pickle_raise(_py, "pickle.loads: additems target is not set");
                    };
                    for item in items {
                        let bits = match pickle_vm_item_to_bits(_py, &item) {
                            Ok(v) => v,
                            Err(err_bits) => return err_bits,
                        };
                        unsafe {
                            crate::set_add_in_place(_py, set_ptr, bits);
                        }
                        if exception_pending(_py) {
                            return MoltObject::none().bits();
                        }
                    }
                    stack.push(PickleVmItem::Value(set_bits));
                }
                PICKLE_OP_FROZENSET => {
                    let items = match pickle_vm_pop_mark_items(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let mut values: Vec<u64> = Vec::with_capacity(items.len());
                    for item in items {
                        let bits = match pickle_vm_item_to_bits(_py, &item) {
                            Ok(v) => v,
                            Err(err_bits) => return err_bits,
                        };
                        values.push(bits);
                    }
                    let list_ptr = alloc_list_with_capacity(_py, values.as_slice(), values.len());
                    if list_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    let list_bits = MoltObject::from_ptr(list_ptr).bits();
                    let tuple_ptr = alloc_tuple(_py, &[list_bits]);
                    dec_ref_bits(_py, list_bits);
                    if tuple_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    let args_bits = MoltObject::from_ptr(tuple_ptr).bits();
                    let out_bits =
                        pickle_apply_reduce_bits(_py, builtin_classes(_py).frozenset, args_bits);
                    dec_ref_bits(_py, args_bits);
                    match out_bits {
                        Ok(bits) => stack.push(PickleVmItem::Value(bits)),
                        Err(err_bits) => return err_bits,
                    }
                }
                PICKLE_OP_GLOBAL => {
                    let module = match pickle_read_line_bytes(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let name = match pickle_read_line_bytes(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let module_text = match pickle_decode_utf8(_py, module, "GLOBAL module") {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let name_text = match pickle_decode_utf8(_py, name, "GLOBAL name") {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    if let Some(global) = pickle_resolve_global(&module_text, &name_text) {
                        stack.push(PickleVmItem::Global(global));
                    } else {
                        match pickle_resolve_global_with_hook(
                            _py,
                            &module_text,
                            &name_text,
                            find_class,
                        ) {
                            Ok(bits) => stack.push(PickleVmItem::Value(bits)),
                            Err(err_bits) => return err_bits,
                        }
                    }
                }
                PICKLE_OP_STACK_GLOBAL => {
                    let name_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let module_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let Some(module) = string_obj_to_owned(obj_from_bits(module_bits)) else {
                        return pickle_raise(_py, "pickle.loads: STACK_GLOBAL module must be str");
                    };
                    let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
                        return pickle_raise(_py, "pickle.loads: STACK_GLOBAL name must be str");
                    };
                    if let Some(global) = pickle_resolve_global(&module, &name) {
                        stack.push(PickleVmItem::Global(global));
                    } else {
                        match pickle_resolve_global_with_hook(_py, &module, &name, find_class) {
                            Ok(bits) => stack.push(PickleVmItem::Value(bits)),
                            Err(err_bits) => return err_bits,
                        }
                    }
                }
                PICKLE_OP_REDUCE => {
                    let args_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let callable_item = match stack.pop() {
                        Some(v) => v,
                        None => return pickle_raise(_py, "pickle.loads: stack underflow"),
                    };
                    match pickle_apply_reduce_vm(_py, callable_item, args_bits) {
                        Ok(bits) => stack.push(PickleVmItem::Value(bits)),
                        Err(err_bits) => return err_bits,
                    }
                }
                PICKLE_OP_NEWOBJ => {
                    let args_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let cls_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    match pickle_apply_newobj(_py, cls_bits, args_bits, None) {
                        Ok(bits) => stack.push(PickleVmItem::Value(bits)),
                        Err(err_bits) => return err_bits,
                    }
                }
                PICKLE_OP_NEWOBJ_EX => {
                    let kwargs_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let args_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let cls_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    match pickle_apply_newobj(_py, cls_bits, args_bits, Some(kwargs_bits)) {
                        Ok(bits) => stack.push(PickleVmItem::Value(bits)),
                        Err(err_bits) => return err_bits,
                    }
                }
                PICKLE_OP_BUILD => {
                    let state_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let inst_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    match pickle_apply_build(_py, inst_bits, state_bits) {
                        Ok(bits) => stack.push(PickleVmItem::Value(bits)),
                        Err(err_bits) => return err_bits,
                    }
                }
                PICKLE_OP_PUT => {
                    let line = match pickle_read_line_bytes(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let text = match std::str::from_utf8(line) {
                        Ok(v) => v,
                        Err(_) => return pickle_raise(_py, "pickle.loads: invalid PUT payload"),
                    };
                    let index = match text.parse::<usize>() {
                        Ok(v) => v,
                        Err(_) => return pickle_raise(_py, "pickle.loads: invalid memo key"),
                    };
                    let item = match stack.last() {
                        Some(v) => v.clone(),
                        None => return pickle_raise(_py, "pickle.loads: stack underflow"),
                    };
                    pickle_memo_set(_py, &mut memo, index, item);
                }
                PICKLE_OP_BINPUT => {
                    let index = match pickle_read_u8(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v as usize,
                        Err(err_bits) => return err_bits,
                    };
                    let item = match stack.last() {
                        Some(v) => v.clone(),
                        None => return pickle_raise(_py, "pickle.loads: stack underflow"),
                    };
                    pickle_memo_set(_py, &mut memo, index, item);
                }
                PICKLE_OP_LONG_BINPUT => {
                    let index = match pickle_read_u32_le(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v as usize,
                        Err(err_bits) => return err_bits,
                    };
                    let item = match stack.last() {
                        Some(v) => v.clone(),
                        None => return pickle_raise(_py, "pickle.loads: stack underflow"),
                    };
                    pickle_memo_set(_py, &mut memo, index, item);
                }
                PICKLE_OP_MEMOIZE => {
                    let item = match stack.last() {
                        Some(v) => v.clone(),
                        None => return pickle_raise(_py, "pickle.loads: stack underflow"),
                    };
                    memo.push(Some(item));
                }
                PICKLE_OP_GET => {
                    let line = match pickle_read_line_bytes(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let text = match std::str::from_utf8(line) {
                        Ok(v) => v,
                        Err(_) => return pickle_raise(_py, "pickle.loads: invalid GET payload"),
                    };
                    let index = match text.parse::<usize>() {
                        Ok(v) => v,
                        Err(_) => return pickle_raise(_py, "pickle.loads: invalid memo key"),
                    };
                    match pickle_memo_get(_py, memo.as_slice(), index) {
                        Ok(item) => stack.push(item),
                        Err(err_bits) => return err_bits,
                    }
                }
                PICKLE_OP_BINGET => {
                    let index = match pickle_read_u8(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v as usize,
                        Err(err_bits) => return err_bits,
                    };
                    match pickle_memo_get(_py, memo.as_slice(), index) {
                        Ok(item) => stack.push(item),
                        Err(err_bits) => return err_bits,
                    }
                }
                PICKLE_OP_LONG_BINGET => {
                    let index = match pickle_read_u32_le(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v as usize,
                        Err(err_bits) => return err_bits,
                    };
                    match pickle_memo_get(_py, memo.as_slice(), index) {
                        Ok(item) => stack.push(item),
                        Err(err_bits) => return err_bits,
                    }
                }
                PICKLE_OP_PERSID => {
                    let pid_line = match pickle_read_line_bytes(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let pid_text = match pickle_decode_utf8(_py, pid_line, "PERSID payload") {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let Some(pid_bits) = alloc_string_bits(_py, pid_text.as_str()) else {
                        return MoltObject::none().bits();
                    };
                    let Some(persistent_load_bits) = persistent_load else {
                        dec_ref_bits(_py, pid_bits);
                        return pickle_raise(
                            _py,
                            "pickle.loads: persistent IDs require persistent_load",
                        );
                    };
                    let value_bits = unsafe { call_callable1(_py, persistent_load_bits, pid_bits) };
                    dec_ref_bits(_py, pid_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(value_bits));
                }
                PICKLE_OP_BINPERSID => {
                    let pid_bits = match pickle_vm_pop_value(_py, &mut stack) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let Some(persistent_load_bits) = persistent_load else {
                        return pickle_raise(
                            _py,
                            "pickle.loads: persistent IDs require persistent_load",
                        );
                    };
                    let value_bits = unsafe { call_callable1(_py, persistent_load_bits, pid_bits) };
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    stack.push(PickleVmItem::Value(value_bits));
                }
                PICKLE_OP_EXT1 | PICKLE_OP_EXT2 | PICKLE_OP_EXT4 => {
                    let code = if op == PICKLE_OP_EXT1 {
                        match pickle_read_u8(data.as_slice(), &mut idx, _py) {
                            Ok(v) => v as i64,
                            Err(err_bits) => return err_bits,
                        }
                    } else if op == PICKLE_OP_EXT2 {
                        match pickle_read_u16_le(data.as_slice(), &mut idx, _py) {
                            Ok(v) => v as i64,
                            Err(err_bits) => return err_bits,
                        }
                    } else {
                        match pickle_read_u32_le(data.as_slice(), &mut idx, _py) {
                            Ok(v) => v as i64,
                            Err(err_bits) => return err_bits,
                        }
                    };
                    match pickle_lookup_extension_bits(_py, code, find_class) {
                        Ok(bits) => stack.push(PickleVmItem::Value(bits)),
                        Err(err_bits) => return err_bits,
                    }
                }
                // Python 2 string opcodes.
                b'U' => {
                    let size = match pickle_read_u8(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v as usize,
                        Err(err_bits) => return err_bits,
                    };
                    let raw = match pickle_read_exact(data.as_slice(), &mut idx, size, _py) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let bits = match pickle_decode_8bit_string(_py, raw, &encoding, &errors) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    stack.push(PickleVmItem::Value(bits));
                }
                b'T' => {
                    let size = match pickle_read_u32_le(data.as_slice(), &mut idx, _py) {
                        Ok(v) => v as usize,
                        Err(err_bits) => return err_bits,
                    };
                    let raw = match pickle_read_exact(data.as_slice(), &mut idx, size, _py) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    let bits = match pickle_decode_8bit_string(_py, raw, &encoding, &errors) {
                        Ok(v) => v,
                        Err(err_bits) => return err_bits,
                    };
                    stack.push(PickleVmItem::Value(bits));
                }
                _ => {
                    let msg = format!("pickle.loads: unsupported opcode 0x{op:02x}");
                    return pickle_raise(_py, msg.as_str());
                }
            }
        }
        let Some(item) = stack.last() else {
            return pickle_raise(_py, "pickle.loads: pickle stack empty");
        };
        match pickle_vm_item_to_bits(_py, item) {
            Ok(bits) => bits,
            Err(err_bits) => err_bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_multiprocessing_codec_dumps(obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let protocol_bits = MoltObject::from_int(PICKLE_PROTO_5).bits();
        let true_bits = MoltObject::from_bool(true).bits();
        let none_bits = MoltObject::none().bits();
        molt_pickle_dumps_core(
            obj_bits,
            protocol_bits,
            true_bits,
            none_bits,
            none_bits,
            none_bits,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_multiprocessing_codec_loads(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let true_bits = MoltObject::from_bool(true).bits();
        let none_bits = MoltObject::none().bits();
        let encoding_ptr = alloc_string(_py, b"ASCII");
        if encoding_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let errors_ptr = alloc_string(_py, b"strict");
        if errors_ptr.is_null() {
            dec_ref_bits(_py, MoltObject::from_ptr(encoding_ptr).bits());
            return MoltObject::none().bits();
        }
        let encoding_bits = MoltObject::from_ptr(encoding_ptr).bits();
        let errors_bits = MoltObject::from_ptr(errors_ptr).bits();
        let out_bits = molt_pickle_loads_core(
            data_bits,
            true_bits,
            encoding_bits,
            errors_bits,
            none_bits,
            none_bits,
            none_bits,
        );
        dec_ref_bits(_py, encoding_bits);
        dec_ref_bits(_py, errors_bits);
        out_bits
    })
}
