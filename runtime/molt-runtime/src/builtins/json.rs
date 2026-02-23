use crate::arena::TempArena;
use crate::*;
use std::fmt::Write as _;
use std::io::Cursor;

// --- JSON ---

/// # Safety
/// Dereferences raw pointers. Caller must ensure ptr is valid UTF-8 of at least len bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_json_parse_int(ptr: *const u8, len_bits: u64) -> i64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let len = usize_from_bits(len_bits);
            let s = {
                let slice = std::slice::from_raw_parts(ptr, len);
                std::str::from_utf8(slice).unwrap()
            };
            let v: serde_json::Value = serde_json::from_str(s).unwrap();
            v.as_i64().unwrap_or(0)
        })
    }
}

fn json_escape_codepoint(code: u32, out: &mut String) {
    if code <= 0xFFFF {
        let _ = write!(out, "\\u{code:04x}");
        return;
    }
    let adjusted = code - 0x10000;
    let high = 0xD800 + ((adjusted >> 10) & 0x3FF);
    let low = 0xDC00 + (adjusted & 0x3FF);
    let _ = write!(out, "\\u{high:04x}\\u{low:04x}");
}

fn json_encode_basestring_impl(value: &str, ensure_ascii: bool) -> String {
    let mut out = String::with_capacity(value.len().saturating_add(8));
    out.push('"');
    for ch in value.chars() {
        let code = ch as u32;
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => {
                if code < 0x20 || (ensure_ascii && code > 0x7E) {
                    json_escape_codepoint(code, &mut out);
                } else {
                    out.push(ch);
                }
            }
        }
    }
    out.push('"');
    out
}

fn json_string_line_col(text: &str, pos: usize) -> (usize, usize) {
    let mut lineno = 1usize;
    let mut colno = 1usize;
    for (idx, ch) in text.chars().enumerate() {
        if idx >= pos {
            break;
        }
        if ch == '\n' {
            lineno += 1;
            colno = 1;
        } else {
            colno += 1;
        }
    }
    (lineno, colno)
}

