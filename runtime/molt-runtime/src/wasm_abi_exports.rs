use crate::{
    MoltObject, TYPE_ID_BIGINT, TYPE_ID_BOUND_METHOD, TYPE_ID_BUFFER2D, TYPE_ID_BYTEARRAY,
    TYPE_ID_BYTES, TYPE_ID_COMPLEX, TYPE_ID_DATACLASS, TYPE_ID_FLOAT, TYPE_ID_FROZENSET,
    TYPE_ID_INTARRAY, TYPE_ID_LIST, TYPE_ID_LIST_BOOL, TYPE_ID_LIST_INT, TYPE_ID_MEMORYVIEW,
    TYPE_ID_RANGE,
    TYPE_ID_SET, TYPE_ID_SLICE, TYPE_ID_STRING, TYPE_ID_TUPLE, TYPE_TAG_ANY, TYPE_TAG_BOOL,
    TYPE_TAG_BUFFER2D, TYPE_TAG_BYTEARRAY, TYPE_TAG_BYTES, TYPE_TAG_COMPLEX, TYPE_TAG_DATACLASS,
    TYPE_TAG_FLOAT, TYPE_TAG_FROZENSET, TYPE_TAG_INT, TYPE_TAG_INTARRAY, TYPE_TAG_LIST,
    TYPE_TAG_MEMORYVIEW, TYPE_TAG_NONE, TYPE_TAG_RANGE, TYPE_TAG_SET, TYPE_TAG_SLICE, TYPE_TAG_STR,
    TYPE_TAG_TUPLE, bound_method_self_bits, molt_dict_get, molt_index, molt_list_append,
    molt_store_index, molt_string_join, obj_from_bits, object_type_id, raise_exception,
};
use crate::object::ops_string::{
    molt_string_lower, molt_string_startswith, molt_string_strip, molt_string_upper,
};
use std::alloc::{Layout, alloc, dealloc};

