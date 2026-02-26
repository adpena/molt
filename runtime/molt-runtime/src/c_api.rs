use crate::builtins::exceptions::molt_exception_new_from_class;
use crate::concurrency::gil::{gil_held, release_runtime_gil};
use crate::state::runtime_state::{
    molt_runtime_ensure_gil, molt_runtime_init, molt_runtime_shutdown,
};
use crate::*;

/// libmolt C-API surface version.
pub const MOLT_C_API_VERSION: u32 = 1;

/// Opaque object handle used by the libmolt C-API.
pub type MoltHandle = u64;

#[repr(C)]
pub struct MoltBufferView {
    pub data: *mut u8,
    pub len: u64,
    pub readonly: u32,
    pub reserved: u32,
    pub stride: i64,
    pub itemsize: u64,
    pub owner: MoltHandle,
}

#[inline]
fn none_bits() -> u64 {
    MoltObject::none().bits()
}

#[inline]
fn runtime_error_type_bits(_py: &PyToken<'_>) -> u64 {
    let bits = exception_type_bits_from_name(_py, "RuntimeError");
    if bits == 0 {
        builtin_classes(_py).exception
    } else {
        bits
    }
}

#[inline]
unsafe fn bytes_slice_from_raw<'a>(data: *const u8, len_bits: u64) -> Option<&'a [u8]> {
    let len = usize::try_from(len_bits).ok()?;
    if len == 0 {
        return Some(&[]);
    }
    if data.is_null() {
        return None;
    }
    Some(unsafe { std::slice::from_raw_parts(data, len) })
}

#[inline]
unsafe fn handles_slice_from_raw<'a>(
    data: *const MoltHandle,
    len_bits: u64,
) -> Option<&'a [MoltHandle]> {
    let len = usize::try_from(len_bits).ok()?;
    if len == 0 {
        return Some(&[]);
    }
    if data.is_null() {
        return None;
    }
    Some(unsafe { std::slice::from_raw_parts(data, len) })
}

#[inline]
fn bool_handle_to_i32(_py: &PyToken<'_>, bits: MoltHandle) -> i32 {
    if exception_pending(_py) {
        if bits != 0 {
            dec_ref_bits(_py, bits);
        }
        return -1;
    }
    let out = if is_truthy(_py, obj_from_bits(bits)) {
        1
    } else {
        0
    };
    let truthy_error = exception_pending(_py);
    if bits != 0 {
        dec_ref_bits(_py, bits);
    }
    if truthy_error { -1 } else { out }
}

#[inline]
fn set_exception_from_message(_py: &PyToken<'_>, exc_type_bits: u64, message: &[u8]) -> i32 {
    let msg_ptr = alloc_string(_py, message);
    if msg_ptr.is_null() {
        return -1;
    }
    let msg_bits = MoltObject::from_ptr(msg_ptr).bits();
    let class_bits = if exc_type_bits == 0 || obj_from_bits(exc_type_bits).is_none() {
        runtime_error_type_bits(_py)
    } else {
        exc_type_bits
    };
    let exc_bits = molt_exception_new_from_class(class_bits, msg_bits);
    dec_ref_bits(_py, msg_bits);
    if obj_from_bits(exc_bits).is_none() {
        return -1;
    }
    let _ = molt_exception_set_last(exc_bits);
    dec_ref_bits(_py, exc_bits);
    if exception_pending(_py) { 0 } else { -1 }
}

#[inline]
fn require_type_handle(_py: &PyToken<'_>, type_bits: MoltHandle) -> Result<*mut u8, i32> {
    let Some(type_ptr) = obj_from_bits(type_bits).as_ptr() else {
        return Err(raise_exception::<i32>(
            _py,
            "TypeError",
            "type object expected",
        ));
    };
    unsafe {
        if object_type_id(type_ptr) != TYPE_ID_TYPE {
            return Err(raise_exception::<i32>(
                _py,
                "TypeError",
                "type object expected",
            ));
        }
    }
    Ok(type_ptr)
}

#[inline]
fn require_string_handle(
    _py: &PyToken<'_>,
    value_bits: MoltHandle,
    label: &str,
) -> Result<*mut u8, i32> {
    let Some(value_ptr) = obj_from_bits(value_bits).as_ptr() else {
        return Err(raise_exception::<i32>(
            _py,
            "TypeError",
            &format!("{label} must be str"),
        ));
    };
    unsafe {
        if object_type_id(value_ptr) != TYPE_ID_STRING {
            return Err(raise_exception::<i32>(
                _py,
                "TypeError",
                &format!("{label} must be str"),
            ));
        }
    }
    Ok(value_ptr)
}

#[inline]
fn module_add_object_impl(
    _py: &PyToken<'_>,
    module_bits: MoltHandle,
    name_bits: MoltHandle,
    value_bits: MoltHandle,
) -> i32 {
    if require_string_handle(_py, name_bits, "module attribute name").is_err() {
        return -1;
    }
    let set_out = molt_module_set_attr(module_bits, name_bits, value_bits);
    if set_out != 0 {
        dec_ref_bits(_py, set_out);
    }
    if exception_pending(_py) { -1 } else { 0 }
}

#[inline]
fn module_get_object_impl(
    _py: &PyToken<'_>,
    module_bits: MoltHandle,
    name_bits: MoltHandle,
) -> MoltHandle {
    if require_string_handle(_py, name_bits, "module attribute name").is_err() {
        return none_bits();
    }
    molt_module_get_attr(module_bits, name_bits)
}

