use crate::arena::TempArena;
use crate::*;
use std::io::Cursor;

// --- JSON ---

/// # Safety
/// Dereferences raw pointers. Caller must ensure ptr is valid UTF-8 of at least len bytes.
#[no_mangle]
pub unsafe extern "C" fn molt_json_parse_int(ptr: *const u8, len_bits: u64) -> i64 {
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
    let slice = std::slice::from_raw_parts(ptr, len);
    let s = std::str::from_utf8(slice).map_err(|_| 1)?;
    let v: serde_json::Value = serde_json::from_str(s).map_err(|_| 1)?;
    value_to_object(_py, v, arena)
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure ptr is valid UTF-8 of at least len bytes.
#[no_mangle]
pub unsafe extern "C" fn molt_json_parse_scalar(
    ptr: *const u8,
    len_bits: u64,
    out: *mut u64,
) -> i32 {
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

/// # Safety
/// Caller must ensure ptr is valid for len bytes.
#[no_mangle]
pub unsafe extern "C" fn molt_msgpack_parse_scalar(
    ptr: *const u8,
    len_bits: u64,
    out: *mut u64,
) -> i32 {
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

/// # Safety
/// Caller must ensure ptr is valid for len bytes.
#[no_mangle]
pub unsafe extern "C" fn molt_cbor_parse_scalar(
    ptr: *const u8,
    len_bits: u64,
    out: *mut u64,
) -> i32 {
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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
