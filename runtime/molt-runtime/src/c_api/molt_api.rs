//! The `molt_*` public C API — lifecycle, GIL, handles, objects, modules, errors.

use super::*;

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
    if crate::state::runtime_state::runtime_state_for_gil().is_some() {
        crate::with_gil_entry!(_py, {
            c_api_module_teardown(_py);
        });
    } else {
        let metadata = c_api_module_metadata_registry();
        metadata
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        let state = c_api_module_state_registry();
        let mut guard = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.by_def.clear();
        guard.by_module.clear();
    }
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
            return raise_i32(
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

/// Returns a **borrowed** handle for an attribute on `obj_bits`.
///
/// Identical to `molt_object_getattr_bytes` except the returned handle does
/// NOT carry an extra refcount.  The handle is valid as long as the parent
/// object (module, type, etc.) continues to hold the attribute.
///
/// This is the runtime counterpart of CPython's internal borrowed-reference
/// getattr used by `PyImport_GetModuleDict`, `PyEval_GetBuiltins`, etc.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_object_getattr_borrowed(
    obj_bits: MoltHandle,
    name_ptr: *const u8,
    name_len: u64,
) -> MoltHandle {
    crate::with_gil_entry!(_py, {
        let result = unsafe { molt_object_getattr_bytes(obj_bits, name_ptr, name_len) };
        if result != 0 && !exception_pending(_py) {
            // Convert new reference → borrowed reference.
            // Safe because the parent object holds its own strong reference
            // to the attribute value (e.g. in its __dict__).
            dec_ref_bits(_py, result);
        }
        result
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
            return raise_i32(
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
            return raise_i32(_py, "RuntimeError", "failed to intern attribute name");
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
        let module_ptr = match require_module_handle(_py, module_bits) {
            Ok(ptr) => ptr,
            Err(_) => return none_bits(),
        };
        unsafe {
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
pub extern "C" fn molt_module_capi_register(
    module_bits: MoltHandle,
    module_def_ptr: usize,
    module_state_size: u64,
) -> i32 {
    crate::with_gil_entry!(_py, {
        let module_ptr = match require_module_handle(_py, module_bits) {
            Ok(ptr) => ptr,
            Err(code) => return code,
        };
        let module_key = module_ptr_key(module_ptr);
        let size = match usize::try_from(module_state_size) {
            Ok(value) => value,
            Err(_) => {
                return raise_i32(
                    _py,
                    "OverflowError",
                    "module state size does not fit in usize",
                );
            }
        };
        let state = if size == 0 {
            None
        } else {
            match alloc_zeroed_state(_py, size) {
                Ok(value) => Some(value),
                Err(code) => return code,
            }
        };
        let metadata = CApiModuleMetadata {
            module_def_ptr,
            module_state: state,
        };
        let registry = c_api_module_metadata_registry();
        let mut guard = registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.insert(module_key, metadata);
        0
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_capi_get_def(module_bits: MoltHandle) -> usize {
    crate::with_gil_entry!(_py, {
        let module_ptr = match require_module_handle(_py, module_bits) {
            Ok(ptr) => ptr,
            Err(_) => return 0,
        };
        let module_key = module_ptr_key(module_ptr);
        let registry = c_api_module_metadata_registry();
        let guard = registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard
            .get(&module_key)
            .map_or(0, |entry| entry.module_def_ptr)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_capi_get_state(module_bits: MoltHandle) -> usize {
    crate::with_gil_entry!(_py, {
        let module_ptr = match require_module_handle(_py, module_bits) {
            Ok(ptr) => ptr,
            Err(_) => return 0,
        };
        let module_key = module_ptr_key(module_ptr);
        let registry = c_api_module_metadata_registry();
        let guard = registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.get(&module_key).map_or(0, |entry| {
            entry
                .module_state
                .as_ref()
                .map_or(0, |state| state.as_ptr() as usize)
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_state_add(module_bits: MoltHandle, module_def_ptr: usize) -> i32 {
    crate::with_gil_entry!(_py, {
        if module_def_ptr == 0 {
            return raise_i32(
                _py,
                "TypeError",
                "module definition pointer must not be NULL",
            );
        }
        let module_ptr = match require_module_handle(_py, module_bits) {
            Ok(ptr) => ptr,
            Err(code) => return code,
        };
        let module_key = module_ptr_key(module_ptr);
        let def_key = module_def_ptr;
        let mut decref_bits: Vec<MoltHandle> = Vec::new();
        {
            let registry = c_api_module_state_registry();
            let mut guard = registry
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());

            if let Some(existing) = guard.by_def.get(&def_key).copied()
                && existing == module_bits
                && guard.by_module.get(&module_key).copied() == Some(def_key)
            {
                return 0;
            }

            if let Some(old_def) = guard.by_module.get(&module_key).copied()
                && old_def != def_key
                && let Some(old_bits) = guard.by_def.remove(&old_def)
            {
                decref_bits.push(old_bits);
            }

            if let Some(old_bits) = guard.by_def.insert(def_key, module_bits)
                && old_bits != module_bits
            {
                if let Some(old_ptr) = obj_from_bits(old_bits).as_ptr() {
                    guard.by_module.remove(&module_ptr_key(old_ptr));
                }
                decref_bits.push(old_bits);
            }

            guard.by_module.insert(module_key, def_key);
            inc_ref_bits(_py, module_bits);
        }
        for bits in decref_bits {
            if !obj_from_bits(bits).is_none() {
                dec_ref_bits(_py, bits);
            }
        }
        0
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_state_find(module_def_ptr: usize) -> MoltHandle {
    crate::with_gil_entry!(_py, {
        if module_def_ptr == 0 {
            let _ = raise_exception::<u64>(
                _py,
                "TypeError",
                "module definition pointer must not be NULL",
            );
            return 0;
        }
        let registry = c_api_module_state_registry();
        let guard = registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.by_def.get(&module_def_ptr).copied().unwrap_or(0)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_module_state_remove(module_def_ptr: usize) -> i32 {
    crate::with_gil_entry!(_py, {
        if module_def_ptr == 0 {
            return raise_i32(
                _py,
                "TypeError",
                "module definition pointer must not be NULL",
            );
        }
        let Some(bits) = c_api_module_state_registry_remove_def(_py, module_def_ptr) else {
            return raise_i32(_py, "RuntimeError", "module definition was not registered");
        };
        if !obj_from_bits(bits).is_none() {
            dec_ref_bits(_py, bits);
        }
        0
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
            return raise_i32(
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
            return raise_i32(
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
pub extern "C" fn molt_module_add_type(module_bits: MoltHandle, type_bits: MoltHandle) -> i32 {
    crate::with_gil_entry!(_py, {
        if require_type_handle(_py, type_bits).is_err() {
            return -1;
        }
        let type_name_bits = unsafe {
            molt_object_getattr_bytes(type_bits, b"__name__".as_ptr(), b"__name__".len() as u64)
        };
        if exception_pending(_py) {
            if !obj_from_bits(type_name_bits).is_none() {
                dec_ref_bits(_py, type_name_bits);
            }
            return -1;
        }
        let rc = module_add_object_impl(_py, module_bits, type_name_bits, type_bits);
        if !obj_from_bits(type_name_bits).is_none() {
            dec_ref_bits(_py, type_name_bits);
        }
        rc
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
            return raise_i32(
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
pub extern "C" fn molt_capi_method_dispatch(
    closure_bits: MoltHandle,
    args_tuple_bits: MoltHandle,
    kwargs_bits: MoltHandle,
) -> MoltHandle {
    crate::with_gil_entry!(_py, {
        let (self_bits, flags, fn_ptr) = match c_api_method_decode_closure(_py, closure_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let args_ptr = match c_api_method_require_tuple(_py, args_tuple_bits) {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        let args_vec = unsafe { seq_vec_ref(args_ptr) };
        let dynamic_self = obj_from_bits(self_bits).is_none();
        let callback_self_bits = if dynamic_self {
            if args_vec.is_empty() {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "C-API method requires a bound self/cls argument",
                );
            }
            args_vec[0]
        } else {
            self_bits
        };
        let kwargs_present = match c_api_method_kwargs_present(_py, kwargs_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let kwargs_for_callback = if kwargs_bits == 0 || obj_from_bits(kwargs_bits).is_none() {
            0
        } else {
            kwargs_bits
        };
        let mut callback_args_owner_bits = 0u64;
        let result = unsafe {
            match flags {
                C_API_METH_VARARGS => {
                    if kwargs_present {
                        return raise_exception::<u64>(
                            _py,
                            "TypeError",
                            "C-API method does not accept keyword arguments",
                        );
                    }
                    let callback_args_bits = if dynamic_self {
                        let tail_ptr = alloc_tuple(_py, &args_vec[1..]);
                        if tail_ptr.is_null() {
                            return none_bits();
                        }
                        callback_args_owner_bits = MoltObject::from_ptr(tail_ptr).bits();
                        callback_args_owner_bits
                    } else {
                        args_tuple_bits
                    };
                    let func: extern "C" fn(u64, u64) -> u64 = std::mem::transmute(fn_ptr);
                    func(callback_self_bits, callback_args_bits)
                }
                C_API_METH_VARARGS_KEYWORDS => {
                    let callback_args_bits = if dynamic_self {
                        let tail_ptr = alloc_tuple(_py, &args_vec[1..]);
                        if tail_ptr.is_null() {
                            return none_bits();
                        }
                        callback_args_owner_bits = MoltObject::from_ptr(tail_ptr).bits();
                        callback_args_owner_bits
                    } else {
                        args_tuple_bits
                    };
                    let func: extern "C" fn(u64, u64, u64) -> u64 = std::mem::transmute(fn_ptr);
                    func(callback_self_bits, callback_args_bits, kwargs_for_callback)
                }
                C_API_METH_NOARGS => {
                    if kwargs_present {
                        return raise_exception::<u64>(
                            _py,
                            "TypeError",
                            "C-API noargs method does not accept keyword arguments",
                        );
                    }
                    let expected = if dynamic_self { 1 } else { 0 };
                    if c_api_method_tuple_len(_py, args_ptr) != expected {
                        return raise_exception::<u64>(
                            _py,
                            "TypeError",
                            "C-API noargs method expects no positional arguments",
                        );
                    }
                    let func: extern "C" fn(u64, u64) -> u64 = std::mem::transmute(fn_ptr);
                    func(callback_self_bits, 0)
                }
                C_API_METH_O => {
                    if kwargs_present {
                        return raise_exception::<u64>(
                            _py,
                            "TypeError",
                            "C-API METH_O method does not accept keyword arguments",
                        );
                    }
                    let expected = if dynamic_self { 2 } else { 1 };
                    if c_api_method_tuple_len(_py, args_ptr) != expected {
                        return raise_exception::<u64>(
                            _py,
                            "TypeError",
                            "C-API METH_O method expects exactly one argument",
                        );
                    }
                    let arg0_bits = if dynamic_self {
                        args_vec[1]
                    } else {
                        args_vec[0]
                    };
                    let func: extern "C" fn(u64, u64) -> u64 = std::mem::transmute(fn_ptr);
                    func(callback_self_bits, arg0_bits)
                }
                _ => {
                    return raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "unsupported C-API method flags",
                    );
                }
            }
        };
        if callback_args_owner_bits != 0 && result == callback_args_owner_bits {
            inc_ref_bits(_py, result);
        }
        if callback_args_owner_bits != 0 {
            dec_ref_bits(_py, callback_args_owner_bits);
        }
        if result == 0 {
            if exception_pending(_py) {
                return none_bits();
            }
            return raise_exception::<u64>(
                _py,
                "RuntimeError",
                "C-API method returned NULL without setting an exception",
            );
        }
        result
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_cfunction_create_bytes(
    self_bits: MoltHandle,
    name_ptr: *const u8,
    name_len: u64,
    method_ptr: usize,
    method_flags: u32,
    doc_ptr: *const u8,
    doc_len: u64,
) -> MoltHandle {
    crate::with_gil_entry!(_py, {
        let Some(name_bytes) = (unsafe { bytes_slice_from_raw(name_ptr, name_len) }) else {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "method name pointer cannot be null when len > 0",
            );
        };
        let doc_bytes = if doc_ptr.is_null() && doc_len == 0 {
            None
        } else {
            let Some(bytes) = (unsafe { bytes_slice_from_raw(doc_ptr, doc_len) }) else {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "method doc pointer cannot be null when len > 0",
                );
            };
            Some(bytes)
        };
        match c_api_method_build_function(
            _py,
            self_bits,
            name_bytes,
            method_ptr,
            method_flags,
            doc_bytes,
        ) {
            Ok(bits) => bits,
            Err(_) => none_bits(),
        }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_module_add_cfunction_bytes(
    module_bits: MoltHandle,
    name_ptr: *const u8,
    name_len: u64,
    method_ptr: usize,
    method_flags: u32,
    doc_ptr: *const u8,
    doc_len: u64,
) -> i32 {
    crate::with_gil_entry!(_py, {
        let Some(name_bytes) = (unsafe { bytes_slice_from_raw(name_ptr, name_len) }) else {
            return raise_i32(
                _py,
                "TypeError",
                "method name pointer cannot be null when len > 0",
            );
        };
        if require_module_handle(_py, module_bits).is_err() {
            return -1;
        }
        let doc_bytes = if doc_ptr.is_null() && doc_len == 0 {
            None
        } else {
            let Some(bytes) = (unsafe { bytes_slice_from_raw(doc_ptr, doc_len) }) else {
                return raise_i32(
                    _py,
                    "TypeError",
                    "method doc pointer cannot be null when len > 0",
                );
            };
            Some(bytes)
        };
        let func_bits = match c_api_method_build_function(
            _py,
            module_bits,
            name_bytes,
            method_ptr,
            method_flags,
            doc_bytes,
        ) {
            Ok(bits) => bits,
            Err(code) => return code,
        };
        let name_ptr_obj = alloc_string(_py, name_bytes);
        if name_ptr_obj.is_null() {
            dec_ref_bits(_py, func_bits);
            return -1;
        }
        let name_bits = MoltObject::from_ptr(name_ptr_obj).bits();
        let module_name_bits = unsafe {
            molt_object_getattr_bytes(module_bits, b"__name__".as_ptr(), b"__name__".len() as u64)
        };
        if !obj_from_bits(module_name_bits).is_none() && !exception_pending(_py) {
            let _ = c_api_method_set_attr_bytes(_py, func_bits, b"__module__", module_name_bits);
            dec_ref_bits(_py, module_name_bits);
            if exception_pending(_py) {
                dec_ref_bits(_py, func_bits);
                dec_ref_bits(_py, name_bits);
                return -1;
            }
        } else if exception_pending(_py) {
            let _ = molt_exception_clear();
        }
        let rc = module_add_object_impl(_py, module_bits, name_bits, func_bits);
        dec_ref_bits(_py, func_bits);
        dec_ref_bits(_py, name_bits);
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
            return raise_i32(_py, "TypeError", "out_view cannot be null");
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

// ---------------------------------------------------------------------------
// GIL release/re-acquire — resolved by molt-runtime-core's FFI declarations
// ---------------------------------------------------------------------------

/// Release the GIL and return an opaque token encoding the saved state.
///
/// The token packs `depth` (shifted left 1) and `had_runtime_guard` (bit 0)
/// into a single `u64` so that `molt_gil_reacquire_guard` can restore it.
#[unsafe(no_mangle)]
pub extern "C" fn molt_gil_release_guard() -> u64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let guard = crate::concurrency::GilReleaseGuard::new();
        let depth = guard.depth;
        let had_runtime = guard.had_runtime_guard;
        std::mem::forget(guard); // Don't drop — we'll reacquire manually
        ((depth as u64) << 1) | (had_runtime as u64)
    }
    #[cfg(target_arch = "wasm32")]
    {
        // Single-threaded: no GIL state to save.
        0
    }
}

/// Re-acquire the GIL using the token returned by `molt_gil_release_guard`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_gil_reacquire_guard(token: u64) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let depth = (token >> 1) as usize;
        let had_runtime = (token & 1) != 0;
        // Reconstruct the guard and let it drop to re-acquire the GIL.
        let guard = crate::concurrency::GilReleaseGuard {
            depth,
            had_runtime_guard: had_runtime,
        };
        drop(guard);
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = token; // Single-threaded: nothing to reacquire.
    }
}
