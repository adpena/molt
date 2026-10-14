//! CPython-shaped `Py*` helpers that only the runtime's unit tests call.
//!
//! The exported C symbols live in `molt-cpython-abi`; these Rust functions
//! let runtime tests drive the same semantics without the C boundary.

use super::cpython_compat::c_dict_pointer;
use super::*;

/// `PyObject_GetIter(obj)` — call `__iter__` on `obj`.
/// Returns a new iterator handle (caller owns the reference) or NULL (0) on error.
pub extern "C" fn PyObject_GetIter(obj: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let res = molt_iter(obj);
        if obj_from_bits(res).is_none() {
            if !exception_pending(_py) {
                let _: u64 = raise_not_iterable(_py, obj);
            }
            return 0;
        }
        res
    })
}

/// `PyIter_Next(iter)` — advance iterator and return the next value.
/// Returns the next value handle (caller owns the reference), or 0 (NULL) when
/// the iterator is exhausted (no exception set) or on error (exception set).
pub extern "C" fn PyIter_Next(iter: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let pair_bits = molt_iter_next(iter);
        if exception_pending(_py) {
            if !obj_from_bits(pair_bits).is_none() {
                dec_ref_bits(_py, pair_bits);
            }
            return 0;
        }
        let Some(pair_ptr) = obj_from_bits(pair_bits).as_ptr() else {
            return 0;
        };
        unsafe {
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                dec_ref_bits(_py, pair_bits);
                return 0;
            }
            let Some(elems) = crate::object::seq_access::snapshot(
                _py,
                pair_ptr,
                "iterator result snapshot allocation failed",
            ) else {
                dec_ref_bits(_py, pair_bits);
                return 0;
            };
            if elems.len() < 2 {
                dec_ref_bits(_py, pair_bits);
                return 0;
            }
            let val_bits = elems[0];
            let done_bits = elems[1];
            let done = is_truthy(_py, obj_from_bits(done_bits));
            if done {
                // StopIteration — no exception, just return NULL.
                dec_ref_bits(_py, pair_bits);
                return 0;
            }
            inc_ref_bits(_py, val_bits);
            dec_ref_bits(_py, pair_bits);
            val_bits
        }
    })
}

/// Internal boxed iterator predicate. Keep completion explicit; the exported
/// pointer-based PyIter_Check contract is owned by molt-cpython-abi.
pub fn PyIter_Check(obj: u64) -> Result<bool, ()> {
    crate::with_gil_entry_nopanic!(_py, { unsafe { is_iterator_bits(_py, obj) } })
}

/// `PyList_Check(obj)` — return 1 if obj is a list, 0 otherwise.
pub extern "C" fn PyList_Check(obj: u64) -> i32 {
    if let Some(ptr) = obj_from_bits(obj).as_ptr()
        && unsafe { object_type_id(ptr) } == TYPE_ID_LIST
    {
        1
    } else {
        0
    }
}

/// `PyDict_Check(obj)` — return 1 if obj is a dict, 0 otherwise.
pub extern "C" fn PyDict_Check(obj: u64) -> i32 {
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            crate::object::class_layout::is_real_subtype(
                py,
                type_of_bits(py, obj),
                builtin_classes(py).dict,
            ) as i32
        }
    })
}

/// `PyTuple_Check(obj)` — return 1 if obj is a tuple, 0 otherwise.
pub extern "C" fn PyTuple_Check(obj: u64) -> i32 {
    if let Some(ptr) = obj_from_bits(obj).as_ptr()
        && unsafe { object_type_id(ptr) } == TYPE_ID_TUPLE
    {
        1
    } else {
        0
    }
}

/// `PyFloat_Check(obj)` — return 1 if obj is a float, 0 otherwise.
pub extern "C" fn PyFloat_Check(obj: u64) -> i32 {
    if obj_from_bits(obj).is_float() { 1 } else { 0 }
}

/// `PyLong_Check(obj)` — return 1 if obj is an int, 0 otherwise.
/// Covers both inline NaN-boxed ints and heap-allocated bigints.
pub extern "C" fn PyLong_Check(obj: u64) -> i32 {
    let obj_mo = obj_from_bits(obj);
    if obj_mo.is_int() {
        return 1;
    }
    if let Some(ptr) = obj_mo.as_ptr()
        && unsafe { object_type_id(ptr) } == TYPE_ID_BIGINT
    {
        return 1;
    }
    0
}

/// `PyUnicode_Check(obj)` — return 1 if obj is a str, 0 otherwise.
pub extern "C" fn PyUnicode_Check(obj: u64) -> i32 {
    if let Some(ptr) = obj_from_bits(obj).as_ptr()
        && unsafe { object_type_id(ptr) } == TYPE_ID_STRING
    {
        1
    } else {
        0
    }
}

/// `PyBool_Check(obj)` — return 1 if obj is a bool, 0 otherwise.
pub extern "C" fn PyBool_Check(obj: u64) -> i32 {
    if obj_from_bits(obj).is_bool() { 1 } else { 0 }
}

/// `PyNone_Check(obj)` — return 1 if obj is None, 0 otherwise.
pub extern "C" fn PyNone_Check(obj: u64) -> i32 {
    if obj_from_bits(obj).is_none() { 1 } else { 0 }
}

