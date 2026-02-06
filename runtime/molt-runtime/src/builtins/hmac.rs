use crate::builtins::hashlib::{build_hash_handle, HashHandle};
use crate::*;

#[derive(Clone)]
pub(crate) struct HmacHandle {
    inner: HashHandle,
    outer: HashHandle,
    digest_size: usize,
    block_size: usize,
}

fn hash_handle_from_bits(bits: u64) -> Option<&'static mut HmacHandle> {
    let ptr = ptr_from_bits(bits);
    if ptr.is_null() {
        return None;
    }
    Some(unsafe { &mut *(ptr as *mut HmacHandle) })
}

fn bytes_like_from_bits(_py: &PyToken<'_>, bits: u64) -> Result<&'static [u8], u64> {
    let obj = obj_from_bits(bits);
    let Some(ptr) = obj.as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "object supporting the buffer API required",
        ));
    };
    unsafe {
        if object_type_id(ptr) == TYPE_ID_STRING {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "Strings must be encoded before hashing",
            ));
        }
        if let Some(slice) = bytes_like_slice(ptr) {
            return Ok(slice);
        }
    }
    Err(raise_exception::<u64>(
        _py,
        "TypeError",
        "object supporting the buffer API required",
    ))
}

fn key_bytes_from_bits(_py: &PyToken<'_>, bits: u64) -> Result<Vec<u8>, u64> {
    let obj = obj_from_bits(bits);
    let Some(ptr) = obj.as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "key: expected bytes or bytearray, but got 'NoneType'",
        ));
    };
    unsafe {
        if let Some(slice) = bytes_like_slice_raw(ptr) {
            return Ok(slice.to_vec());
        }
    }
    let type_name = type_name(_py, obj);
    let msg = format!("key: expected bytes or bytearray, but got '{type_name}'");
    Err(raise_exception::<u64>(_py, "TypeError", &msg))
}

fn build_hmac_handle(
    _py: &PyToken<'_>,
    key_bits: u64,
    msg_bits: u64,
    name: &str,
    options_bits: u64,
) -> Result<HmacHandle, u64> {
    let mut inner = build_hash_handle(_py, name, options_bits)?;
    if inner.is_xof {
        return Err(raise_exception::<u64>(
            _py,
            "ValueError",
            "no reason supplied",
        ));
    }
    let mut outer = build_hash_handle(_py, name, options_bits)?;
    let digest_size = inner.digest_size;
    let block_size = inner.block_size;
    let mut key = key_bytes_from_bits(_py, key_bits)?;
    if key.len() > block_size {
        let mut key_hash = build_hash_handle(_py, name, options_bits)?;
        key_hash.update(&key);
        key = key_hash
            .finalize_bytes(None)
            .map_err(|_| raise_exception::<u64>(_py, "ValueError", "no reason supplied"))?;
    }
    if key.len() < block_size {
        key.resize(block_size, 0);
    }
    let mut i_key_pad = vec![0x36; block_size];
    let mut o_key_pad = vec![0x5c; block_size];
    for (idx, byte) in key.iter().enumerate() {
        i_key_pad[idx] ^= byte;
        o_key_pad[idx] ^= byte;
    }
    inner.update(&i_key_pad);
    outer.update(&o_key_pad);
    if !obj_from_bits(msg_bits).is_none() {
        let msg = bytes_like_from_bits(_py, msg_bits)?;
        if !msg.is_empty() {
            inner.update(msg);
        }
    }
    Ok(HmacHandle {
        inner,
        outer,
        digest_size,
        block_size,
    })
}

#[no_mangle]
pub extern "C" fn molt_hmac_new(
    key_bits: u64,
    msg_bits: u64,
    name_bits: u64,
    options_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name) = string_obj_to_owned(name_obj) else {
            return raise_exception::<u64>(_py, "TypeError", "hash name must be str");
        };
        let handle = match build_hmac_handle(_py, key_bits, msg_bits, &name, options_bits) {
            Ok(handle) => handle,
            Err(bits) => return bits,
        };
        let ptr = Box::into_raw(Box::new(handle)) as *mut u8;
        bits_from_ptr(ptr)
    })
}

