//! Object-runtime boundary for the implementation owned by `molt-gpu`.
//!
//! Keep this file limited to ABI translation.  GPU algorithms, backend policy,
//! tensor semantics, and data transforms belong in the satellite crate.

#[unsafe(no_mangle)]
pub extern "C" fn __molt_gpu_raise_exception(
    kind_ptr: *const u8,
    kind_len: usize,
    message_ptr: *const u8,
    message_len: usize,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if (kind_ptr.is_null() && kind_len != 0) || (message_ptr.is_null() && message_len != 0) {
            return crate::MoltObject::none().bits();
        }
        let kind_bytes = if kind_len == 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(kind_ptr, kind_len) }
        };
        let message_bytes = if message_len == 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(message_ptr, message_len) }
        };
        let kind = std::str::from_utf8(kind_bytes).unwrap_or("RuntimeError");
        let message = String::from_utf8_lossy(message_bytes);
        crate::raise_exception::<u64>(_py, kind, &message)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_gpu_object_type_id(ptr: *mut u8) -> u32 {
    crate::with_gil_entry_nopanic!(_py, {
        if ptr.is_null() {
            0
        } else {
            unsafe { crate::object_type_id(ptr) }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_gpu_alloc_bytearray(data_ptr: *const u8, data_len: usize) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        let data = if data_len == 0 {
            &[]
        } else if data_ptr.is_null() {
            return std::ptr::null_mut();
        } else {
            unsafe { std::slice::from_raw_parts(data_ptr, data_len) }
        };
        crate::alloc_bytearray(_py, data)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_gpu_bytes_view(
    ptr: *mut u8,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        if ptr.is_null() || out_ptr.is_null() || out_len.is_null() {
            return 0;
        }
        let type_id = unsafe { crate::object_type_id(ptr) };
        if type_id != crate::TYPE_ID_BYTES && type_id != crate::TYPE_ID_BYTEARRAY {
            return 0;
        }
        unsafe {
            *out_ptr = crate::bytes_data(ptr);
            *out_len = crate::bytes_len(ptr);
        }
        1
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_gpu_to_i64(bits: u64, out: *mut i64) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        if out.is_null() {
            return 0;
        }
        match crate::to_i64(crate::obj_from_bits(bits)) {
            Some(value) => {
                unsafe { *out = value };
                1
            }
            None => 0,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_gpu_to_f64(bits: u64, out: *mut f64) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        if out.is_null() {
            return 0;
        }
        match crate::to_f64(crate::obj_from_bits(bits)) {
            Some(value) => {
                unsafe { *out = value };
                1
            }
            None => 0,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_gpu_attr_name_bits(data_ptr: *const u8, data_len: usize) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if data_ptr.is_null() && data_len != 0 {
            return crate::MoltObject::none().bits();
        }
        let data = if data_len == 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(data_ptr, data_len) }
        };
        crate::attr_name_bits_from_bytes(_py, data)
            .unwrap_or_else(|| crate::MoltObject::none().bits())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_gpu_object_setattr_raw(
    obj_ptr: *mut u8,
    name_bits: u64,
    name_ptr: *const u8,
    name_len: usize,
    value_bits: u64,
) -> i64 {
    crate::with_gil_entry_nopanic!(_py, {
        if obj_ptr.is_null() || (name_ptr.is_null() && name_len != 0) {
            return -1;
        }
        let bytes = if name_len == 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(name_ptr, name_len) }
        };
        let Ok(name) = std::str::from_utf8(bytes) else {
            return crate::raise_exception::<i64>(_py, "TypeError", "attribute name must be UTF-8");
        };
        unsafe {
            crate::builtins::attributes::object_setattr_raw(
                _py, obj_ptr, name_bits, name, value_bits,
            )
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_gpu_alloc_instance_for_class(class_ptr: *mut u8) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if class_ptr.is_null() {
            return crate::MoltObject::none().bits();
        }
        unsafe { crate::alloc_instance_for_class(_py, class_ptr) }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_gpu_builtin_float() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { crate::builtin_classes(_py).float })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_gpu_object_class_bits(ptr: *mut u8) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if ptr.is_null() {
            0
        } else {
            unsafe { crate::object_class_bits(ptr) }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_gpu_seq_len(ptr: *mut u8) -> usize {
    crate::with_gil_entry_nopanic!(_py, {
        if ptr.is_null() {
            0
        } else {
            unsafe { crate::object::seq_access::len(ptr) }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_gpu_seq_snapshot(
    ptr: *mut u8,
    _message_ptr: *const u8,
    _message_len: usize,
    out_ptr: *mut *const u64,
    out_len: *mut usize,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe { crate::seq_snapshot_bridge::export(_py, ptr, out_ptr, out_len) }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_gpu_seq_visit(
    ptr: *mut u8,
    visitor: unsafe extern "C" fn(*const u64, usize, *mut std::ffi::c_void),
    context: *mut std::ffi::c_void,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        if ptr.is_null() || context.is_null() {
            return 0;
        }
        unsafe {
            crate::object::seq_access::with_borrowed(ptr, |values| {
                visitor(values.as_ptr(), values.len(), context);
                1
            })
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_gpu_seq_pin_item(ptr: *mut u8, index: usize, out: *mut u64) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        if ptr.is_null() || out.is_null() {
            return 0;
        }
        crate::object::seq_access::read_item_owned(ptr, index, out)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_gpu_alloc_list_owned(
    elems_ptr: *const u64,
    elems_len: usize,
    capacity: usize,
) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        if elems_ptr.is_null() && elems_len != 0 {
            return std::ptr::null_mut();
        }
        let elems = if elems_len == 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(elems_ptr, elems_len) }
        };
        crate::object::builders::alloc_list_with_capacity_owned(_py, elems, capacity)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_gpu_callargs_positional_snapshot(
    builder_bits: u64,
    out_ptr: *mut *const u64,
    out_len: *mut usize,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        if out_ptr.is_null() || out_len.is_null() {
            return 0;
        }
        let values =
            match unsafe { crate::call::bind::callargs_positional_snapshot(_py, builder_bits) } {
                Ok(values) => values,
                Err(_) => return 0,
            };
        unsafe { crate::resource::bridge_buffer::export_u64_slice(&values, out_ptr, out_len) }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_gpu_clone_callargs_builder(builder_bits: u64, out: *mut u64) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        if out.is_null() {
            return 0;
        }
        match unsafe { crate::call::bind::clone_callargs_builder_bits(_py, builder_bits) } {
            Ok(bits) => {
                unsafe { *out = bits };
                1
            }
            Err(bits) => {
                unsafe { *out = bits };
                0
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_gpu_missing_bits() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { crate::missing_bits(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_gpu_call_callable1(call_bits: u64, arg_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe { crate::call::dispatch::call_callable1(_py, call_bits, arg_bits) }
    })
}