/// `PyList_New(size)` — create a new list of length `size` filled with None values.
/// Returns the new list handle (caller owns the reference) or 0 on error.
pub extern "C" fn PyList_New(size: isize) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if size < 0 {
            let _ =
                raise_exception::<u64>(_py, "SystemError", "negative size passed to PyList_New");
            return 0;
        }
        let n = size as usize;
        let none = none_bits();
        let elems: Vec<u64> = vec![none; n];
        let ptr = alloc_list(_py, &elems);
        if ptr.is_null() {
            return 0;
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

/// `PyList_Size(list)` — return the length of the list, or -1 on error.
pub extern "C" fn PyList_Size(list: u64) -> isize {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(ptr) = obj_from_bits(list).as_ptr() else {
            let _ = raise_exception::<u64>(_py, "TypeError", "expected list object");
            return -1;
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_LIST {
                let _ = raise_exception::<u64>(_py, "TypeError", "expected list object");
                return -1;
            }
            list_len(ptr) as isize
        }
    })
}

/// `PyList_GetItem(list, index)` — return a **borrowed** reference to list[index].
/// Returns 0 on error. The caller must NOT decref the returned handle.
pub extern "C" fn PyList_GetItem(list: u64, index: isize) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(ptr) = obj_from_bits(list).as_ptr() else {
            let _ = raise_exception::<u64>(_py, "TypeError", "expected list object");
            return 0;
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_LIST {
                let _ = raise_exception::<u64>(_py, "TypeError", "expected list object");
                return 0;
            }
            let len = crate::object::seq_access::len(ptr);
            let actual_idx = if index < 0 {
                let adjusted = (len as isize) + index;
                if adjusted < 0 {
                    let _ = raise_exception::<u64>(_py, "IndexError", "list index out of range");
                    return 0;
                }
                adjusted as usize
            } else {
                index as usize
            };
            if actual_idx >= len {
                let _ = raise_exception::<u64>(_py, "IndexError", "list index out of range");
                return 0;
            }
            // Borrowed reference — do not inc_ref.
            crate::object::seq_access::item(ptr, actual_idx).unwrap_or(0)
        }
    })
}

/// `PyList_SetItem(list, index, item)` — set list[index] to `item`.
/// **Steals** a reference to `item`. Returns 0 on success, -1 on error.
pub extern "C" fn PyList_SetItem(list: u64, index: isize, item: u64) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(ptr) = obj_from_bits(list).as_ptr() else {
            // Steal the reference even on failure (CPython semantics).
            dec_ref_bits(_py, item);
            let _ = raise_exception::<u64>(_py, "TypeError", "expected list object");
            return -1;
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_LIST {
                dec_ref_bits(_py, item);
                let _ = raise_exception::<u64>(_py, "TypeError", "expected list object");
                return -1;
            }
            let len = list_len(ptr);
            let actual_idx = if index < 0 {
                let adjusted = (len as isize) + index;
                if adjusted < 0 {
                    dec_ref_bits(_py, item);
                    let _ = raise_exception::<u64>(
                        _py,
                        "IndexError",
                        "list assignment index out of range",
                    );
                    return -1;
                }
                adjusted as usize
            } else {
                index as usize
            };
            if actual_idx >= len {
                dec_ref_bits(_py, item);
                let _ =
                    raise_exception::<u64>(_py, "IndexError", "list assignment index out of range");
                return -1;
            }
            let changed = crate::object::list_mutation::replace_indices(
                _py,
                ptr,
                std::slice::from_ref(&actual_idx),
                std::slice::from_ref(&item),
            );
            // Steal the caller's reference on both success and failure. The
            // canonical mutation authority acquired the list's own edge before
            // publication when the store succeeded.
            dec_ref_bits(_py, item);
            if changed { 0 } else { -1 }
        }
    })
}

/// `PyList_Append(list, item)` — append `item` to `list`.
/// Returns 0 on success, -1 on error.
pub extern "C" fn PyList_Append(list: u64, item: u64) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(ptr) = obj_from_bits(list).as_ptr() else {
            let _ = raise_exception::<u64>(_py, "TypeError", "expected list object");
            return -1;
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_LIST {
                let _ = raise_exception::<u64>(_py, "TypeError", "expected list object");
                return -1;
            }
            if crate::object::list_mutation::extend_from_slice(
                _py,
                ptr,
                std::slice::from_ref(&item),
            ) {
                0
            } else {
                -1
            }
        }
    })
}

/// `PyDict_New()` — create a new empty dict.
/// Returns the new dict handle (caller owns the reference) or 0 on error.
pub extern "C" fn PyDict_New() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ptr = alloc_dict_with_pairs(_py, &[]);
        if ptr.is_null() {
            return 0;
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

/// `PyDict_SetItem(dict, key, val)` — insert key/value pair into dict.
/// Returns 0 on success, -1 on error.
pub extern "C" fn PyDict_SetItem(dict: u64, key: u64, val: u64) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(ptr) = c_dict_pointer(_py, dict) else {
            return -1;
        };
        unsafe {
            if dict_set_in_place(_py, ptr, key, val) {
                0
            } else {
                -1
            }
        }
    })
}