fn json_scanstring_decode(
    text: &str,
    end: usize,
    strict: bool,
) -> Result<(String, usize), (String, usize)> {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    if end > len {
        return Err(("end is out of bounds".to_string(), end));
    }
    let mut idx = end;
    let mut out = String::new();
    while idx < len {
        let ch = chars[idx];
        if ch == '"' {
            return Ok((out, idx + 1));
        }
        if ch == '\\' {
            idx += 1;
            if idx >= len {
                let start = end.saturating_sub(1);
                return Err(("Unterminated string starting at".to_string(), start));
            }
            let esc = chars[idx];
            match esc {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                '/' => out.push('/'),
                'b' => out.push('\u{08}'),
                'f' => out.push('\u{0C}'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'u' => {
                    let hex_start = idx + 1;
                    if hex_start + 4 > len {
                        return Err(("Invalid \\uXXXX escape".to_string(), idx));
                    }
                    let mut code: u32 = 0;
                    for c in &chars[hex_start..hex_start + 4] {
                        let Some(digit) = c.to_digit(16) else {
                            return Err(("Invalid \\uXXXX escape".to_string(), idx));
                        };
                        code = (code << 4) | digit;
                    }
                    idx += 4;
                    if (0xD800..=0xDBFF).contains(&code)
                        && idx + 6 <= len
                        && chars[idx + 1] == '\\'
                        && chars[idx + 2] == 'u'
                    {
                        let mut low: u32 = 0;
                        let mut valid = true;
                        for c in &chars[idx + 3..idx + 7] {
                            if let Some(d) = c.to_digit(16) {
                                low = (low << 4) | d;
                            } else {
                                valid = false;
                                break;
                            }
                        }
                        if valid && (0xDC00..=0xDFFF).contains(&low) {
                            let combined = 0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00);
                            if let Some(real) = char::from_u32(combined) {
                                out.push(real);
                                idx += 6;
                                idx += 1;
                                continue;
                            }
                        }
                    }
                    if let Some(real) = char::from_u32(code) {
                        out.push(real);
                    } else {
                        return Err(("Invalid \\uXXXX escape".to_string(), idx));
                    }
                }
                _ => return Err(("Invalid \\uXXXX escape".to_string(), idx)),
            }
            idx += 1;
            continue;
        }
        if strict && (ch as u32) < 0x20 {
            return Err(("Invalid control character at".to_string(), idx));
        }
        out.push(ch);
        idx += 1;
    }
    let start = end.saturating_sub(1);
    Err(("Unterminated string starting at".to_string(), start))
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_json_encode_basestring_obj(obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = string_obj_to_owned(obj_from_bits(obj_bits)) else {
            let type_name = type_name(_py, obj_from_bits(obj_bits));
            let msg = format!("first argument must be a string, not {type_name}");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        let encoded = json_encode_basestring_impl(value.as_str(), false);
        let ptr = alloc_string(_py, encoded.as_bytes());
        if ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "failed to allocate string");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_json_encode_basestring_ascii_obj(obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = string_obj_to_owned(obj_from_bits(obj_bits)) else {
            let type_name = type_name(_py, obj_from_bits(obj_bits));
            let msg = format!("first argument must be a string, not {type_name}");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        let encoded = json_encode_basestring_impl(value.as_str(), true);
        let ptr = alloc_string(_py, encoded.as_bytes());
        if ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "failed to allocate string");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_json_scanstring_obj(text_bits: u64, end_bits: u64, strict_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            let type_name = type_name(_py, obj_from_bits(text_bits));
            let msg = format!("first argument must be a string, not {type_name}");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        let Some(end_i64) = to_i64(obj_from_bits(end_bits)) else {
            let type_name = type_name(_py, obj_from_bits(end_bits));
            let msg = format!("'{type_name}' object cannot be interpreted as an integer");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        if end_i64 < 0 {
            return raise_exception::<_>(_py, "ValueError", "end is out of bounds");
        }
        let strict = is_truthy(_py, obj_from_bits(strict_bits));
        let end = end_i64 as usize;
        match json_scanstring_decode(text.as_str(), end, strict) {
            Ok((decoded, idx)) => {
                let decoded_ptr = alloc_string(_py, decoded.as_bytes());
                if decoded_ptr.is_null() {
                    return raise_exception::<_>(_py, "MemoryError", "failed to allocate string");
                }
                let tuple_ptr = alloc_tuple(
                    _py,
                    &[
                        MoltObject::from_ptr(decoded_ptr).bits(),
                        MoltObject::from_int(idx as i64).bits(),
                    ],
                );
                if tuple_ptr.is_null() {
                    return raise_exception::<_>(_py, "MemoryError", "failed to allocate tuple");
                }
                MoltObject::from_ptr(tuple_ptr).bits()
            }
            Err((msg, pos)) => {
                if msg == "end is out of bounds" {
                    return raise_exception::<_>(_py, "ValueError", msg.as_str());
                }
                let (lineno, colno) = json_string_line_col(text.as_str(), pos);
                let detail = format!("{msg}: line {lineno} column {colno} (char {pos})");
                raise_exception::<_>(_py, "ValueError", detail.as_str())
            }
        }
    })
}

fn value_to_object(
    _py: &PyToken<'_>,
    value: serde_json::Value,
    arena: &mut TempArena,
) -> Result<MoltObject, i32> {
    match value {
        serde_json::Value::Null => Ok(MoltObject::none()),
        serde_json::Value::Bool(b) => Ok(MoltObject::from_bool(b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(MoltObject::from_int(i))
            } else if let Some(f) = n.as_f64() {
                Ok(MoltObject::from_float(f))
            } else {
                Err(2)
            }
        }
        serde_json::Value::String(s) => {
            let ptr = alloc_string(_py, s.as_bytes());
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        serde_json::Value::Array(items) => {
            if items.len() > MAX_SMALL_LIST {
                return Err(2);
            }
            if items.is_empty() {
                let ptr = alloc_list(_py, &[]);
                if ptr.is_null() {
                    return Err(2);
                }
                return Ok(MoltObject::from_ptr(ptr));
            }
            let len = items.len();
            let elems_ptr = arena.alloc_slice::<u64>(len);
            if elems_ptr.is_null() {
                return Err(2);
            }
            for (idx, item) in items.into_iter().enumerate() {
                let obj = value_to_object(_py, item, arena)?;
                unsafe {
                    *elems_ptr.add(idx) = obj.bits();
                }
            }
            let elems = unsafe { std::slice::from_raw_parts(elems_ptr, len) };
            let ptr = alloc_list(_py, elems);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        serde_json::Value::Object(map) => {
            if map.len() > MAX_SMALL_LIST {
                return Err(2);
            }
            if map.is_empty() {
                let ptr = alloc_dict_with_pairs(_py, &[]);
                if ptr.is_null() {
                    return Err(2);
                }
                return Ok(MoltObject::from_ptr(ptr));
            }
            let len = map.len();
            let pairs_ptr = arena.alloc_slice::<u64>(len * 2);
            if pairs_ptr.is_null() {
                return Err(2);
            }
            for (idx, (key, value)) in map.into_iter().enumerate() {
                let key_ptr = alloc_string(_py, key.as_bytes());
                if key_ptr.is_null() {
                    return Err(2);
                }
                let val_obj = value_to_object(_py, value, arena)?;
                unsafe {
                    *pairs_ptr.add(idx * 2) = MoltObject::from_ptr(key_ptr).bits();
                    *pairs_ptr.add(idx * 2 + 1) = val_obj.bits();
                }
            }
            let pairs = unsafe { std::slice::from_raw_parts(pairs_ptr, len * 2) };
            let ptr = alloc_dict_with_pairs(_py, pairs);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
    }
}

fn msgpack_value_to_object(
    _py: &PyToken<'_>,
    value: rmpv::Value,
    arena: &mut TempArena,
) -> Result<MoltObject, i32> {
    match value {
        rmpv::Value::Nil => Ok(MoltObject::none()),
        rmpv::Value::Boolean(b) => Ok(MoltObject::from_bool(b)),
        rmpv::Value::Integer(i) => {
            if let Some(v) = i.as_i64() {
                Ok(MoltObject::from_int(v))
            } else if let Some(v) = i.as_u64() {
                if v <= i64::MAX as u64 {
                    Ok(MoltObject::from_int(v as i64))
                } else {
                    Err(2)
                }
            } else {
                Err(2)
            }
        }
        rmpv::Value::F32(f) => Ok(MoltObject::from_float(f as f64)),
        rmpv::Value::F64(f) => Ok(MoltObject::from_float(f)),
        rmpv::Value::String(s) => {
            let s = s.as_str().ok_or(2)?;
            let ptr = alloc_string(_py, s.as_bytes());
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        rmpv::Value::Binary(b) => {
            let ptr = alloc_bytes(_py, &b);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        rmpv::Value::Array(items) => {
            if items.len() > MAX_SMALL_LIST {
                return Err(2);
            }
            if items.is_empty() {
                let ptr = alloc_list(_py, &[]);
                if ptr.is_null() {
                    return Err(2);
                }
                return Ok(MoltObject::from_ptr(ptr));
            }
            let len = items.len();
            let elems_ptr = arena.alloc_slice::<u64>(len);
            if elems_ptr.is_null() {
                return Err(2);
            }
            for (idx, item) in items.into_iter().enumerate() {
                let obj = msgpack_value_to_object(_py, item, arena)?;
                unsafe {
                    *elems_ptr.add(idx) = obj.bits();
                }
            }
            let elems = unsafe { std::slice::from_raw_parts(elems_ptr, len) };
            let ptr = alloc_list(_py, elems);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        rmpv::Value::Map(items) => {
            if items.len() > MAX_SMALL_LIST {
                return Err(2);
            }
            if items.is_empty() {
                let ptr = alloc_dict_with_pairs(_py, &[]);
                if ptr.is_null() {
                    return Err(2);
                }
                return Ok(MoltObject::from_ptr(ptr));
            }
            let len = items.len();
            let pairs_ptr = arena.alloc_slice::<u64>(len * 2);
            if pairs_ptr.is_null() {
                return Err(2);
            }
            for (idx, (key, value)) in items.into_iter().enumerate() {
                let key_obj = msgpack_key_to_object(_py, key)?;
                let val_obj = msgpack_value_to_object(_py, value, arena)?;
                unsafe {
                    *pairs_ptr.add(idx * 2) = key_obj.bits();
                    *pairs_ptr.add(idx * 2 + 1) = val_obj.bits();
                }
            }
            let pairs = unsafe { std::slice::from_raw_parts(pairs_ptr, len * 2) };
            let ptr = alloc_dict_with_pairs(_py, pairs);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        _ => Err(2),
    }
}

fn msgpack_key_to_object(_py: &PyToken<'_>, value: rmpv::Value) -> Result<MoltObject, i32> {
    match value {
        rmpv::Value::Nil => Ok(MoltObject::none()),
        rmpv::Value::Boolean(b) => Ok(MoltObject::from_bool(b)),
        rmpv::Value::Integer(i) => {
            if let Some(v) = i.as_i64() {
                Ok(MoltObject::from_int(v))
            } else if let Some(v) = i.as_u64() {
                if v <= i64::MAX as u64 {
                    Ok(MoltObject::from_int(v as i64))
                } else {
                    Err(2)
                }
            } else {
                Err(2)
            }
        }
        rmpv::Value::String(s) => {
            let ptr = alloc_string(_py, s.as_bytes());
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        rmpv::Value::Binary(b) => {
            let ptr = alloc_bytes(_py, &b);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        _ => Err(2),
    }
}

fn cbor_value_to_object(
    _py: &PyToken<'_>,
    value: serde_cbor::Value,
    arena: &mut TempArena,
) -> Result<MoltObject, i32> {
    match value {
        serde_cbor::Value::Null => Ok(MoltObject::none()),
        serde_cbor::Value::Bool(b) => Ok(MoltObject::from_bool(b)),
        serde_cbor::Value::Integer(i) => {
            if i < i64::MIN as i128 || i > i64::MAX as i128 {
                return Err(2);
            }
            Ok(MoltObject::from_int(i as i64))
        }
        serde_cbor::Value::Float(f) => Ok(MoltObject::from_float(f)),
        serde_cbor::Value::Text(s) => {
            let ptr = alloc_string(_py, s.as_bytes());
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        serde_cbor::Value::Bytes(b) => {
            let ptr = alloc_bytes(_py, &b);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        serde_cbor::Value::Array(items) => {
            if items.len() > MAX_SMALL_LIST {
                return Err(2);
            }
            if items.is_empty() {
                let ptr = alloc_list(_py, &[]);
                if ptr.is_null() {
                    return Err(2);
                }
                return Ok(MoltObject::from_ptr(ptr));
            }
            let len = items.len();
            let elems_ptr = arena.alloc_slice::<u64>(len);
            if elems_ptr.is_null() {
                return Err(2);
            }
            for (idx, item) in items.into_iter().enumerate() {
                let obj = cbor_value_to_object(_py, item, arena)?;
                unsafe {
                    *elems_ptr.add(idx) = obj.bits();
                }
            }
            let elems = unsafe { std::slice::from_raw_parts(elems_ptr, len) };
            let ptr = alloc_list(_py, elems);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        serde_cbor::Value::Map(items) => {
            if items.len() > MAX_SMALL_LIST {
                return Err(2);
            }
            if items.is_empty() {
                let ptr = alloc_dict_with_pairs(_py, &[]);
                if ptr.is_null() {
                    return Err(2);
                }
                return Ok(MoltObject::from_ptr(ptr));
            }
            let len = items.len();
            let pairs_ptr = arena.alloc_slice::<u64>(len * 2);
            if pairs_ptr.is_null() {
                return Err(2);
            }
            for (idx, (key, value)) in items.into_iter().enumerate() {
                let key_obj = cbor_key_to_object(_py, key)?;
                let val_obj = cbor_value_to_object(_py, value, arena)?;
                unsafe {
                    *pairs_ptr.add(idx * 2) = key_obj.bits();
                    *pairs_ptr.add(idx * 2 + 1) = val_obj.bits();
                }
            }
            let pairs = unsafe { std::slice::from_raw_parts(pairs_ptr, len * 2) };
            let ptr = alloc_dict_with_pairs(_py, pairs);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        _ => Err(2),
    }
}

fn cbor_key_to_object(_py: &PyToken<'_>, value: serde_cbor::Value) -> Result<MoltObject, i32> {
    match value {
        serde_cbor::Value::Null => Ok(MoltObject::none()),
        serde_cbor::Value::Bool(b) => Ok(MoltObject::from_bool(b)),
        serde_cbor::Value::Integer(i) => {
            let i_val = i;
            if i_val < i64::MIN as i128 || i_val > i64::MAX as i128 {
                Err(2)
            } else {
                Ok(MoltObject::from_int(i_val as i64))
            }
        }
        serde_cbor::Value::Text(s) => {
            let ptr = alloc_string(_py, s.as_bytes());
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        serde_cbor::Value::Bytes(b) => {
            let ptr = alloc_bytes(_py, &b);
            if ptr.is_null() {
                Err(2)
            } else {
                Ok(MoltObject::from_ptr(ptr))
            }
        }
        _ => Err(2),
    }
}

unsafe fn parse_json_scalar(
    _py: &PyToken<'_>,
    ptr: *const u8,
    len: usize,
    arena: &mut TempArena,
) -> Result<MoltObject, i32> {
    unsafe {
        let slice = std::slice::from_raw_parts(ptr, len);
        let s = std::str::from_utf8(slice).map_err(|_| 1)?;
        let v: serde_json::Value = serde_json::from_str(s).map_err(|_| 1)?;
        value_to_object(_py, v, arena)
    }
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure ptr is valid UTF-8 of at least len bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_json_parse_scalar(
    ptr: *const u8,
    len_bits: u64,
    out: *mut u64,
) -> i32 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let len = usize_from_bits(len_bits);
            if out.is_null() {
                return 2;
            }
            let obj = PARSE_ARENA.with(|arena| {
                let mut arena = arena.borrow_mut();
                let result = parse_json_scalar(_py, ptr, len, &mut arena);
                arena.reset();
                result
            });
            let obj = match obj {
                Ok(val) => val,
                Err(code) => return code,
            };
            *out = obj.bits();
            0
        })
    }
}

/// # Safety
/// Caller must ensure ptr is valid for len bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_msgpack_parse_scalar(
    ptr: *const u8,
    len_bits: u64,
    out: *mut u64,
) -> i32 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let len = usize_from_bits(len_bits);
            if out.is_null() {
                return 2;
            }
            let slice = std::slice::from_raw_parts(ptr, len);
            let mut cursor = Cursor::new(slice);
            let v = match rmpv::decode::read_value(&mut cursor) {
                Ok(val) => val,
                Err(_) => return 1,
            };
            let obj = PARSE_ARENA.with(|arena| {
                let mut arena = arena.borrow_mut();
                let result = msgpack_value_to_object(_py, v, &mut arena);
                arena.reset();
                result
            });
            let obj = match obj {
                Ok(val) => val,
                Err(code) => return code,
            };
            *out = obj.bits();
            0
        })
    }
}

/// # Safety
/// Caller must ensure ptr is valid for len bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_cbor_parse_scalar(
    ptr: *const u8,
    len_bits: u64,
    out: *mut u64,
) -> i32 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let len = usize_from_bits(len_bits);
            if out.is_null() {
                return 2;
            }
            let slice = std::slice::from_raw_parts(ptr, len);
            let v: serde_cbor::Value = match serde_cbor::from_slice(slice) {
                Ok(val) => val,
                Err(_) => return 1,
            };
            let obj = PARSE_ARENA.with(|arena| {
                let mut arena = arena.borrow_mut();
                let result = cbor_value_to_object(_py, v, &mut arena);
                arena.reset();
                result
            });
            let obj = match obj {
                Ok(val) => val,
                Err(code) => return code,
            };
            *out = obj.bits();
            0
        })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_json_parse_scalar_obj(obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(obj_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "json.parse expects str");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_STRING {
                let msg = format!("json.parse expects str, got {}", type_name(_py, obj));
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            let len = string_len(ptr);
            let data = string_bytes(ptr);
            let obj = PARSE_ARENA.with(|arena| {
                let mut arena = arena.borrow_mut();
                let result = parse_json_scalar(_py, data, len, &mut arena);
                arena.reset();
                result
            });
            match obj {
                Ok(val) => val.bits(),
                Err(_) => raise_exception::<_>(_py, "ValueError", "invalid JSON payload"),
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_msgpack_parse_scalar_obj(obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(obj_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "msgpack.parse expects bytes");
        };
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id != TYPE_ID_BYTES && type_id != TYPE_ID_BYTEARRAY {
                let msg = format!("msgpack.parse expects bytes, got {}", type_name(_py, obj));
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            let len = bytes_len(ptr);
            let data = bytes_data(ptr);
            let slice = std::slice::from_raw_parts(data, len);
            let mut cursor = Cursor::new(slice);
            let v = match rmpv::decode::read_value(&mut cursor) {
                Ok(val) => val,
                Err(_) => {
                    return raise_exception::<u64>(_py, "ValueError", "invalid msgpack payload");
                }
            };
            let obj = PARSE_ARENA.with(|arena| {
                let mut arena = arena.borrow_mut();
                let result = msgpack_value_to_object(_py, v, &mut arena);
                arena.reset();
                result
            });
            match obj {
                Ok(val) => val.bits(),
                Err(_) => raise_exception::<u64>(_py, "ValueError", "invalid msgpack payload"),
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_cbor_parse_scalar_obj(obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(obj_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "cbor.parse expects bytes");
        };
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id != TYPE_ID_BYTES && type_id != TYPE_ID_BYTEARRAY {
                let msg = format!("cbor.parse expects bytes, got {}", type_name(_py, obj));
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            let len = bytes_len(ptr);
            let data = bytes_data(ptr);
            let slice = std::slice::from_raw_parts(data, len);
            let v: serde_cbor::Value = match serde_cbor::from_slice(slice) {
                Ok(val) => val,
                Err(_) => {
                    return raise_exception::<u64>(_py, "ValueError", "invalid cbor payload");
                }
            };
            let obj = PARSE_ARENA.with(|arena| {
                let mut arena = arena.borrow_mut();
                let result = cbor_value_to_object(_py, v, &mut arena);
                arena.reset();
                result
            });
            match obj {
                Ok(val) => val.bits(),
                Err(_) => raise_exception::<u64>(_py, "ValueError", "invalid cbor payload"),
            }
        }
    })
}

// ---------------------------------------------------------------------------
// JSON detect_encoding / loads / dumps
// ---------------------------------------------------------------------------

/// Detect the encoding of a JSON byte string by inspecting the BOM or the
/// first few bytes.  Returns a MoltObject string: one of "utf-8", "utf-16-le",
/// "utf-16-be", "utf-32-le", "utf-32-be".
#[unsafe(no_mangle)]
pub extern "C" fn molt_json_detect_encoding(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(data_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "detect_encoding expects bytes");
        };
        let (data, len) = unsafe {
            let type_id = object_type_id(ptr);
            if type_id != TYPE_ID_BYTES && type_id != TYPE_ID_BYTEARRAY {
                let msg = format!("detect_encoding expects bytes, got {}", type_name(_py, obj));
                return raise_exception::<u64>(_py, "TypeError", &msg);
            }
            let len = bytes_len(ptr);
            let data_ptr = bytes_data(ptr);
            (std::slice::from_raw_parts(data_ptr, len), len)
        };

        let encoding = detect_json_encoding(data, len);
        let enc_ptr = alloc_string(_py, encoding.as_bytes());
        if enc_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "failed to allocate string");
        }
        MoltObject::from_ptr(enc_ptr).bits()
    })
}

fn detect_json_encoding(data: &[u8], len: usize) -> &'static str {
    // Check BOM first
    if len >= 4 {
        match (data[0], data[1], data[2], data[3]) {
            (0x00, 0x00, 0xFE, 0xFF) => return "utf-32-be",
            (0xFF, 0xFE, 0x00, 0x00) => return "utf-32-le",
            _ => {}
        }
    }
    if len >= 2 {
        match (data[0], data[1]) {
            (0xFE, 0xFF) => return "utf-16-be",
            (0xFF, 0xFE) => return "utf-16-le",
            _ => {}
        }
    }
    if len >= 3 && data[0] == 0xEF && data[1] == 0xBB && data[2] == 0xBF {
        return "utf-8";
    }

    // No BOM — use the null-byte pattern of the first 2–4 bytes (RFC 4627 §3)
    if len >= 4 {
        match (data[0], data[1], data[2], data[3]) {
            (0x00, 0x00, 0x00, _) => return "utf-32-be",
            (_, 0x00, 0x00, 0x00) => return "utf-32-le",
            (0x00, _, 0x00, _) => return "utf-16-be",
            (_, 0x00, _, 0x00) => return "utf-16-le",
            _ => {}
        }
    } else if len >= 2 {
        match (data[0], data[1]) {
            (0x00, _) => return "utf-16-be",
            (_, 0x00) => return "utf-16-le",
            _ => {}
        }
    }
    "utf-8"
}

/// Full JSON loads: parse a JSON string and return the MoltObject tree.
#[unsafe(no_mangle)]
pub extern "C" fn molt_json_loads(text_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(text_bits);

        // Accept str
        if let Some(text) = string_obj_to_owned(obj) {
            return json_loads_str(_py, &text);
        }

        // Accept bytes / bytearray — decode as UTF-8
        if let Some(ptr) = obj.as_ptr() {
            let type_id = unsafe { object_type_id(ptr) };
            if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
                let slice = unsafe {
                    let len = bytes_len(ptr);
                    let data_ptr = bytes_data(ptr);
                    std::slice::from_raw_parts(data_ptr, len)
                };
                let text = match std::str::from_utf8(slice) {
                    Ok(s) => s,
                    Err(_) => {
                        return raise_exception::<u64>(
                            _py,
                            "UnicodeDecodeError",
                            "bytes is not valid UTF-8",
                        );
                    }
                };
                return json_loads_str(_py, text);
            }
        }

        let tn = type_name(_py, obj);
        let msg = format!("the JSON object must be str, bytes or bytearray, not {tn}");
        raise_exception::<u64>(_py, "TypeError", &msg)
    })
}