#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_getitem(dict_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        // Fast path: we know the container is a dict (backend proved it).
        // Skip the type-dispatch chain in molt_index entirely.
        let obj = obj_from_bits(dict_bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                if object_type_id(ptr) == crate::TYPE_ID_DICT {
                    if let Some(val) = crate::dict_get_in_place(_py, ptr, key_bits) {
                        if obj_from_bits(val).as_ptr().is_some() {
                            crate::inc_ref_bits(_py, val);
                        }
                        return val;
                    }
                    return crate::raise_key_error_with_key(_py, key_bits);
                }
            }
        }
        // Fallback for non-dict (shouldn't happen if backend proved dict).
        molt_index(dict_bits, key_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tuple_getitem(tuple_bits: u64, index_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { molt_index(tuple_bits, index_bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_setitem(dict_bits: u64, key_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        // Fast path: we know the container is a dict (backend proved it).
        // Skip the type-dispatch chain in molt_store_index entirely.
        let obj = obj_from_bits(dict_bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                if object_type_id(ptr) == crate::TYPE_ID_DICT {
                    crate::dict_set_in_place(_py, ptr, key_bits, value_bits);
                    if crate::exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    return dict_bits;
                }
            }
        }
        // Fallback for non-dict (shouldn't happen if backend proved dict).
        let _ = molt_store_index(dict_bits, key_bits, value_bits);
        MoltObject::none().bits()
    })
}

fn bound_method_self_or_type_error(_py: &crate::PyToken<'_>, method_bits: u64) -> Result<u64, u64> {
    let Some(ptr) = obj_from_bits(method_bits).as_ptr() else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "expected bound method",
        ));
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_BOUND_METHOD {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "expected bound method",
            ));
        }
        Ok(bound_method_self_bits(ptr))
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fast_list_append(method_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let self_bits = match bound_method_self_or_type_error(_py, method_bits) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        molt_list_append(self_bits, value_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fast_str_join(method_bits: u64, iterable_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let self_bits = match bound_method_self_or_type_error(_py, method_bits) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        molt_string_join(self_bits, iterable_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fast_dict_get(method_bits: u64, key_bits: u64, default_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let self_bits = match bound_method_self_or_type_error(_py, method_bits) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        molt_dict_get(self_bits, key_bits, default_bits)
    })
}

// --- Fast-path string method dispatchers ---
// These extract the bound method's self and delegate directly to the
// optimized string operation, bypassing callargs allocation + IC dispatch.

#[unsafe(no_mangle)]
pub extern "C" fn molt_fast_str_startswith(method_bits: u64, prefix_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let self_bits = match bound_method_self_or_type_error(_py, method_bits) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        molt_string_startswith(self_bits, prefix_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fast_str_upper(method_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let self_bits = match bound_method_self_or_type_error(_py, method_bits) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        molt_string_upper(self_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fast_str_lower(method_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let self_bits = match bound_method_self_or_type_error(_py, method_bits) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        molt_string_lower(self_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fast_str_strip(method_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let self_bits = match bound_method_self_or_type_error(_py, method_bits) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        molt_string_strip(self_bits, MoltObject::none().bits())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_resource_on_allocate(size_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(size) = crate::to_i64(obj_from_bits(size_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "size must be an integer");
        };
        if size < 0 {
            return raise_exception::<_>(_py, "ValueError", "size must be non-negative");
        }
        match crate::resource::with_tracker(|tracker| tracker.on_allocate(size as usize)) {
            Ok(()) => MoltObject::from_int(0).bits(),
            Err(_) => MoltObject::from_int(1).bits(),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_resource_on_free(size_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(size) = crate::to_i64(obj_from_bits(size_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "size must be an integer");
        };
        if size < 0 {
            return raise_exception::<_>(_py, "ValueError", "size must be non-negative");
        }
        crate::resource::with_tracker(|tracker| tracker.on_free(size as usize));
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_type_tag_of_bits(bits: u64) -> i64 {
    let obj = obj_from_bits(bits);
    if obj.is_int() {
        return TYPE_TAG_INT;
    }
    if obj.is_float() {
        return TYPE_TAG_FLOAT;
    }
    if obj.is_bool() {
        return TYPE_TAG_BOOL;
    }
    if obj.is_none() {
        return TYPE_TAG_NONE;
    }
    let Some(ptr) = obj.as_ptr() else {
        return TYPE_TAG_ANY;
    };
    unsafe {
        match object_type_id(ptr) {
            TYPE_ID_STRING => TYPE_TAG_STR,
            TYPE_ID_BYTES => TYPE_TAG_BYTES,
            TYPE_ID_BYTEARRAY => TYPE_TAG_BYTEARRAY,
            TYPE_ID_LIST | TYPE_ID_LIST_INT | TYPE_ID_LIST_BOOL => TYPE_TAG_LIST,
            TYPE_ID_TUPLE => TYPE_TAG_TUPLE,
            TYPE_ID_RANGE => TYPE_TAG_RANGE,
            TYPE_ID_SLICE => TYPE_TAG_SLICE,
            TYPE_ID_DATACLASS => TYPE_TAG_DATACLASS,
            TYPE_ID_BUFFER2D => TYPE_TAG_BUFFER2D,
            TYPE_ID_MEMORYVIEW => TYPE_TAG_MEMORYVIEW,
            TYPE_ID_INTARRAY => TYPE_TAG_INTARRAY,
            TYPE_ID_SET => TYPE_TAG_SET,
            TYPE_ID_FROZENSET => TYPE_TAG_FROZENSET,
            TYPE_ID_COMPLEX => TYPE_TAG_COMPLEX,
            TYPE_ID_BIGINT => TYPE_TAG_INT,
            TYPE_ID_FLOAT => TYPE_TAG_FLOAT,
            _ => TYPE_TAG_ANY,
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_scratch_alloc(size: u64) -> u64 {
    let Ok(size) = usize::try_from(size) else {
        return 0;
    };
    let alloc_size = size.max(1);
    let Ok(layout) = Layout::from_size_align(alloc_size, 8) else {
        return 0;
    };
    let ptr = unsafe { alloc(layout) };
    ptr as usize as u64
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_scratch_free(ptr: u64, size: u64) {
    if ptr == 0 {
        return;
    }
    let Ok(size) = usize::try_from(size) else {
        return;
    };
    let alloc_size = size.max(1);
    let Ok(layout) = Layout::from_size_align(alloc_size, 8) else {
        return;
    };
    unsafe {
        dealloc(ptr as usize as *mut u8, layout);
    }
}