/// `PyDict_SetItemString(dict, key, val)` — insert string key/value into dict.
/// The key is a C string (null-terminated). Returns 0 on success, -1 on error.
pub unsafe extern "C" fn PyDict_SetItemString(
    dict: u64,
    key: *const std::ffi::c_char,
    val: u64,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        if key.is_null() {
            let _ = raise_exception::<u64>(_py, "TypeError", "key string pointer cannot be null");
            return -1;
        }
        let key_cstr = unsafe { std::ffi::CStr::from_ptr(key) };
        let key_bytes = key_cstr.to_bytes();
        let key_ptr = alloc_string(_py, key_bytes);
        if key_ptr.is_null() {
            return -1;
        }
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        let rc = PyDict_SetItem(dict, key_bits, val);
        dec_ref_bits(_py, key_bits);
        rc
    })
}

/// `PyDict_Size(dict)` — return the number of items in the dict, or -1 on error.
pub extern "C" fn PyDict_Size(dict: u64) -> isize {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(ptr) = c_dict_pointer(_py, dict) else {
            return -1;
        };
        unsafe { dict_len(ptr) as isize }
    })
}

/// `PyDict_Contains(dict, key)` — return 1 if key is in dict, 0 if not, -1 on error.
pub extern "C" fn PyDict_Contains(dict: u64, key: u64) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(ptr) = c_dict_pointer(_py, dict) else {
            return -1;
        };
        unsafe {
            match dict_get_in_place(_py, ptr, key) {
                Ok(Some(_)) => 1,
                Ok(None) => 0,
                Err(()) => -1,
            }
        }
    })
}

/// `PyTuple_New(size)` — create a new exact-length tuple with uninitialized slots.
/// Returns the new tuple handle (caller owns the reference) or 0 on error.
pub extern "C" fn PyTuple_New(size: isize) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if size < 0 {
            let _ =
                raise_exception::<u64>(_py, "SystemError", "negative size passed to PyTuple_New");
            return 0;
        }
        let n = size as usize;
        let ptr = crate::object::builders::alloc_tuple_uninitialized(_py, n);
        if ptr.is_null() {
            return 0;
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

/// `PyTuple_Size(tuple)` — return the length of the tuple, or -1 on error.
pub extern "C" fn PyTuple_Size(tuple: u64) -> isize {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(ptr) = obj_from_bits(tuple).as_ptr() else {
            let _ = raise_exception::<u64>(_py, "TypeError", "expected tuple object");
            return -1;
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_TUPLE {
                let _ = raise_exception::<u64>(_py, "TypeError", "expected tuple object");
                return -1;
            }
            tuple_len(ptr) as isize
        }
    })
}

/// `PyTuple_GetItem(tuple, index)` — return a **borrowed** reference to tuple[index].
/// Returns 0 on error. The caller must NOT decref the returned handle.
pub extern "C" fn PyTuple_GetItem(tuple: u64, index: isize) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(ptr) = obj_from_bits(tuple).as_ptr() else {
            let _ = raise_exception::<u64>(_py, "TypeError", "expected tuple object");
            return 0;
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_TUPLE {
                let _ = raise_exception::<u64>(_py, "TypeError", "expected tuple object");
                return 0;
            }
            let len = crate::object::seq_access::len(ptr);
            if index < 0 || (index as usize) >= len {
                let _ = raise_exception::<u64>(_py, "IndexError", "tuple index out of range");
                return 0;
            }
            // Borrowed reference — do not inc_ref.
            crate::object::seq_access::initialized_tuple_item(_py, ptr, index as usize).unwrap_or(0)
        }
    })
}

/// `PyTuple_SetItem(tuple, index, item)` — set tuple[index] to `item`.
/// **Steals** a reference to `item`. Returns 0 on success, -1 on error.
/// Intended for filling newly-created tuples before they are exposed to other code.
pub extern "C" fn PyTuple_SetItem(tuple: u64, index: isize, item: u64) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(ptr) = obj_from_bits(tuple).as_ptr() else {
            dec_ref_bits(_py, item);
            let _ = raise_exception::<u64>(_py, "TypeError", "expected tuple object");
            return -1;
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_TUPLE {
                dec_ref_bits(_py, item);
                let _ = raise_exception::<u64>(_py, "TypeError", "expected tuple object");
                return -1;
            }
            if index < 0 || (index as usize) >= crate::object::seq_access::len(ptr) {
                dec_ref_bits(_py, item);
                let _ = raise_exception::<u64>(_py, "IndexError", "tuple index out of range");
                return -1;
            }
            let Some(old) =
                crate::object::seq_access::replace_unique_item_owned(ptr, index as usize, item)
            else {
                dec_ref_bits(_py, item);
                let _ = raise_exception::<u64>(
                    _py,
                    "SystemError",
                    "PyTuple_SetItem requires a uniquely owned tuple",
                );
                return -1;
            };
            dec_ref_bits(_py, old);
            0
        }
    })
}