fn json_loads_str(_py: &PyToken<'_>, text: &str) -> u64 {
    let v: serde_json::Value = match serde_json::from_str(text) {
        Ok(val) => val,
        Err(e) => {
            let msg = format!("{e}");
            return raise_exception::<u64>(_py, "ValueError", &msg);
        }
    };
    PARSE_ARENA.with(|arena| {
        let mut arena = arena.borrow_mut();
        let result = value_to_object(_py, v, &mut arena);
        arena.reset();
        match result {
            Ok(val) => val.bits(),
            Err(_) => raise_exception::<u64>(_py, "ValueError", "failed to convert JSON value"),
        }
    })
}

/// Full JSON dumps: serialize a MoltObject to a JSON string.
///
/// Arguments (all as NaN-boxed u64 bits):
///   obj_bits          — the value to serialize
///   indent_bits       — None for compact, int for number of spaces
///   sort_keys_bits    — boolean
///   ensure_ascii_bits — boolean
#[unsafe(no_mangle)]
pub extern "C" fn molt_json_dumps(
    obj_bits: u64,
    indent_bits: u64,
    sort_keys_bits: u64,
    ensure_ascii_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let indent_obj = obj_from_bits(indent_bits);
        let indent: Option<usize> = if indent_obj.is_none() {
            None
        } else if let Some(i) = crate::builtins::numbers::to_i64(indent_obj) {
            Some(if i < 0 { 0 } else { i as usize })
        } else {
            return raise_exception::<u64>(_py, "TypeError", "indent must be None or an integer");
        };
        let sort_keys = is_truthy(_py, obj_from_bits(sort_keys_bits));
        let ensure_ascii = is_truthy(_py, obj_from_bits(ensure_ascii_bits));

        let mut out = String::with_capacity(128);
        if let Err(msg) = object_to_json(
            _py,
            obj_from_bits(obj_bits),
            &mut out,
            indent,
            sort_keys,
            ensure_ascii,
            0,
        ) {
            return raise_exception::<u64>(_py, "TypeError", &msg);
        }

        let enc_ptr = alloc_string(_py, out.as_bytes());
        if enc_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "failed to allocate string");
        }
        MoltObject::from_ptr(enc_ptr).bits()
    })
}