#[no_mangle]
pub extern "C" fn molt_hmac_update(handle_bits: u64, data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = hash_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid hmac handle");
        };
        let data = match bytes_like_from_bits(_py, data_bits) {
            Ok(slice) => slice,
            Err(bits) => return bits,
        };
        if !data.is_empty() {
            handle.inner.update(data);
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_hmac_copy(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = hash_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid hmac handle");
        };
        let copy = handle.clone();
        let ptr = Box::into_raw(Box::new(copy)) as *mut u8;
        bits_from_ptr(ptr)
    })
}

#[no_mangle]
pub extern "C" fn molt_hmac_digest(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = hash_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid hmac handle");
        };
        let inner_digest = match handle.inner.clone().finalize_bytes(None) {
            Ok(bytes) => bytes,
            Err(_) => return raise_exception::<u64>(_py, "ValueError", "no reason supplied"),
        };
        let mut outer = handle.outer.clone();
        outer.update(&inner_digest);
        let out = match outer.finalize_bytes(None) {
            Ok(bytes) => bytes,
            Err(_) => return raise_exception::<u64>(_py, "ValueError", "no reason supplied"),
        };
        let ptr = alloc_bytes(_py, &out);
        if ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_hmac_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = ptr_from_bits(handle_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        release_ptr(ptr);
        let _ = unsafe { Box::from_raw(ptr as *mut HmacHandle) };
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_compare_digest(a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let a_obj = obj_from_bits(a_bits);
        let b_obj = obj_from_bits(b_bits);
        let a_ptr = a_obj.as_ptr();
        let b_ptr = b_obj.as_ptr();
        let a_is_str = a_ptr.is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_STRING });
        let b_is_str = b_ptr.is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_STRING });
        if a_is_str && b_is_str {
            let a_text = match string_obj_to_owned(a_obj) {
                Some(text) => text,
                None => {
                    return raise_exception::<u64>(_py, "TypeError", "expected str");
                }
            };
            let b_text = match string_obj_to_owned(b_obj) {
                Some(text) => text,
                None => {
                    return raise_exception::<u64>(_py, "TypeError", "expected str");
                }
            };
            let a_len = a_text.chars().count();
            let b_len = b_text.chars().count();
            if a_len != b_len {
                return MoltObject::from_bool(false).bits();
            }
            let mut acc: u32 = 0;
            for (left, right) in a_text.chars().zip(b_text.chars()) {
                acc |= (left as u32) ^ (right as u32);
            }
            return MoltObject::from_bool(acc == 0).bits();
        }
        let a_bytes = a_ptr.and_then(|ptr| unsafe { bytes_like_slice(ptr) });
        let b_bytes = b_ptr.and_then(|ptr| unsafe { bytes_like_slice(ptr) });
        if let (Some(a_slice), Some(b_slice)) = (a_bytes, b_bytes) {
            if a_slice.len() != b_slice.len() {
                return MoltObject::from_bool(false).bits();
            }
            let mut acc: u8 = 0;
            for (left, right) in a_slice.iter().zip(b_slice.iter()) {
                acc |= left ^ right;
            }
            return MoltObject::from_bool(acc == 0).bits();
        }
        if a_bytes.is_some() || b_bytes.is_some() {
            let other = if a_bytes.is_some() { b_obj } else { a_obj };
            let type_name = type_name(_py, other);
            let msg = format!("a bytes-like object is required, not '{type_name}'");
            return raise_exception::<u64>(_py, "TypeError", &msg);
        }
        if a_is_str || b_is_str {
            let a_name = type_name(_py, a_obj);
            let b_name = type_name(_py, b_obj);
            let msg = format!(
                "unsupported operand types(s) or combination of types: '{a_name}' and '{b_name}'"
            );
            return raise_exception::<u64>(_py, "TypeError", &msg);
        }
        let a_name = type_name(_py, a_obj);
        let b_name = type_name(_py, b_obj);
        let msg = format!(
            "unsupported operand types(s) or combination of types: '{a_name}' and '{b_name}'"
        );
        raise_exception::<u64>(_py, "TypeError", &msg)
    })
}