/// `PyNumber_Power(a, b, mod_)` — return `pow(a, b, mod_)`.
/// Canonical None selects two-argument power; every other bit pattern is a
/// present modulus, including float +0.0 (`0`), float -0.0, and integer zero.
pub extern "C" fn PyNumber_Power(a: u64, b: u64, mod_: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let res = if obj_from_bits(mod_).is_none() {
            molt_pow(a, b)
        } else {
            molt_pow_mod(a, b, mod_)
        };
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyNumber_Check(o)` — return 1 if `o` is a numeric type (int, float, bool), 0 otherwise.
pub extern "C" fn PyNumber_Check(o: u64) -> i32 {
    let obj = obj_from_bits(o);
    if obj.is_int() || obj.is_float() || obj.is_bool() {
        return 1;
    }
    if let Some(ptr) = obj.as_ptr()
        && unsafe { object_type_id(ptr) } == TYPE_ID_BIGINT
    {
        return 1;
    }
    0
}

/// `PyNumber_Long(o)` — return `int(o)`, or 0 on error.
pub extern "C" fn PyNumber_Long(o: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let res = molt_int_from_obj(o, none_bits(), 0);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyNumber_Float(o)` — return `float(o)`, or 0 on error.
pub extern "C" fn PyNumber_Float(o: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let res = molt_float_from_obj(o);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyMapping_Length(o)` — return `len(o)` for dict-like objects, or -1 on error.
pub extern "C" fn PyMapping_Length(o: u64) -> isize {
    crate::with_gil_entry_nopanic!(_py, {
        let len_bits = molt_len(o);
        if exception_pending(_py) {
            if !obj_from_bits(len_bits).is_none() {
                dec_ref_bits(_py, len_bits);
            }
            return -1;
        }
        let out = len_bits_to_i64(_py, len_bits);
        dec_ref_bits(_py, len_bits);
        out as isize
    })
}

/// `PyMapping_GetItemString(o, key)` — return `o[key]` where `key` is a NUL-terminated
/// C string. Returns 0 on error.
pub unsafe extern "C" fn PyMapping_GetItemString(o: u64, key: *const std::ffi::c_char) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if key.is_null() {
            let _ = raise_exception::<u64>(_py, "TypeError", "key string pointer cannot be null");
            return 0;
        }
        let key_cstr = unsafe { std::ffi::CStr::from_ptr(key) };
        let key_bytes = key_cstr.to_bytes();
        let key_ptr = alloc_string(_py, key_bytes);
        if key_ptr.is_null() {
            return 0;
        }
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        let res = if let Some(obj_ptr) = obj_from_bits(o).as_ptr()
            && unsafe { crate::object::object_is_exact_builtin_dict(_py, obj_ptr) }
        {
            unsafe {
                match crate::object::ops::dict_get_in_place(_py, obj_ptr, key_bits) {
                    Ok(Some(value)) => {
                        inc_ref_bits(_py, value);
                        Ok(value)
                    }
                    Ok(None) => {
                        raise_key_error_with_key::<()>(_py, key_bits);
                        Err(())
                    }
                    Err(()) => Err(()),
                }
            }
        } else {
            crate::object::ops::index_impl(o, key_bits, false, true)
        };
        dec_ref_bits(_py, key_bits);
        res.unwrap_or(0)
    })
}

/// `PyMapping_HasKey(o, key)` — return 1 if `key in o`, 0 otherwise.
/// Does not raise exceptions on failure (returns 0 instead).
pub extern "C" fn PyMapping_HasKey(o: u64, key: u64) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let res = molt_contains(o, key);
        if exception_pending(_py) {
            let _ = molt_exception_clear();
            return 0;
        }
        if is_truthy(_py, obj_from_bits(res)) {
            1
        } else {
            0
        }
    })
}

/// `PySequence_GetItem(o, i)` — return `o[i]`, or 0 on error.
pub extern "C" fn PySequence_GetItem(o: u64, i: isize) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let res = crate::object::sequence_index::sequence_item_at_index(_py, o, i as i64);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PySequence_Length(o)` — return `len(o)`, or -1 on error.
pub extern "C" fn PySequence_Length(o: u64) -> isize {
    crate::with_gil_entry_nopanic!(_py, {
        let len_bits = molt_len(o);
        if exception_pending(_py) {
            if !obj_from_bits(len_bits).is_none() {
                dec_ref_bits(_py, len_bits);
            }
            return -1;
        }
        let out = len_bits_to_i64(_py, len_bits);
        dec_ref_bits(_py, len_bits);
        out as isize
    })
}

/// `PySequence_Contains(o, value)` — return 1 if `value in o`, 0 if not, -1 on error.
pub extern "C" fn PySequence_Contains(o: u64, value: u64) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let res = molt_contains(o, value);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return -1;
        }
        if is_truthy(_py, obj_from_bits(res)) {
            1
        } else {
            0
        }
    })
}

/// `PyBytes_FromStringAndSize(v, len)` — create a new bytes object from a buffer.
/// If `v` is NULL and `len > 0`, returns 0 (error). If `len == 0`, returns an empty bytes.
/// Returns the new bytes handle (caller owns the reference) or 0 on error.
pub unsafe extern "C" fn PyBytes_FromStringAndSize(v: *const u8, len: isize) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if len < 0 {
            let _ = raise_exception::<u64>(
                _py,
                "SystemError",
                "negative size passed to PyBytes_FromStringAndSize",
            );
            return 0;
        }
        let data = if len == 0 {
            &[]
        } else if v.is_null() {
            let _ = raise_exception::<u64>(
                _py,
                "TypeError",
                "bytes source pointer cannot be null when len > 0",
            );
            return 0;
        } else {
            unsafe { std::slice::from_raw_parts(v, len as usize) }
        };
        let ptr = alloc_bytes(_py, data);
        if ptr.is_null() {
            return 0;
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

/// `PyBytes_AsString(o)` — return a pointer to the internal buffer of a bytes object.
/// Returns NULL on error (e.g. not a bytes object). The pointer is borrowed and valid
/// as long as the bytes object is alive.
pub extern "C" fn PyBytes_AsString(o: u64) -> *const u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(ptr) = obj_from_bits(o).as_ptr() else {
            let _ = raise_exception::<u64>(_py, "TypeError", "expected bytes object");
            return std::ptr::null();
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_BYTES {
                let _ = raise_exception::<u64>(_py, "TypeError", "expected bytes object");
                return std::ptr::null();
            }
            bytes_data(ptr)
        }
    })
}