#[inline]
fn callargs_builder_for_call(
    _py: &PyToken<'_>,
    callable_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    let mut pos: &[u64] = &[];
    if !obj_from_bits(args_bits).is_none() {
        let Some(args_ptr) = obj_from_bits(args_bits).as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "args must be tuple or list");
        };
        unsafe {
            let type_id = object_type_id(args_ptr);
            if type_id != TYPE_ID_TUPLE && type_id != TYPE_ID_LIST {
                return raise_exception::<u64>(_py, "TypeError", "args must be tuple or list");
            }
            pos = seq_vec_ref(args_ptr);
        }
    }

    let builder_bits = molt_callargs_new(pos.len() as u64, 0);
    if builder_bits == 0 || obj_from_bits(builder_bits).is_none() {
        return none_bits();
    }

    for &val in pos {
        let _ = unsafe { molt_callargs_push_pos(builder_bits, val) };
        if exception_pending(_py) {
            dec_ref_bits(_py, builder_bits);
            return none_bits();
        }
    }

    if !obj_from_bits(kwargs_bits).is_none() {
        let _ = unsafe { molt_callargs_expand_kwstar(builder_bits, kwargs_bits) };
        if exception_pending(_py) {
            dec_ref_bits(_py, builder_bits);
            return none_bits();
        }
    }

    molt_call_bind(callable_bits, builder_bits)
}