fn json_write_indent(out: &mut String, indent: Option<usize>, depth: usize) {
    if let Some(n) = indent {
        out.push('\n');
        for _ in 0..(n * depth) {
            out.push(' ');
        }
    }
}

fn object_to_json(
    _py: &PyToken<'_>,
    obj: MoltObject,
    out: &mut String,
    indent: Option<usize>,
    sort_keys: bool,
    ensure_ascii: bool,
    depth: usize,
) -> Result<(), String> {
    // None → null
    if obj.is_none() {
        out.push_str("null");
        return Ok(());
    }
    // Bool (must check before int since bools can overlap with int tag)
    if let Some(b) = obj.as_bool() {
        out.push_str(if b { "true" } else { "false" });
        return Ok(());
    }
    // Int
    if obj.is_int()
        && let Some(i) = obj.as_int()
    {
        let _ = std::fmt::Write::write_fmt(out, format_args!("{i}"));
        return Ok(());
    }
    // Float
    if let Some(f) = obj.as_float() {
        if f.is_nan() || f.is_infinite() {
            return Err(format!(
                "ValueError: Out of range float values are not JSON compliant: {f}"
            ));
        }
        // Match Python repr: 0.0 not "0", 1.5 not "1.5000..."
        let s = format!("{f}");
        // serde_json-style: ensure there is a decimal point
        if !s.contains('.') && !s.contains('e') && !s.contains('E') {
            out.push_str(&s);
            out.push_str(".0");
        } else {
            out.push_str(&s);
        }
        return Ok(());
    }

    // Pointer-based types
    let Some(ptr) = obj.as_ptr() else {
        return Err(format!(
            "Object of type {} is not JSON serializable",
            type_name(_py, obj)
        ));
    };
    let type_id = unsafe { object_type_id(ptr) };
    match type_id {
        TYPE_ID_STRING => {
            let s = string_obj_to_owned(obj).unwrap_or_default();
            let encoded = json_encode_basestring_impl(&s, ensure_ascii);
            out.push_str(&encoded);
            Ok(())
        }
        TYPE_ID_BYTES | TYPE_ID_BYTEARRAY => {
            Err("Object of type bytes is not JSON serializable".to_string())
        }
        TYPE_ID_LIST | TYPE_ID_TUPLE => {
            let elems = unsafe { seq_vec_ref(ptr) };
            out.push('[');
            if elems.is_empty() {
                out.push(']');
                return Ok(());
            }
            for (i, &elem_bits) in elems.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                    if indent.is_none() {
                        out.push(' ');
                    }
                }
                json_write_indent(out, indent, depth + 1);
                object_to_json(
                    _py,
                    obj_from_bits(elem_bits),
                    out,
                    indent,
                    sort_keys,
                    ensure_ascii,
                    depth + 1,
                )?;
            }
            json_write_indent(out, indent, depth);
            out.push(']');
            Ok(())
        }
        TYPE_ID_DICT => {
            let order = unsafe { crate::builtins::containers::dict_order(ptr) };
            let len = order.len() / 2;
            out.push('{');
            if len == 0 {
                out.push('}');
                return Ok(());
            }

            // Collect key-value pairs; optionally sort by key
            let mut pairs: Vec<(usize, u64, u64)> = Vec::with_capacity(len);
            for i in 0..len {
                let key_bits = order[i * 2];
                let val_bits = order[i * 2 + 1];
                pairs.push((i, key_bits, val_bits));
            }
            if sort_keys {
                pairs.sort_by(|a, b| {
                    let a_str = string_obj_to_owned(obj_from_bits(a.1));
                    let b_str = string_obj_to_owned(obj_from_bits(b.1));
                    match (a_str, b_str) {
                        (Some(a_s), Some(b_s)) => a_s.cmp(&b_s),
                        _ => std::cmp::Ordering::Equal,
                    }
                });
            }

            for (idx, &(_, key_bits, val_bits)) in pairs.iter().enumerate() {
                if idx > 0 {
                    out.push(',');
                    if indent.is_none() {
                        out.push(' ');
                    }
                }
                json_write_indent(out, indent, depth + 1);

                // Keys must be strings (Python json.dumps raises TypeError otherwise)
                let key_obj = obj_from_bits(key_bits);
                let Some(key_str) = string_obj_to_owned(key_obj) else {
                    // Attempt int/float/bool/None key coercion like Python json encoder
                    let coerced = coerce_dict_key_to_string(_py, key_obj)?;
                    let encoded = json_encode_basestring_impl(&coerced, ensure_ascii);
                    out.push_str(&encoded);
                    out.push(':');
                    if indent.is_some() {
                        out.push(' ');
                    }
                    object_to_json(
                        _py,
                        obj_from_bits(val_bits),
                        out,
                        indent,
                        sort_keys,
                        ensure_ascii,
                        depth + 1,
                    )?;
                    continue;
                };
                let encoded = json_encode_basestring_impl(&key_str, ensure_ascii);
                out.push_str(&encoded);
                out.push(':');
                if indent.is_some() {
                    out.push(' ');
                }
                object_to_json(
                    _py,
                    obj_from_bits(val_bits),
                    out,
                    indent,
                    sort_keys,
                    ensure_ascii,
                    depth + 1,
                )?;
            }
            json_write_indent(out, indent, depth);
            out.push('}');
            Ok(())
        }
        TYPE_ID_BIGINT => {
            // BigInts that didn't fit inline
            let tn = type_name(_py, obj);
            Err(format!("Object of type {tn} is not JSON serializable"))
        }
        _ => {
            let tn = type_name(_py, obj);
            Err(format!("Object of type {tn} is not JSON serializable"))
        }
    }
}

/// Coerce a non-string dict key to a JSON-legal string, mirroring Python's
/// `json.encoder.JSONEncoder` key coercion for int, float, bool, None.
fn coerce_dict_key_to_string(_py: &PyToken<'_>, key: MoltObject) -> Result<String, String> {
    if key.is_none() {
        return Ok("null".to_string());
    }
    if let Some(b) = key.as_bool() {
        return Ok(if b { "true" } else { "false" }.to_string());
    }
    if let Some(i) = key.as_int() {
        return Ok(format!("{i}"));
    }
    if let Some(f) = key.as_float() {
        if f.is_nan() || f.is_infinite() {
            return Err(format!(
                "ValueError: Out of range float values are not JSON compliant: {f}"
            ));
        }
        return Ok(format!("{f}"));
    }
    let tn = type_name(_py, key);
    Err(format!(
        "keys must be str, int, float, bool or None, not {tn}"
    ))
}