/// `PyBytes_Size(o)` — return the length of a bytes object, or -1 on error.
pub extern "C" fn PyBytes_Size(o: u64) -> isize {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(ptr) = obj_from_bits(o).as_ptr() else {
            let _ = raise_exception::<u64>(_py, "TypeError", "expected bytes object");
            return -1;
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_BYTES {
                let _ = raise_exception::<u64>(_py, "TypeError", "expected bytes object");
                return -1;
            }
            bytes_len(ptr) as isize
        }
    })
}

/// `PyUnicode_FromString(v)` — create a new str from a NUL-terminated UTF-8 C string.
/// Returns the new string handle (caller owns the reference) or 0 on error.
pub unsafe extern "C" fn PyUnicode_FromString(v: *const std::ffi::c_char) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if v.is_null() {
            let _ =
                raise_exception::<u64>(_py, "TypeError", "string source pointer cannot be null");
            return 0;
        }
        let cstr = unsafe { std::ffi::CStr::from_ptr(v) };
        let bytes = cstr.to_bytes();
        let ptr = alloc_string(_py, bytes);
        if ptr.is_null() {
            return 0;
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

/// `PyUnicode_AsUTF8(o)` — return a pointer to the UTF-8 representation of a string.
/// Returns NULL on error. The pointer is borrowed and valid as long as the string object
/// is alive.
pub extern "C" fn PyUnicode_AsUTF8(o: u64) -> *const std::ffi::c_char {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(ptr) = obj_from_bits(o).as_ptr() else {
            let _ = raise_exception::<u64>(_py, "TypeError", "expected str object");
            return std::ptr::null();
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_STRING {
                let _ = raise_exception::<u64>(_py, "TypeError", "expected str object");
                return std::ptr::null();
            }
            string_bytes(ptr) as *const std::ffi::c_char
        }
    })
}

/// `PyUnicode_AsUTF8AndSize(o, size)` — return a pointer to the UTF-8 representation
/// and write the length to `*size` (if `size` is not NULL).
/// Returns NULL on error.
pub unsafe extern "C" fn PyUnicode_AsUTF8AndSize(
    o: u64,
    size: *mut isize,
) -> *const std::ffi::c_char {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(ptr) = obj_from_bits(o).as_ptr() else {
            let _ = raise_exception::<u64>(_py, "TypeError", "expected str object");
            return std::ptr::null();
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_STRING {
                let _ = raise_exception::<u64>(_py, "TypeError", "expected str object");
                return std::ptr::null();
            }
            if !size.is_null() {
                *size = string_len(ptr) as isize;
            }
            string_bytes(ptr) as *const std::ffi::c_char
        }
    })
}

/// `PyMem_Malloc(size)` — allocate `size` bytes of memory.
/// Returns a pointer to the allocated memory, or NULL on failure.
pub unsafe extern "C" fn PyMem_Malloc(size: usize) -> *mut u8 {
    unsafe { molt_cpython_abi::api::memory::PyMem_Malloc(size).cast() }
}

/// `PyMem_Realloc(ptr, size)` — resize a previously allocated block.
/// Returns a pointer to the reallocated memory, or NULL on failure.
pub unsafe extern "C" fn PyMem_Realloc(ptr: *mut u8, size: usize) -> *mut u8 {
    unsafe { molt_cpython_abi::api::memory::PyMem_Realloc(ptr.cast(), size).cast() }
}

/// `PyMem_Free(ptr)` — free memory allocated by `PyMem_Malloc` or `PyMem_Realloc`.
pub unsafe extern "C" fn PyMem_Free(ptr: *mut u8) {
    unsafe { molt_cpython_abi::api::memory::PyMem_Free(ptr.cast()) }
}

/// `PyObject_Malloc(size)` — allocate memory for an object.
/// Currently an alias for `PyMem_Malloc`.
pub unsafe extern "C" fn PyObject_Malloc(size: usize) -> *mut u8 {
    unsafe { PyMem_Malloc(size) }
}

/// `PyObject_Realloc(ptr, size)` — reallocate memory for an object.
/// Currently an alias for `PyMem_Realloc`.
pub unsafe extern "C" fn PyObject_Realloc(ptr: *mut u8, size: usize) -> *mut u8 {
    unsafe { PyMem_Realloc(ptr, size) }
}

/// `PyObject_Free(ptr)` — free memory allocated by `PyObject_Malloc`.
/// Currently an alias for `PyMem_Free`.
pub unsafe extern "C" fn PyObject_Free(ptr: *mut u8) {
    unsafe { PyMem_Free(ptr) }
}