#[inline]
fn len_bits_to_i64(_py: &PyToken<'_>, len_bits: u64) -> i64 {
    match to_i64(obj_from_bits(len_bits)) {
        Some(v) => v,
        None => {
            let _ =
                raise_exception::<u64>(_py, "OverflowError", "sequence length does not fit in i64");
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_c_api_version() -> u32 {
    MOLT_C_API_VERSION
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_init() -> i32 {
    if molt_runtime_init() == 0 { -1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_shutdown() -> i32 {
    // Shutdown returning 0 means "already shut down", which is still a clean state.
    let _ = molt_runtime_shutdown();
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gil_acquire() -> i32 {
    molt_runtime_ensure_gil();
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gil_release() -> i32 {
    release_runtime_gil();
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gil_is_held() -> i32 {
    if gil_held() { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_handle_incref(handle: MoltHandle) {
    crate::with_gil_entry!(_py, {
        inc_ref_bits(_py, handle);
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_handle_decref(handle: MoltHandle) {
    crate::with_gil_entry!(_py, {
        dec_ref_bits(_py, handle);
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_none() -> MoltHandle {
    none_bits()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bool_from_i32(value: i32) -> MoltHandle {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(value != 0).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_from_i64(value: i64) -> MoltHandle {
    crate::with_gil_entry!(_py, { MoltObject::from_int(value).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_as_i64(value_bits: MoltHandle) -> i64 {
    crate::with_gil_entry!(_py, {
        if let Some(value) = to_i64(obj_from_bits(value_bits)) {
            return value;
        }
        let _ = raise_exception::<u64>(_py, "TypeError", "int-compatible object expected");
        -1
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_float_from_f64(value: f64) -> MoltHandle {
    crate::with_gil_entry!(_py, { MoltObject::from_float(value).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_float_as_f64(value_bits: MoltHandle) -> f64 {
    crate::with_gil_entry!(_py, {
        let value_obj = obj_from_bits(value_bits);
        if let Some(value) = value_obj.as_float() {
            return value;
        }
        if let Some(value) = to_i64(value_obj) {
            return value as f64;
        }
        let _ = raise_exception::<u64>(_py, "TypeError", "float-compatible object expected");
        -1.0
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_err_set(
    exc_type_bits: MoltHandle,
    message_ptr: *const u8,
    message_len: u64,
) -> i32 {
    crate::with_gil_entry!(_py, {
        let Some(message) = (unsafe { bytes_slice_from_raw(message_ptr, message_len) }) else {
            return raise_exception::<i32>(
                _py,
                "TypeError",
                "exception message pointer cannot be null when len > 0",
            );
        };
        set_exception_from_message(_py, exc_type_bits, message)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_err_format(
    exc_type_bits: MoltHandle,
    message_ptr: *const u8,
    message_len: u64,
) -> i32 {
    unsafe { molt_err_set(exc_type_bits, message_ptr, message_len) }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_err_clear() -> i32 {
    let _ = molt_exception_clear();
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_err_pending() -> i32 {
    crate::with_gil_entry!(_py, { if exception_pending(_py) { 1 } else { 0 } })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_err_peek() -> MoltHandle {
    crate::with_gil_entry!(_py, {
        if !exception_pending(_py) {
            return none_bits();
        }
        molt_exception_last()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_err_fetch() -> MoltHandle {
    crate::with_gil_entry!(_py, {
        if !exception_pending(_py) {
            return none_bits();
        }
        let exc_bits = molt_exception_last();
        let _ = molt_exception_clear();
        exc_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_err_restore(exc_bits: MoltHandle) -> i32 {
    crate::with_gil_entry!(_py, {
        if obj_from_bits(exc_bits).is_none() {
            return -1;
        }
        let _ = molt_exception_set_last(exc_bits);
        if exception_pending(_py) { 0 } else { -1 }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_err_matches(exc_type_bits: MoltHandle) -> i32 {
    crate::with_gil_entry!(_py, {
        let Some(exc_type_ptr) = obj_from_bits(exc_type_bits).as_ptr() else {
            return -1;
        };
        unsafe {
            if object_type_id(exc_type_ptr) != TYPE_ID_TYPE {
                return -1;
            }
        }
        if !exception_pending(_py) {
            return 0;
        }
        let Some(exc_bits) = exception_last_bits_noinc(_py) else {
            return 0;
        };
        let Some(exc_ptr) = obj_from_bits(exc_bits).as_ptr() else {
            return 0;
        };
        let mut class_bits = unsafe { exception_class_bits(exc_ptr) };
        if class_bits == 0 || obj_from_bits(class_bits).is_none() {
            let kind_bits = unsafe { exception_kind_bits(exc_ptr) };
            class_bits = exception_type_bits(_py, kind_bits);
        }
        if class_bits == 0 || obj_from_bits(class_bits).is_none() {
            return 0;
        }
        let matches = issubclass_bits(class_bits, exc_type_bits);
        if matches { 1 } else { 0 }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_getattr(obj_bits: MoltHandle, name_bits: MoltHandle) -> MoltHandle {
    molt_get_attr_name(obj_bits, name_bits)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_object_getattr_bytes(
    obj_bits: MoltHandle,
    name_ptr: *const u8,
    name_len: u64,
) -> MoltHandle {
    crate::with_gil_entry!(_py, {
        let Some(name_bytes) = (unsafe { bytes_slice_from_raw(name_ptr, name_len) }) else {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "attribute name pointer cannot be null when len > 0",
            );
        };
        let Some(name_bits) = crate::builtins::attr::attr_name_bits_from_bytes(_py, name_bytes)
        else {
            if exception_pending(_py) {
                return none_bits();
            }
            return raise_exception::<u64>(_py, "RuntimeError", "failed to intern attribute name");
        };
        let out = molt_get_attr_name(obj_bits, name_bits);
        dec_ref_bits(_py, name_bits);
        out
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_object_setattr_bytes(
    obj_bits: MoltHandle,
    name_ptr: *const u8,
    name_len: u64,
    val_bits: MoltHandle,
) -> i32 {
    crate::with_gil_entry!(_py, {
        let Some(name_bytes) = (unsafe { bytes_slice_from_raw(name_ptr, name_len) }) else {
            return raise_exception::<i32>(
                _py,
                "TypeError",
                "attribute name pointer cannot be null when len > 0",
            );
        };
        let Some(name_bits) = crate::builtins::attr::attr_name_bits_from_bytes(_py, name_bytes)
        else {
            if exception_pending(_py) {
                return -1;
            }
            return raise_exception::<i32>(_py, "RuntimeError", "failed to intern attribute name");
        };
        let set_out = molt_set_attr_name(obj_bits, name_bits, val_bits);
        dec_ref_bits(_py, name_bits);
        if set_out != 0 {
            dec_ref_bits(_py, set_out);
        }
        if exception_pending(_py) { -1 } else { 0 }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_hasattr(obj_bits: MoltHandle, name_bits: MoltHandle) -> i32 {
    crate::with_gil_entry!(_py, {
        let has_bits = molt_has_attr_name(obj_bits, name_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, has_bits);
            return -1;
        }
        let out = if is_truthy(_py, obj_from_bits(has_bits)) {
            1
        } else {
            0
        };
        dec_ref_bits(_py, has_bits);
        out
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_call(
    callable_bits: MoltHandle,
    args_bits: MoltHandle,
    kwargs_bits: MoltHandle,
) -> MoltHandle {
    crate::with_gil_entry!(_py, {
        callargs_builder_for_call(_py, callable_bits, args_bits, kwargs_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_repr(obj_bits: MoltHandle) -> MoltHandle {
    molt_repr_from_obj(obj_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_str(obj_bits: MoltHandle) -> MoltHandle {
    molt_str_from_obj(obj_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_truthy(obj_bits: MoltHandle) -> i32 {
    crate::with_gil_entry!(_py, {
        let out = is_truthy(_py, obj_from_bits(obj_bits));
        if exception_pending(_py) {
            -1
        } else if out {
            1
        } else {
            0
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_equal(lhs_bits: MoltHandle, rhs_bits: MoltHandle) -> i32 {
    crate::with_gil_entry!(_py, {
        let eq_bits = molt_eq(lhs_bits, rhs_bits);
        bool_handle_to_i32(_py, eq_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_not_equal(lhs_bits: MoltHandle, rhs_bits: MoltHandle) -> i32 {
    crate::with_gil_entry!(_py, {
        let ne_bits = molt_ne(lhs_bits, rhs_bits);
        bool_handle_to_i32(_py, ne_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_contains(container_bits: MoltHandle, item_bits: MoltHandle) -> i32 {
    crate::with_gil_entry!(_py, {
        let contains_bits = molt_contains(container_bits, item_bits);
        bool_handle_to_i32(_py, contains_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_type_ready(type_bits: MoltHandle) -> i32 {
    crate::with_gil_entry!(_py, {
        if require_type_handle(_py, type_bits).is_err() {
            return -1;
        }
        0
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_create(name_bits: MoltHandle) -> MoltHandle {
    molt_module_new(name_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_get_dict(module_bits: MoltHandle) -> MoltHandle {
    crate::with_gil_entry!(_py, {
        let Some(module_ptr) = obj_from_bits(module_bits).as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "module object expected");
        };
        unsafe {
            if object_type_id(module_ptr) != TYPE_ID_MODULE {
                return raise_exception::<u64>(_py, "TypeError", "module object expected");
            }
            let dict_bits = module_dict_bits(module_ptr);
            if obj_from_bits(dict_bits).is_none() {
                return raise_exception::<u64>(_py, "RuntimeError", "module dict missing");
            }
            inc_ref_bits(_py, dict_bits);
            dict_bits
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_add_object(
    module_bits: MoltHandle,
    name_bits: MoltHandle,
    value_bits: MoltHandle,
) -> i32 {
    crate::with_gil_entry!(_py, {
        module_add_object_impl(_py, module_bits, name_bits, value_bits)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_module_add_object_bytes(
    module_bits: MoltHandle,
    name_ptr: *const u8,
    name_len: u64,
    value_bits: MoltHandle,
) -> i32 {
    crate::with_gil_entry!(_py, {
        let Some(name_bytes) = (unsafe { bytes_slice_from_raw(name_ptr, name_len) }) else {
            return raise_exception::<i32>(
                _py,
                "TypeError",
                "module attribute name pointer cannot be null when len > 0",
            );
        };
        let Some(name_bits) = crate::builtins::attr::attr_name_bits_from_bytes(_py, name_bytes)
        else {
            if exception_pending(_py) {
                return -1;
            }
            return raise_exception::<i32>(
                _py,
                "RuntimeError",
                "failed to intern module attribute name",
            );
        };
        let rc = module_add_object_impl(_py, module_bits, name_bits, value_bits);
        dec_ref_bits(_py, name_bits);
        rc
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_get_object(
    module_bits: MoltHandle,
    name_bits: MoltHandle,
) -> MoltHandle {
    crate::with_gil_entry!(_py, { module_get_object_impl(_py, module_bits, name_bits) })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_module_get_object_bytes(
    module_bits: MoltHandle,
    name_ptr: *const u8,
    name_len: u64,
) -> MoltHandle {
    crate::with_gil_entry!(_py, {
        let Some(name_bytes) = (unsafe { bytes_slice_from_raw(name_ptr, name_len) }) else {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "module attribute name pointer cannot be null when len > 0",
            );
        };
        let Some(name_bits) = crate::builtins::attr::attr_name_bits_from_bytes(_py, name_bytes)
        else {
            if exception_pending(_py) {
                return none_bits();
            }
            return raise_exception::<u64>(
                _py,
                "RuntimeError",
                "failed to intern module attribute name",
            );
        };
        let out = module_get_object_impl(_py, module_bits, name_bits);
        dec_ref_bits(_py, name_bits);
        out
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_add_int_constant(
    module_bits: MoltHandle,
    name_bits: MoltHandle,
    value: i64,
) -> i32 {
    crate::with_gil_entry!(_py, {
        module_add_object_impl(
            _py,
            module_bits,
            name_bits,
            MoltObject::from_int(value).bits(),
        )
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_module_add_string_constant(
    module_bits: MoltHandle,
    name_bits: MoltHandle,
    value_ptr: *const u8,
    value_len: u64,
) -> i32 {
    crate::with_gil_entry!(_py, {
        let Some(value_bytes) = (unsafe { bytes_slice_from_raw(value_ptr, value_len) }) else {
            return raise_exception::<i32>(
                _py,
                "TypeError",
                "string constant pointer cannot be null when len > 0",
            );
        };
        let string_ptr = alloc_string(_py, value_bytes);
        if string_ptr.is_null() {
            return -1;
        }
        let string_bits = MoltObject::from_ptr(string_ptr).bits();
        let rc = module_add_object_impl(_py, module_bits, name_bits, string_bits);
        dec_ref_bits(_py, string_bits);
        rc
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_number_add(a_bits: MoltHandle, b_bits: MoltHandle) -> MoltHandle {
    molt_add(a_bits, b_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_number_sub(a_bits: MoltHandle, b_bits: MoltHandle) -> MoltHandle {
    molt_sub(a_bits, b_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_number_mul(a_bits: MoltHandle, b_bits: MoltHandle) -> MoltHandle {
    molt_mul(a_bits, b_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_number_truediv(a_bits: MoltHandle, b_bits: MoltHandle) -> MoltHandle {
    molt_div(a_bits, b_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_number_floordiv(a_bits: MoltHandle, b_bits: MoltHandle) -> MoltHandle {
    molt_floordiv(a_bits, b_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_number_long(obj_bits: MoltHandle) -> MoltHandle {
    molt_int_from_obj(obj_bits, none_bits(), 0)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_number_float(obj_bits: MoltHandle) -> MoltHandle {
    molt_float_from_obj(obj_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sequence_length(seq_bits: MoltHandle) -> i64 {
    crate::with_gil_entry!(_py, {
        let len_bits = molt_len(seq_bits);
        if exception_pending(_py) {
            return -1;
        }
        let out = len_bits_to_i64(_py, len_bits);
        dec_ref_bits(_py, len_bits);
        out
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sequence_getitem(seq_bits: MoltHandle, key_bits: MoltHandle) -> MoltHandle {
    molt_getitem_method(seq_bits, key_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sequence_setitem(
    seq_bits: MoltHandle,
    key_bits: MoltHandle,
    val_bits: MoltHandle,
) -> i32 {
    crate::with_gil_entry!(_py, {
        let _ = molt_setitem_method(seq_bits, key_bits, val_bits);
        if exception_pending(_py) { -1 } else { 0 }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_mapping_getitem(
    mapping_bits: MoltHandle,
    key_bits: MoltHandle,
) -> MoltHandle {
    molt_getitem_method(mapping_bits, key_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_mapping_setitem(
    mapping_bits: MoltHandle,
    key_bits: MoltHandle,
    val_bits: MoltHandle,
) -> i32 {
    crate::with_gil_entry!(_py, {
        let _ = molt_setitem_method(mapping_bits, key_bits, val_bits);
        if exception_pending(_py) { -1 } else { 0 }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_mapping_length(mapping_bits: MoltHandle) -> i64 {
    crate::with_gil_entry!(_py, {
        let len_bits = molt_len(mapping_bits);
        if exception_pending(_py) {
            return -1;
        }
        let out = len_bits_to_i64(_py, len_bits);
        dec_ref_bits(_py, len_bits);
        out
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_mapping_keys(mapping_bits: MoltHandle) -> MoltHandle {
    crate::with_gil_entry!(_py, {
        let keys_method_bits =
            unsafe { molt_object_getattr_bytes(mapping_bits, b"keys".as_ptr(), 4) };
        if exception_pending(_py) {
            if !obj_from_bits(keys_method_bits).is_none() {
                dec_ref_bits(_py, keys_method_bits);
            }
            return none_bits();
        }
        let out = molt_object_call(keys_method_bits, none_bits(), none_bits());
        if !obj_from_bits(keys_method_bits).is_none() {
            dec_ref_bits(_py, keys_method_bits);
        }
        if exception_pending(_py) {
            if !obj_from_bits(out).is_none() {
                dec_ref_bits(_py, out);
            }
            return none_bits();
        }
        out
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_tuple_from_array(items: *const MoltHandle, len: u64) -> MoltHandle {
    crate::with_gil_entry!(_py, {
        let Some(elems) = (unsafe { handles_slice_from_raw(items, len) }) else {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "tuple source pointer cannot be null when len > 0",
            );
        };
        let ptr = alloc_tuple(_py, elems);
        if ptr.is_null() {
            return none_bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_list_from_array(items: *const MoltHandle, len: u64) -> MoltHandle {
    crate::with_gil_entry!(_py, {
        let Some(elems) = (unsafe { handles_slice_from_raw(items, len) }) else {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "list source pointer cannot be null when len > 0",
            );
        };
        let ptr = alloc_list(_py, elems);
        if ptr.is_null() {
            return none_bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_dict_from_pairs(
    keys: *const MoltHandle,
    values: *const MoltHandle,
    len: u64,
) -> MoltHandle {
    crate::with_gil_entry!(_py, {
        let Some(key_slice) = (unsafe { handles_slice_from_raw(keys, len) }) else {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "dict key pointer cannot be null when len > 0",
            );
        };
        let Some(value_slice) = (unsafe { handles_slice_from_raw(values, len) }) else {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "dict value pointer cannot be null when len > 0",
            );
        };
        let mut pairs = Vec::with_capacity(key_slice.len().saturating_mul(2));
        for (&key, &value) in key_slice.iter().zip(value_slice.iter()) {
            pairs.push(key);
            pairs.push(value);
        }
        let ptr = alloc_dict_with_pairs(_py, pairs.as_slice());
        if ptr.is_null() {
            return none_bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_buffer_acquire(
    obj_bits: MoltHandle,
    out_view: *mut MoltBufferView,
) -> i32 {
    crate::with_gil_entry!(_py, {
        if out_view.is_null() {
            return raise_exception::<i32>(_py, "TypeError", "out_view cannot be null");
        }
        let mut export = BufferExport {
            ptr: 0,
            len: 0,
            readonly: 1,
            stride: 1,
            itemsize: 1,
        };
        if unsafe { molt_buffer_export(obj_bits, &mut export as *mut BufferExport) } != 0 {
            return -1;
        }
        inc_ref_bits(_py, obj_bits);
        unsafe {
            *out_view = MoltBufferView {
                data: export.ptr as usize as *mut u8,
                len: export.len,
                readonly: export.readonly as u32,
                reserved: 0,
                stride: export.stride,
                itemsize: export.itemsize,
                owner: obj_bits,
            };
        }
        0
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_buffer_release(view: *mut MoltBufferView) -> i32 {
    crate::with_gil_entry!(_py, {
        if view.is_null() {
            return -1;
        }
        unsafe {
            if (*view).owner != 0 {
                dec_ref_bits(_py, (*view).owner);
            }
            (*view).data = std::ptr::null_mut();
            (*view).len = 0;
            (*view).readonly = 1;
            (*view).reserved = 0;
            (*view).stride = 1;
            (*view).itemsize = 1;
            (*view).owner = 0;
        }
        0
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_bytes_from(data: *const u8, len: u64) -> MoltHandle {
    crate::with_gil_entry!(_py, {
        let Some(bytes) = (unsafe { bytes_slice_from_raw(data, len) }) else {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "bytes source pointer cannot be null when len > 0",
            );
        };
        let ptr = alloc_bytes(_py, bytes);
        if ptr.is_null() {
            return none_bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_bytes_as_ptr(bytes_bits: MoltHandle, out_len: *mut u64) -> *const u8 {
    crate::with_gil_entry!(_py, {
        let Some(ptr) = obj_from_bits(bytes_bits).as_ptr() else {
            let _ = raise_exception::<u64>(_py, "TypeError", "bytes object expected");
            return std::ptr::null();
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_BYTES {
                let _ = raise_exception::<u64>(_py, "TypeError", "bytes object expected");
                return std::ptr::null();
            }
            if !out_len.is_null() {
                *out_len = bytes_len(ptr) as u64;
            }
            bytes_data(ptr)
        }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_string_from(data: *const u8, len: u64) -> MoltHandle {
    crate::with_gil_entry!(_py, {
        let Some(bytes) = (unsafe { bytes_slice_from_raw(data, len) }) else {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "string source pointer cannot be null when len > 0",
            );
        };
        let ptr = alloc_string(_py, bytes);
        if ptr.is_null() {
            return none_bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_string_as_ptr(
    string_bits: MoltHandle,
    out_len: *mut u64,
) -> *const u8 {
    crate::with_gil_entry!(_py, {
        let Some(ptr) = obj_from_bits(string_bits).as_ptr() else {
            let _ = raise_exception::<u64>(_py, "TypeError", "string object expected");
            return std::ptr::null();
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_STRING {
                let _ = raise_exception::<u64>(_py, "TypeError", "string object expected");
                return std::ptr::null();
            }
            if !out_len.is_null() {
                *out_len = string_len(ptr) as u64;
            }
            string_bytes(ptr)
        }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_bytearray_from(data: *const u8, len: u64) -> MoltHandle {
    crate::with_gil_entry!(_py, {
        let Some(bytes) = (unsafe { bytes_slice_from_raw(data, len) }) else {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "bytearray source pointer cannot be null when len > 0",
            );
        };
        let ptr = alloc_bytearray(_py, bytes);
        if ptr.is_null() {
            return none_bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_bytearray_as_ptr(
    bytearray_bits: MoltHandle,
    out_len: *mut u64,
) -> *mut u8 {
    crate::with_gil_entry!(_py, {
        let Some(ptr) = obj_from_bits(bytearray_bits).as_ptr() else {
            let _ = raise_exception::<u64>(_py, "TypeError", "bytearray object expected");
            return std::ptr::null_mut();
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_BYTEARRAY {
                let _ = raise_exception::<u64>(_py, "TypeError", "bytearray object expected");
                return std::ptr::null_mut();
            }
            let vec_ptr = bytearray_vec_ptr(ptr);
            if vec_ptr.is_null() {
                return std::ptr::null_mut();
            }
            let data = (*vec_ptr).as_mut_ptr();
            if !out_len.is_null() {
                *out_len = (*vec_ptr).len() as u64;
            }
            data
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtins::exceptions::molt_exception_class;

    #[test]
    fn c_api_version_is_nonzero() {
        assert!(molt_c_api_version() >= 1);
    }

    #[test]
    fn err_set_matches_fetch_roundtrip() {
        let _ = molt_runtime_init();
        let runtime_error = crate::with_gil_entry!(_py, { runtime_error_type_bits(_py) });
        let msg = b"boom";
        let rc = unsafe { molt_err_set(runtime_error, msg.as_ptr(), msg.len() as u64) };
        assert_eq!(rc, 0);
        assert_eq!(molt_exception_pending(), 1);
        assert_eq!(molt_err_matches(runtime_error), 1);
        let exc_bits = molt_err_fetch();
        assert!(!obj_from_bits(exc_bits).is_none());
        assert_eq!(molt_exception_pending(), 0);
        let kind_bits = molt_exception_kind(exc_bits);
        let class_bits = molt_exception_class(kind_bits);
        assert_eq!(molt_err_matches(runtime_error), 0);
        assert_eq!(issubclass_bits(class_bits, runtime_error), true);
        crate::with_gil_entry!(_py, {
            dec_ref_bits(_py, kind_bits);
            dec_ref_bits(_py, class_bits);
            dec_ref_bits(_py, exc_bits);
        });
    }

    #[test]
    fn object_call_numeric_and_sequence_wrappers() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let list_ptr = alloc_list(
                _py,
                &[
                    MoltObject::from_int(3).bits(),
                    MoltObject::from_int(4).bits(),
                ],
            );
            assert!(!list_ptr.is_null());
            let list_bits = MoltObject::from_ptr(list_ptr).bits();

            let append_name_ptr = alloc_string(_py, b"append");
            assert!(!append_name_ptr.is_null());
            let append_name_bits = MoltObject::from_ptr(append_name_ptr).bits();
            let append_bits = molt_object_getattr(list_bits, append_name_bits);
            assert!(!obj_from_bits(append_bits).is_none());
            let append_args_ptr = alloc_tuple(_py, &[MoltObject::from_int(5).bits()]);
            assert!(!append_args_ptr.is_null());
            let append_args_bits = MoltObject::from_ptr(append_args_ptr).bits();
            let append_out = molt_object_call(append_bits, append_args_bits, none_bits());
            assert!(!exception_pending(_py));
            assert!(obj_from_bits(append_out).is_none());
            dec_ref_bits(_py, append_args_bits);
            dec_ref_bits(_py, append_bits);
            dec_ref_bits(_py, append_name_bits);

            assert_eq!(molt_sequence_length(list_bits), 3);
            let idx_bits = MoltObject::from_int(1).bits();
            let got_bits = molt_sequence_getitem(list_bits, idx_bits);
            assert_eq!(to_i64(obj_from_bits(got_bits)), Some(4));
            let rc = molt_sequence_setitem(
                list_bits,
                MoltObject::from_int(0).bits(),
                MoltObject::from_int(9).bits(),
            );
            assert_eq!(rc, 0);
            let got0 = molt_sequence_getitem(list_bits, MoltObject::from_int(0).bits());
            assert_eq!(to_i64(obj_from_bits(got0)), Some(9));
            let got2 = molt_sequence_getitem(list_bits, MoltObject::from_int(2).bits());
            assert_eq!(to_i64(obj_from_bits(got2)), Some(5));
            dec_ref_bits(_py, got_bits);
            dec_ref_bits(_py, got0);
            dec_ref_bits(_py, got2);
            dec_ref_bits(_py, list_bits);
        });
    }

    #[test]
    fn buffer_acquire_and_release_pins_owner() {
        let _ = molt_runtime_init();
        let bytes_bits = unsafe { molt_bytes_from(b"abc".as_ptr(), 3) };
        assert!(!obj_from_bits(bytes_bits).is_none());
        let mut view = MoltBufferView {
            data: std::ptr::null_mut(),
            len: 0,
            readonly: 1,
            reserved: 0,
            stride: 1,
            itemsize: 1,
            owner: 0,
        };
        let rc = unsafe { molt_buffer_acquire(bytes_bits, &mut view as *mut MoltBufferView) };
        assert_eq!(rc, 0);
        assert_eq!(view.len, 3);
        assert_eq!(view.readonly, 1);
        assert!(!view.data.is_null());
        assert_eq!(view.owner, bytes_bits);
        let observed =
            unsafe { std::slice::from_raw_parts(view.data as *const u8, view.len as usize) };
        assert_eq!(observed, b"abc");
        let rc_release = unsafe { molt_buffer_release(&mut view as *mut MoltBufferView) };
        assert_eq!(rc_release, 0);
        assert!(view.data.is_null());
        assert_eq!(view.owner, 0);
        crate::with_gil_entry!(_py, {
            dec_ref_bits(_py, bytes_bits);
        });
    }

    #[test]
    fn err_pending_peek_restore_roundtrip() {
        let _ = molt_runtime_init();
        let runtime_error = crate::with_gil_entry!(_py, { runtime_error_type_bits(_py) });
        let msg = b"boom";
        let rc = unsafe { molt_err_set(runtime_error, msg.as_ptr(), msg.len() as u64) };
        assert_eq!(rc, 0);
        assert_eq!(molt_err_pending(), 1);
        let peek_bits = molt_err_peek();
        assert!(!obj_from_bits(peek_bits).is_none());
        assert_eq!(molt_err_pending(), 1);
        let fetched_bits = molt_err_fetch();
        assert!(!obj_from_bits(fetched_bits).is_none());
        assert_eq!(molt_err_pending(), 0);
        assert_eq!(molt_err_restore(fetched_bits), 0);
        assert_eq!(molt_err_pending(), 1);
        let restored_bits = molt_err_fetch();
        assert!(!obj_from_bits(restored_bits).is_none());
        assert_eq!(molt_err_pending(), 0);
        crate::with_gil_entry!(_py, {
            dec_ref_bits(_py, peek_bits);
            dec_ref_bits(_py, fetched_bits);
            dec_ref_bits(_py, restored_bits);
        });
    }

    #[test]
    fn mapping_length_success_and_failure_paths() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let dict_ptr = alloc_dict_with_pairs(_py, &[]);
            assert!(!dict_ptr.is_null());
            let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
            let key_ptr = alloc_string(_py, b"k");
            assert!(!key_ptr.is_null());
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            let value_bits = MoltObject::from_int(7).bits();
            assert_eq!(molt_mapping_setitem(dict_bits, key_bits, value_bits), 0);
            assert_eq!(molt_mapping_length(dict_bits), 1);
            let invalid_bits = MoltObject::from_int(42).bits();
            assert_eq!(molt_mapping_length(invalid_bits), -1);
            assert!(exception_pending(_py));
            let _ = molt_exception_clear();
            dec_ref_bits(_py, key_bits);
            dec_ref_bits(_py, dict_bits);
        });
    }

    #[test]
    fn mapping_keys_success_and_failure_paths() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let dict_ptr = alloc_dict_with_pairs(_py, &[]);
            assert!(!dict_ptr.is_null());
            let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
            let key_ptr = alloc_string(_py, b"k");
            assert!(!key_ptr.is_null());
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            let value_bits = MoltObject::from_int(7).bits();
            assert_eq!(molt_mapping_setitem(dict_bits, key_bits, value_bits), 0);

            let keys_bits = molt_mapping_keys(dict_bits);
            assert!(!obj_from_bits(keys_bits).is_none());
            assert_eq!(molt_sequence_length(keys_bits), 1);
            assert_eq!(molt_object_contains(keys_bits, key_bits), 1);
            dec_ref_bits(_py, keys_bits);

            let invalid_bits = MoltObject::from_int(42).bits();
            assert!(obj_from_bits(molt_mapping_keys(invalid_bits)).is_none());
            assert!(exception_pending(_py));
            let _ = molt_exception_clear();

            dec_ref_bits(_py, key_bits);
            dec_ref_bits(_py, dict_bits);
        });
    }

    #[test]
    fn string_from_as_ptr_roundtrip_and_type_errors() {
        let _ = molt_runtime_init();
        let text = b"hello";
        let string_bits = unsafe { molt_string_from(text.as_ptr(), text.len() as u64) };
        assert!(!obj_from_bits(string_bits).is_none());
        let mut out_len = 0u64;
        let ptr = unsafe { molt_string_as_ptr(string_bits, &mut out_len as *mut u64) };
        assert!(!ptr.is_null());
        assert_eq!(out_len, text.len() as u64);
        let observed = unsafe { std::slice::from_raw_parts(ptr, out_len as usize) };
        assert_eq!(observed, text);

        let invalid_bits = MoltObject::from_int(9).bits();
        let bad_ptr = unsafe { molt_string_as_ptr(invalid_bits, std::ptr::null_mut()) };
        assert!(bad_ptr.is_null());
        assert_eq!(molt_err_pending(), 1);
        assert_eq!(molt_err_clear(), 0);

        let null_bits = unsafe { molt_string_from(std::ptr::null(), 1) };
        assert_eq!(molt_err_pending(), 1);
        assert_eq!(molt_err_clear(), 0);

        crate::with_gil_entry!(_py, {
            dec_ref_bits(_py, string_bits);
            if !obj_from_bits(null_bits).is_none() {
                dec_ref_bits(_py, null_bits);
            }
        });
    }

    #[test]
    fn object_setattr_symbol_roundtrip() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let runtime_error = runtime_error_type_bits(_py);
            let msg_ptr = alloc_string(_py, b"msg");
            assert!(!msg_ptr.is_null());
            let msg_bits = MoltObject::from_ptr(msg_ptr).bits();
            let exc_bits = molt_exception_new_from_class(runtime_error, msg_bits);
            assert!(!obj_from_bits(exc_bits).is_none());
            let attr_ptr = alloc_string(_py, b"custom");
            assert!(!attr_ptr.is_null());
            let attr_bits = MoltObject::from_ptr(attr_ptr).bits();
            let value_bits = MoltObject::from_int(99).bits();
            let set_result = molt_object_setattr(exc_bits, attr_bits, value_bits);
            assert!(!exception_pending(_py));
            let got_bits = molt_object_getattr(exc_bits, attr_bits);
            assert_eq!(to_i64(obj_from_bits(got_bits)), Some(99));
            dec_ref_bits(_py, got_bits);
            if !obj_from_bits(set_result).is_none() {
                dec_ref_bits(_py, set_result);
            }
            dec_ref_bits(_py, attr_bits);
            dec_ref_bits(_py, exc_bits);
            dec_ref_bits(_py, msg_bits);
        });
    }

    #[test]
    fn scalar_handle_helpers_roundtrip() {
        let _ = molt_runtime_init();
        assert!(obj_from_bits(molt_none()).is_none());

        let true_bits = molt_bool_from_i32(1);
        let false_bits = molt_bool_from_i32(0);
        assert_eq!(molt_object_truthy(true_bits), 1);
        assert_eq!(molt_object_truthy(false_bits), 0);

        let int_bits = molt_int_from_i64(-42);
        assert_eq!(molt_int_as_i64(int_bits), -42);

        let float_bits = molt_float_from_f64(3.5);
        assert_eq!(molt_float_as_f64(float_bits), 3.5);
        assert_eq!(molt_float_as_f64(int_bits), -42.0);

        assert_eq!(molt_int_as_i64(float_bits), -1);
        assert_eq!(molt_err_pending(), 1);
        assert_eq!(molt_err_clear(), 0);

        crate::with_gil_entry!(_py, {
            dec_ref_bits(_py, true_bits);
            dec_ref_bits(_py, false_bits);
            dec_ref_bits(_py, int_bits);
            dec_ref_bits(_py, float_bits);
        });
    }

    #[test]
    fn object_bytes_compare_and_contains_helpers() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let runtime_error = runtime_error_type_bits(_py);
            let msg_ptr = alloc_string(_py, b"msg");
            assert!(!msg_ptr.is_null());
            let msg_bits = MoltObject::from_ptr(msg_ptr).bits();
            let exc_bits = molt_exception_new_from_class(runtime_error, msg_bits);
            assert!(!obj_from_bits(exc_bits).is_none());

            let value_bits = MoltObject::from_int(77).bits();
            let set_rc = unsafe {
                molt_object_setattr_bytes(
                    exc_bits,
                    b"custom".as_ptr(),
                    b"custom".len() as u64,
                    value_bits,
                )
            };
            assert_eq!(set_rc, 0);
            let got_bits = unsafe {
                molt_object_getattr_bytes(exc_bits, b"custom".as_ptr(), b"custom".len() as u64)
            };
            assert_eq!(to_i64(obj_from_bits(got_bits)), Some(77));
            dec_ref_bits(_py, got_bits);

            assert_eq!(
                molt_object_equal(
                    MoltObject::from_int(5).bits(),
                    MoltObject::from_int(5).bits()
                ),
                1
            );
            assert_eq!(
                molt_object_not_equal(
                    MoltObject::from_int(5).bits(),
                    MoltObject::from_int(6).bits()
                ),
                1
            );

            let list_ptr = alloc_list(
                _py,
                &[
                    MoltObject::from_int(1).bits(),
                    MoltObject::from_int(2).bits(),
                    MoltObject::from_int(3).bits(),
                ],
            );
            assert!(!list_ptr.is_null());
            let list_bits = MoltObject::from_ptr(list_ptr).bits();
            assert_eq!(
                molt_object_contains(list_bits, MoltObject::from_int(2).bits()),
                1
            );
            assert_eq!(
                molt_object_contains(list_bits, MoltObject::from_int(9).bits()),
                0
            );

            dec_ref_bits(_py, list_bits);
            dec_ref_bits(_py, exc_bits);
            dec_ref_bits(_py, msg_bits);
        });
    }

    #[test]
    fn array_constructors_roundtrip() {
        let _ = molt_runtime_init();
        let elems = [
            MoltObject::from_int(10).bits(),
            MoltObject::from_int(20).bits(),
            MoltObject::from_int(30).bits(),
        ];
        let tuple_bits = unsafe { molt_tuple_from_array(elems.as_ptr(), elems.len() as u64) };
        let list_bits = unsafe { molt_list_from_array(elems.as_ptr(), elems.len() as u64) };
        assert!(!obj_from_bits(tuple_bits).is_none());
        assert!(!obj_from_bits(list_bits).is_none());
        assert_eq!(molt_sequence_length(tuple_bits), 3);
        assert_eq!(molt_sequence_length(list_bits), 3);

        let keys = [
            MoltObject::from_int(1).bits(),
            MoltObject::from_int(2).bits(),
        ];
        let values = [
            MoltObject::from_int(100).bits(),
            MoltObject::from_int(200).bits(),
        ];
        let dict_bits = unsafe { molt_dict_from_pairs(keys.as_ptr(), values.as_ptr(), 2) };
        assert!(!obj_from_bits(dict_bits).is_none());
        assert_eq!(molt_mapping_length(dict_bits), 2);
        let got_bits = molt_mapping_getitem(dict_bits, keys[1]);
        assert_eq!(to_i64(obj_from_bits(got_bits)), Some(200));
        crate::with_gil_entry!(_py, {
            dec_ref_bits(_py, got_bits);
            dec_ref_bits(_py, tuple_bits);
            dec_ref_bits(_py, list_bits);
            dec_ref_bits(_py, dict_bits);
        });

        let null_tuple_bits = unsafe { molt_tuple_from_array(std::ptr::null::<MoltHandle>(), 1) };
        assert!(obj_from_bits(null_tuple_bits).is_none());
        assert_eq!(molt_err_pending(), 1);
        assert_eq!(molt_err_clear(), 0);
    }

    #[test]
    fn type_ready_and_module_parity_wrappers_roundtrip() {
        let _ = molt_runtime_init();
        crate::with_gil_entry!(_py, {
            let builtins = crate::builtins::classes::builtin_classes(_py);
            assert_eq!(molt_type_ready(builtins.type_obj), 0);
            assert_eq!(molt_type_ready(MoltObject::from_int(1).bits()), -1);
            assert_eq!(molt_err_pending(), 1);
            assert_eq!(molt_err_clear(), 0);

            let module_name_bits = unsafe { molt_string_from(b"demo_ext".as_ptr(), 8) };
            assert!(!obj_from_bits(module_name_bits).is_none());
            let module_bits = molt_module_create(module_name_bits);
            assert!(!obj_from_bits(module_bits).is_none());

            let answer_name_ptr = alloc_string(_py, b"answer");
            assert!(!answer_name_ptr.is_null());
            let answer_name_bits = MoltObject::from_ptr(answer_name_ptr).bits();
            assert_eq!(
                molt_module_add_int_constant(module_bits, answer_name_bits, 42),
                0
            );
            let answer_bits = molt_module_get_object(module_bits, answer_name_bits);
            assert_eq!(to_i64(obj_from_bits(answer_bits)), Some(42));

            assert_eq!(
                unsafe {
                    molt_module_add_object_bytes(
                        module_bits,
                        b"status".as_ptr(),
                        b"status".len() as u64,
                        MoltObject::from_int(7).bits(),
                    )
                },
                0
            );
            let status_bits = unsafe {
                molt_module_get_object_bytes(
                    module_bits,
                    b"status".as_ptr(),
                    b"status".len() as u64,
                )
            };
            assert_eq!(to_i64(obj_from_bits(status_bits)), Some(7));

            let label_name_ptr = alloc_string(_py, b"label");
            assert!(!label_name_ptr.is_null());
            let label_name_bits = MoltObject::from_ptr(label_name_ptr).bits();
            assert_eq!(
                unsafe {
                    molt_module_add_string_constant(module_bits, label_name_bits, b"ok".as_ptr(), 2)
                },
                0
            );
            let label_bits = molt_module_get_object(module_bits, label_name_bits);
            let mut label_len = 0u64;
            let label_ptr = unsafe { molt_string_as_ptr(label_bits, &mut label_len as *mut u64) };
            assert!(!label_ptr.is_null());
            assert_eq!(label_len, 2);
            let label_text = unsafe { std::slice::from_raw_parts(label_ptr, label_len as usize) };
            assert_eq!(label_text, b"ok");

            let dict_bits = molt_module_get_dict(module_bits);
            assert!(!obj_from_bits(dict_bits).is_none());
            assert!(molt_mapping_length(dict_bits) >= 3);

            dec_ref_bits(_py, dict_bits);
            dec_ref_bits(_py, label_bits);
            dec_ref_bits(_py, label_name_bits);
            dec_ref_bits(_py, status_bits);
            dec_ref_bits(_py, answer_bits);
            dec_ref_bits(_py, answer_name_bits);
            dec_ref_bits(_py, module_bits);
            dec_ref_bits(_py, module_name_bits);
        });
    }
}