/// `PyObject_Repr(obj)` — return repr(obj), or 0 on error. Caller owns the reference.
pub extern "C" fn PyObject_Repr(obj: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let res = molt_repr_from_obj(obj);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyObject_Str(obj)` — return str(obj), or 0 on error. Caller owns the reference.
pub extern "C" fn PyObject_Str(obj: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let res = molt_str_from_obj(obj);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyObject_Hash(obj)` — return the hash of obj, or -1 on error.
pub extern "C" fn PyObject_Hash(obj: u64) -> i64 {
    crate::with_gil_entry_nopanic!(_py, {
        let res = molt_hash_builtin(obj);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return -1;
        }
        let obj_res = obj_from_bits(res);
        if obj_res.is_none() {
            return -1;
        }
        let h = to_i64(obj_res).unwrap_or(-1);
        if obj_res.as_ptr().is_some() {
            dec_ref_bits(_py, res);
        }
        h
    })
}

/// `PyObject_IsTrue(obj)` — return 1 if obj is truthy, 0 if falsy, -1 on error.
pub extern "C" fn PyObject_IsTrue(obj: u64) -> i32 {
    molt_object_truthy(obj)
}

/// `PyObject_Not(obj)` — return 0 if obj is truthy, 1 if falsy, -1 on error.
pub extern "C" fn PyObject_Not(obj: u64) -> i32 {
    let t = PyObject_IsTrue(obj);
    match t {
        1 => 0,
        0 => 1,
        _ => -1,
    }
}

/// `PyObject_Type(obj)` — return the type of obj. Caller owns the reference.
pub extern "C" fn PyObject_Type(obj: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let res = molt_type_of(obj);
        if exception_pending(_py) || obj_from_bits(res).is_none() {
            return 0;
        }
        inc_ref_bits(_py, res);
        res
    })
}

/// `PyObject_Length(obj)` — return the length of obj, or -1 on error.
pub extern "C" fn PyObject_Length(obj: u64) -> isize {
    crate::with_gil_entry_nopanic!(_py, {
        let res = molt_len(obj);
        if exception_pending(_py) {
            return -1;
        }
        let n = to_i64(obj_from_bits(res)).unwrap_or(-1);
        dec_ref_bits(_py, res);
        n as isize
    })
}

/// `PyObject_Size(obj)` — alias for PyObject_Length.
pub extern "C" fn PyObject_Size(obj: u64) -> isize {
    PyObject_Length(obj)
}

/// `PyObject_SetAttr(obj, name, value)` — set obj.name = value. Returns 0 on success, -1 on error.
pub extern "C" fn PyObject_SetAttr(obj: u64, name: u64, value: u64) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let completed = crate::builtins::attributes::mutate_attr_name(
            _py,
            obj,
            name,
            crate::builtins::attr::DescriptorMutation::Set(value),
            molt_cpython_abi::hooks::AttributeMutation::Normal,
        );
        if completed.is_ok() { 0 } else { -1 }
    })
}

/// `PyObject_DelAttr(obj, name)` — delete obj.name. Returns 0 on success, -1 on error.
pub extern "C" fn PyObject_DelAttr(obj: u64, name: u64) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let completed = crate::builtins::attributes::mutate_attr_name(
            _py,
            obj,
            name,
            crate::builtins::attr::DescriptorMutation::Delete,
            molt_cpython_abi::hooks::AttributeMutation::Normal,
        );
        if completed.is_ok() { 0 } else { -1 }
    })
}

/// `PyObject_RichCompareBool(a, b, op)` — compare two objects.
/// op: Py_LT=0, Py_LE=1, Py_EQ=2, Py_NE=3, Py_GT=4, Py_GE=5
/// Returns 1 if true, 0 if false, -1 on error.
pub extern "C" fn PyObject_RichCompareBool(a: u64, b: u64, op: i32) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        if a == b && matches!(op, 2 | 3) {
            return i32::from(op == 2);
        }
        let res = match op {
            0 => molt_lt(a, b), // Py_LT
            1 => molt_le(a, b), // Py_LE
            2 => molt_eq(a, b), // Py_EQ
            3 => molt_ne(a, b), // Py_NE
            4 => molt_gt(a, b), // Py_GT
            5 => molt_ge(a, b), // Py_GE
            _ => {
                let _ = raise_exception::<u64>(
                    _py,
                    "SystemError",
                    "Bad internal call: invalid comparison op",
                );
                return -1;
            }
        };
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return -1;
        }
        let out = if is_truthy(_py, obj_from_bits(res)) {
            1
        } else {
            0
        };
        dec_ref_bits(_py, res);
        if exception_pending(_py) { -1 } else { out }
    })
}

/// `PyObject_RichCompare(a, b, op)` — compare two objects, returning the result object.
pub extern "C" fn PyObject_RichCompare(a: u64, b: u64, op: i32) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let res = match op {
            0 => molt_lt(a, b),
            1 => molt_le(a, b),
            2 => molt_eq(a, b),
            3 => molt_ne(a, b),
            4 => molt_gt(a, b),
            5 => molt_ge(a, b),
            _ => {
                let _ = raise_exception::<u64>(
                    _py,
                    "SystemError",
                    "Bad internal call: invalid comparison op",
                );
                return 0;
            }
        };
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyCallable_Check(obj)` — return 1 if obj is callable, 0 otherwise.
pub extern "C" fn PyCallable_Check(obj: u64) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let res = molt_callable_builtin(obj);
        if exception_pending(_py) {
            let _ = molt_exception_clear();
            return 0;
        }
        if is_truthy(_py, obj_from_bits(res)) {
            1
        } else {
            0
        }
    })
}

/// `PyObject_IsInstance(obj, cls)` — return 1 if isinstance(obj, cls), 0 if not, -1 on error.
pub extern "C" fn PyObject_IsInstance(obj: u64, cls: u64) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let res = molt_isinstance(obj, cls);
        if exception_pending(_py) {
            return -1;
        }
        if is_truthy(_py, obj_from_bits(res)) {
            1
        } else {
            0
        }
    })
}

/// `PySet_Check(obj)` — return 1 if obj is a set, 0 otherwise.
pub extern "C" fn PySet_Check(obj: u64) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(ptr) = obj_from_bits(obj).as_ptr() else {
            return 0;
        };
        unsafe {
            if object_type_id(ptr) == TYPE_ID_SET {
                1
            } else {
                0
            }
        }
    })
}

/// `PyFrozenSet_Check(obj)` — return 1 if obj is a frozenset, 0 otherwise.
pub extern "C" fn PyFrozenSet_Check(obj: u64) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(ptr) = obj_from_bits(obj).as_ptr() else {
            return 0;
        };
        unsafe {
            if object_type_id(ptr) == TYPE_ID_FROZENSET {
                1
            } else {
                0
            }
        }
    })
}

/// `PyUnicode_GetLength(obj)` — return the length of the Unicode string in code points.
pub extern "C" fn PyUnicode_GetLength(obj: u64) -> isize {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(ptr) = obj_from_bits(obj).as_ptr() else {
            let _ = raise_exception::<u64>(_py, "TypeError", "expected str object");
            return -1;
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_STRING {
                let _ = raise_exception::<u64>(_py, "TypeError", "expected str object");
                return -1;
            }
            string_len(ptr) as isize
        }
    })
}

/// `PyUnicode_Concat(left, right)` — return left + right as a new string, or 0 on error.
pub extern "C" fn PyUnicode_Concat(left: u64, right: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let res = molt_add(left, right);
        if exception_pending(_py) {
            if !obj_from_bits(res).is_none() {
                dec_ref_bits(_py, res);
            }
            return 0;
        }
        res
    })
}

/// `PyUnicode_Contains(container, element)` — return 1 if element in container, 0 if not, -1 on error.
pub extern "C" fn PyUnicode_Contains(container: u64, element: u64) -> i32 {
    molt_object_contains(container, element)
}

/// `PyUnicode_CompareWithASCIIString(uni, string)` — compare with a C ASCII string.
/// Returns -1, 0, or 1 for less, equal, greater.
pub extern "C" fn PyUnicode_CompareWithASCIIString(
    uni: u64,
    string: *const std::ffi::c_char,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        if string.is_null() {
            return -1;
        }
        let cstr = unsafe { std::ffi::CStr::from_ptr(string) };
        let rhs_bytes = cstr.to_bytes();
        let mut out_len: u64 = 0;
        let lhs_ptr = unsafe { molt_string_as_ptr(uni, &mut out_len as *mut u64) };
        if lhs_ptr.is_null() {
            if exception_pending(_py) {
                let _ = molt_exception_clear();
            }
            return -1;
        }
        let Ok(out_len) = usize::try_from(out_len) else {
            let _ = raise_exception::<u64>(
                _py,
                "OverflowError",
                "unicode length exceeds the active address space",
            );
            return -1;
        };
        let lhs = unsafe { std::slice::from_raw_parts(lhs_ptr, out_len) };
        lhs.cmp(rhs_bytes) as i32
    })
}

/// `PyDict_GetItemString(dict, key)` — get item using C string key. Borrowed reference.
pub extern "C" fn PyDict_GetItemString(dict: u64, key: *const std::ffi::c_char) -> u64 {
    molt_cpython_abi::api::errors::with_preserved_error(|| {
        crate::with_gil_entry_nopanic!(_py, {
            if key.is_null() {
                return 0;
            }
            let bytes = unsafe { std::ffi::CStr::from_ptr(key) }.to_bytes();
            let key_ptr = alloc_string(_py, bytes);
            if key_ptr.is_null() {
                return 0;
            }
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            let result = PyDict_GetItem(dict, key_bits);
            dec_ref_bits(_py, key_bits);
            result
        })
    })
}

/// `PyDict_DelItem(dict, key)` — delete dict[key]. Returns 0 on success, -1 on error.
pub extern "C" fn PyDict_DelItem(dict: u64, key: u64) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(storage) = c_dict_pointer(_py, dict) else {
            return -1;
        };
        match unsafe { crate::object::ops::dict_del_in_place(_py, storage, key) } {
            Ok(true) => 0,
            Ok(false) => {
                raise_key_error_with_key::<()>(_py, key);
                -1
            }
            Err(()) => -1,
        }
    })
}

/// `PyDict_Update(a, b)` — merge b into a. Returns 0 on success, -1 on error.
pub extern "C" fn PyDict_Update(a: u64, b: u64) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(storage) = c_dict_pointer(_py, a) else {
            return -1;
        };
        let a = MoltObject::from_ptr(storage).bits();
        let completed = unsafe {
            crate::object::ops_dict::dict_update_apply(
                _py,
                a,
                crate::object::ops_dict::dict_update_set_in_place,
                b,
            )
        };
        if completed { 0 } else { -1 }
    })
}

/// `PyList_Insert(list, index, item)` — insert item at index. Returns 0 on success, -1 on error.
pub extern "C" fn PyList_Insert(list: u64, index: isize, item: u64) -> i32 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(storage) = crate::object::ops_list::list_receiver(py, list, "insert") else {
            return -1;
        };
        if !storage.promote() {
            return -1;
        }
        let inserted = if let Some(ptr) = storage.managed() {
            unsafe {
                crate::object::ops_list::insert_at_native_index_with_projection(
                    py,
                    ptr,
                    index as i64,
                    item,
                    std::ptr::null_mut(),
                )
            }
        } else {
            storage.insert(index, item)
        };
        if inserted { 0 } else { -1 }
    })
}

/// `PyList_AsTuple(list)` — return a tuple with the same items as the list.
pub extern "C" fn PyList_AsTuple(list: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(ptr) = obj_from_bits(list).as_ptr() else {
            let _ = raise_exception::<u64>(_py, "TypeError", "expected list object");
            return 0;
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_LIST {
                let _ = raise_exception::<u64>(_py, "TypeError", "expected list object");
                return 0;
            }
            let Some(elems) = crate::object::seq_access::snapshot(
                _py,
                ptr,
                "list-to-tuple snapshot allocation failed",
            ) else {
                return 0;
            };
            let tuple_ptr = alloc_tuple(_py, &elems);
            if tuple_ptr.is_null() {
                return 0;
            }
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

/// `PyErr_SetString(type, message)` — set the current exception.
pub extern "C" fn PyErr_SetString(exc_type: u64, message: *const std::ffi::c_char) {
    crate::with_gil_entry_nopanic!(_py, {
        if message.is_null() {
            set_exception_from_message(_py, exc_type, b"<null message>");
            return;
        }
        let cstr = unsafe { std::ffi::CStr::from_ptr(message) };
        set_exception_from_message(_py, exc_type, cstr.to_bytes());
    })
}

/// `PyErr_Occurred()` — return the current exception type bits if an exception is pending, or 0.
pub extern "C" fn PyErr_Occurred() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if exception_pending(_py) {
            // Return a non-zero value to indicate an exception is pending.
            // In CPython this returns the exception type; we return a sentinel.
            1
        } else {
            0
        }
    })
}

/// `PyErr_Clear()` — clear the current exception.
pub extern "C" fn PyErr_Clear() {
    crate::with_gil_entry_nopanic!(_py, {
        if exception_pending(_py) {
            let _ = molt_exception_clear();
        }
    })
}

/// `PyErr_NoMemory()` — set MemoryError and return NULL (0).
pub extern "C" fn PyErr_NoMemory() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        crate::abi_return::fail_memory::<crate::abi_return::NullHandle>(_py)
    })
}

/// `Py_IncRef(obj)` — increment the reference count.
/// NOTE: the canonical C export is in `molt-cpython-abi`.
pub extern "C" fn Py_IncRef(obj: u64) {
    if obj == 0 {
        return;
    }
    crate::with_gil_entry_nopanic!(_py, {
        if !obj_from_bits(obj).is_none() {
            inc_ref_bits(_py, obj);
        }
    })
}

/// `Py_DecRef(obj)` — decrement the reference count.
/// NOTE: the canonical C export is in `molt-cpython-abi`.
pub extern "C" fn Py_DecRef(obj: u64) {
    if obj == 0 {
        return;
    }
    crate::with_gil_entry_nopanic!(_py, {
        if !obj_from_bits(obj).is_none() {
            dec_ref_bits(_py, obj);
        }
    })
}

/// `Py_XINCREF(obj)` — increment ref count if obj is non-NULL.
pub extern "C" fn Py_XINCREF(obj: u64) {
    Py_IncRef(obj)
}

/// `Py_XDECREF(obj)` — decrement ref count if obj is non-NULL.
pub extern "C" fn Py_XDECREF(obj: u64) {
    Py_DecRef(obj)
}

/// `PyLong_AsLong(obj)` — return the integer value as a C long, or -1 on error.
pub extern "C" fn PyLong_AsLong(obj: u64) -> i64 {
    crate::with_gil_entry_nopanic!(_py, {
        match to_i64(obj_from_bits(obj)) {
            Some(v) => v,
            None => {
                let _ = raise_exception::<u64>(_py, "TypeError", "an integer is required");
                -1
            }
        }
    })
}

/// `PyLong_FromLong(v)` — create a new integer from a C long.
pub extern "C" fn PyLong_FromLong(v: i64) -> u64 {
    MoltObject::from_int(v).bits()
}

/// `PyFloat_AsDouble(obj)` — return the float value as a C double, or -1.0 on error.
pub extern "C" fn PyFloat_AsDouble(obj: u64) -> f64 {
    crate::with_gil_entry_nopanic!(_py, {
        match to_f64(obj_from_bits(obj)) {
            Some(v) => v,
            None => {
                let _ = raise_exception::<u64>(_py, "TypeError", "must be real number, not str");
                -1.0
            }
        }
    })
}

/// `PyFloat_FromDouble(v)` — create a new float from a C double.
pub extern "C" fn PyFloat_FromDouble(v: f64) -> u64 {
    MoltObject::from_float(v).bits()
}

/// `PyBool_FromLong(v)` — return Py_True if v is nonzero, Py_False otherwise.
pub extern "C" fn PyBool_FromLong(v: i64) -> u64 {
    MoltObject::from_bool(v != 0).bits()
}

/// `Py_BuildNone()` — return None handle (borrowed).
pub extern "C" fn Py_BuildNone() -> u64 {
    none_bits()
}
