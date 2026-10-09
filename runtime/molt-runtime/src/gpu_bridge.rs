//! Object-runtime boundary for the implementation owned by `molt-gpu`.
//!
//! Keep this file limited to ABI translation.  GPU algorithms, backend policy,
//! tensor semantics, and data transforms belong in the satellite crate.

/// Translate the Python launch tuple into the executor's owned call builder.
/// The executor borrows this builder; each per-thread clone is consumed by bind.
#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_kernel_launch_python(
    callable_bits: u64,
    grid_bits: u64,
    threads_bits: u64,
    args_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if crate::exception_pending(_py) {
            return crate::MoltObject::none().bits();
        }
        let Some(args_ptr) = crate::obj_from_bits(args_bits).as_ptr() else {
            return crate::raise_exception::<u64>(
                _py,
                "TypeError",
                "GPU launch args must be a tuple",
            );
        };
        if unsafe { crate::object_type_id(args_ptr) } != crate::TYPE_ID_TUPLE {
            return crate::raise_exception::<u64>(
                _py,
                "TypeError",
                "GPU launch args must be a tuple",
            );
        }
        let zero = crate::MoltObject::from_int(0).bits();
        let builder = crate::call::bind::molt_callargs_new_expanded(zero, zero);
        if crate::exception_pending(_py) {
            return crate::MoltObject::none().bits();
        }
        let Some(builder_ptr) = crate::obj_from_bits(builder).as_ptr() else {
            return crate::raise_exception::<u64>(
                _py,
                "MemoryError",
                "GPU launch builder allocation failed",
            );
        };
        let _builder_owner = crate::PtrDropGuard::preserving(builder_ptr);
        let expanded = unsafe { crate::molt_callargs_expand_star(builder, args_bits) };
        if crate::exception_pending(_py) {
            return expanded;
        }
        crate::dec_ref_bits(_py, expanded);
        molt_gpu_runtime::molt_gpu_kernel_launch(callable_bits, grid_bits, threads_bits, builder)
    })
}

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

/// Classify protocol-free scalar carriers using the canonical semantic class.
/// Storage TYPE_ID alone is insufficient: int/float subclasses use those too.
/// -1 refuses subclass protocols; 0 leaves nonnumeric Buffer admission to its owner.
#[unsafe(no_mangle)]
pub extern "C" fn __molt_gpu_exact_scalar_kind(bits: u64) -> i32 {
    crate::with_gil_entry_nopanic!(py, {
        let obj = crate::obj_from_bits(bits);
        if obj.is_bool() {
            return 3;
        }
        if crate::builtins::numbers::is_exact_float(py, bits) {
            return 2;
        }
        let class = crate::type_of_bits(py, bits);
        if class == crate::builtin_classes(py).int {
            return 1;
        }
        if crate::builtins::numbers::int_subclass_value_bits_raw(bits).is_some()
            || crate::object::class_layout::scalar_value_bits(
                obj,
                crate::object::class_layout::ScalarValueKind::Float,
            )
            .is_some()
        {
            return -1;
        }
        if let Some(ptr) = obj.as_ptr() {
            if matches!(
                unsafe { crate::object_type_id(ptr) },
                crate::TYPE_ID_BIGINT | crate::TYPE_ID_FLOAT
            ) {
                return -1;
            }
        }
        0
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
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if obj_ptr.is_null() || (name_ptr.is_null() && name_len != 0) {
            return crate::raise_exception::<u64>(
                _py,
                "TypeError",
                "invalid attribute receiver or name",
            );
        }
        let bytes = if name_len == 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(name_ptr, name_len) }
        };
        let Ok(name) = std::str::from_utf8(bytes) else {
            return crate::raise_exception::<u64>(_py, "TypeError", "attribute name must be UTF-8");
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

/// Optional attribute lookup must inspect the raised slot, not the active
/// handled exception exposed by Python's exception-last API.
#[unsafe(no_mangle)]
pub extern "C" fn __molt_gpu_clear_attribute_error_if_pending() -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        i32::from(crate::builtins::attr::clear_attribute_error_if_pending(_py))
    })
}

/// Compiler-only publication into the immutable code owner. This direct ABI
/// is deliberately absent from the Python intrinsic manifest/resolver. Generic
/// Python attributes cannot publish certificates. As with compiler-generated
/// code itself, arbitrary native/foreign ABI callers are trusted, not a sandbox.
/// Generic function attributes are never consulted.
#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_kernel_descriptor_set(callable: u64, descriptor: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(function) = crate::obj_from_bits(callable).as_ptr() else {
            return crate::raise_exception::<u64>(
                py,
                "TypeError",
                "GPU descriptor requires a function",
            );
        };
        unsafe {
            if crate::object_type_id(function) != crate::TYPE_ID_FUNCTION {
                return crate::raise_exception::<u64>(
                    py,
                    "TypeError",
                    "GPU descriptor requires a function",
                );
            }
            let Some(code) = crate::obj_from_bits(crate::function_code_bits(function)).as_ptr()
            else {
                return crate::raise_exception::<u64>(
                    py,
                    "RuntimeError",
                    "GPU function has no published code",
                );
            };
            if crate::object_type_id(code) != crate::TYPE_ID_CODE {
                return crate::raise_exception::<u64>(
                    py,
                    "RuntimeError",
                    "GPU function has invalid code",
                );
            }
            crate::object::layout::code_publish_gpu_descriptor(py, code, descriptor);
        }
        crate::MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_gpu_kernel_descriptor(callable: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(function) = crate::obj_from_bits(callable).as_ptr() else {
            return crate::MoltObject::none().bits();
        };
        unsafe {
            if crate::object_type_id(function) != crate::TYPE_ID_FUNCTION {
                return crate::MoltObject::none().bits();
            }
            let Some(code) = crate::obj_from_bits(crate::function_code_bits(function)).as_ptr()
            else {
                return crate::MoltObject::none().bits();
            };
            if crate::object_type_id(code) != crate::TYPE_ID_CODE {
                return crate::MoltObject::none().bits();
            }
            let descriptor = crate::object::layout::code_gpu_descriptor_bits(code);
            if descriptor == 0 {
                return crate::MoltObject::none().bits();
            }
            crate::inc_ref_bits(py, descriptor);
            descriptor
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_gpu_descriptor_is_current(
    callable: u64,
    descriptor: u64,
    slot: u64,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(function) = crate::obj_from_bits(callable).as_ptr() else {
            return 0;
        };
        unsafe {
            if crate::object_type_id(function) != crate::TYPE_ID_FUNCTION
                || crate::function_mutation_version(function) != 0
            {
                return 0;
            }
            let Some(code) = crate::obj_from_bits(crate::function_code_bits(function)).as_ptr()
            else {
                return 0;
            };
            i32::from(
                crate::object_type_id(code) == crate::TYPE_ID_CODE
                    && crate::object::layout::code_gpu_descriptor_bits(code) == descriptor
                    && crate::object::layout::code_frame_slot_id(code) == Some(slot),
            )
        }
    })
}

/// Read the exact namespace entry without descriptors, __getattr__, equality
/// callbacks or the launcher's active frame globals. The returned value owns a
/// reference; missing/ambiguous/custom namespace cells fail admission.
fn gpu_exact_namespace_lookup(
    py: &crate::PyToken<'_>,
    namespace: u64,
    name: &[u8],
) -> Result<Option<u64>, ()> {
    use crate::object::ops::{ExactStringLookup, dict_exact_string_lookup, hash_string_bytes};
    let dict = crate::obj_from_bits(namespace).as_ptr().ok_or(())?;
    unsafe {
        if !crate::object::object_is_exact_builtin_dict(py, dict) {
            return Err(());
        }
        match dict_exact_string_lookup(py, dict, name, hash_string_bytes(py, name) as u64) {
            ExactStringLookup::Found(index) => Ok(Some(crate::dict_entries(dict)[index].value)),
            ExactStringLookup::Absent => Ok(None),
            ExactStringLookup::Undecided => Err(()),
        }
    }
}

fn gpu_namespace_value(py: &crate::PyToken<'_>, namespace: u64, name: &[u8]) -> u64 {
    match gpu_exact_namespace_lookup(py, namespace, name) {
        Ok(Some(bits)) => {
            crate::inc_ref_bits(py, bits);
            bits
        }
        _ => crate::raise_exception::<u64>(
            py,
            "RuntimeError",
            "GPU binding needs an exact callback-free namespace entry",
        ),
    }
}

/// Same pinned MRO and exact-string dictionary authority as runtime lookup,
/// with ambiguous/custom dictionary equality refused rather than executed.
unsafe fn gpu_class_binding(
    py: &crate::PyToken<'_>,
    object: *mut u8,
    name: &[u8],
) -> Result<Option<u64>, ()> {
    unsafe {
        let class = crate::obj_from_bits(crate::object_class_bits(object))
            .as_ptr()
            .ok_or(())?;
        if crate::object_type_id(class) != crate::TYPE_ID_TYPE {
            return Err(());
        }
        let mro = crate::class_mro_pinned(py, class).ok_or(())?;
        for &bits in mro.iter() {
            let owner = crate::obj_from_bits(bits).as_ptr().ok_or(())?;
            if crate::object_type_id(owner) != crate::TYPE_ID_TYPE {
                return Err(());
            }
            if let Some(value) =
                gpu_exact_namespace_lookup(py, crate::class_dict_bits(owner), name)?
            {
                return Ok(Some(value));
            }
        }
        Ok(None)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_gpu_class_binding(object: u64, name: *const u8, len: usize) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let selected = if name.is_null() {
            None
        } else {
            crate::obj_from_bits(object)
                .as_ptr()
                .and_then(|object| unsafe {
                    gpu_class_binding(py, object, std::slice::from_raw_parts(name, len))
                        .ok()
                        .flatten()
                })
        };
        if let Some(bits) = selected {
            crate::inc_ref_bits(py, bits);
            bits
        } else {
            crate::raise_exception::<u64>(
                py,
                "RuntimeError",
                "GPU buffer method binding is unavailable",
            )
        }
    })
}

/// A final instance-field read after callback-capable argument extraction.
/// Only the existing inferred-field/dictionary owner with ordinary attribute
/// access is supported. No property, descriptor or guest equality runs here.
#[unsafe(no_mangle)]
pub extern "C" fn __molt_gpu_default_field(object: u64, name: *const u8, len: usize) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let refused = || {
            crate::raise_exception::<u64>(
                py,
                "RuntimeError",
                "GPU buffer storage needs default attribute access and an ordinary field",
            )
        };
        if name.is_null() {
            return refused();
        }
        let Some(object) = crate::obj_from_bits(object).as_ptr() else {
            return refused();
        };
        // Materialize the canonical method anchors before inspecting mutable
        // class/instance state. All subsequent successful reads are callback-free.
        let Some(get) = crate::builtins::methods::object_method_bits(py, "__getattribute__") else {
            return crate::MoltObject::none().bits();
        };
        let Some(set) = crate::builtins::methods::object_method_bits(py, "__setattr__") else {
            return crate::MoltObject::none().bits();
        };
        unsafe {
            let name = std::slice::from_raw_parts(name, len);
            if !crate::object::object_has_class_shape(object)
                || !gpu_class_binding(py, object, b"__getattribute__")
                    .is_ok_and(|value| value.is_none_or(|value| value == get))
                || !gpu_class_binding(py, object, b"__setattr__")
                    .is_ok_and(|value| value.is_none_or(|value| value == set))
                || gpu_class_binding(py, object, name) != Ok(None)
            {
                return refused();
            }
            let value = match crate::object::field_storage::current_dictionary(py, object) {
                Err(()) => return crate::MoltObject::none().bits(),
                Ok(Some(dict)) => gpu_exact_namespace_lookup(py, dict, name).ok().flatten(),
                Ok(None) => {
                    let Some(class) =
                        crate::obj_from_bits(crate::object_class_bits(object)).as_ptr()
                    else {
                        return refused();
                    };
                    let mut selected = None;
                    crate::object::field_storage::for_each_instance_field(
                        py,
                        object,
                        class,
                        &mut |field, slot| {
                            let Some(key) = crate::obj_from_bits(field.name).as_ptr() else {
                                return;
                            };
                            if field.kind == crate::object::class_layout::ClassFieldKind::Inferred
                                && crate::object_type_id(key) == crate::TYPE_ID_STRING
                                && std::slice::from_raw_parts(
                                    crate::string_bytes(key),
                                    crate::string_len(key),
                                ) == name
                            {
                                selected = Some(*slot);
                            }
                        },
                    );
                    selected
                }
            };
            if let Some(bits) = value {
                crate::inc_ref_bits(py, bits);
                bits
            } else {
                refused()
            }
        }
    })
}

/// Resolve the selected Python body's own globals/builtins, never the active
/// launcher's frame. The default builtin namespace can be an exact module.
#[unsafe(no_mangle)]
pub extern "C" fn __molt_gpu_function_binding(function: u64, name: *const u8, len: usize) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let refused = || {
            crate::raise_exception::<u64>(
                py,
                "RuntimeError",
                "GPU Python body dependency requires exact captured namespaces",
            )
        };
        if name.is_null() {
            return refused();
        }
        let Some(function) = crate::obj_from_bits(function).as_ptr() else {
            return refused();
        };
        unsafe {
            if crate::object_type_id(function) != crate::TYPE_ID_FUNCTION {
                return refused();
            }
            let name = std::slice::from_raw_parts(name, len);
            match gpu_exact_namespace_lookup(py, crate::function_globals_bits(function), name) {
                Ok(Some(bits)) => {
                    crate::inc_ref_bits(py, bits);
                    return bits;
                }
                Err(()) => return refused(),
                Ok(None) => (),
            }
            let mut builtins = crate::object::layout::function_builtins_bits(function);
            if let Some(ptr) = crate::obj_from_bits(builtins).as_ptr() {
                if crate::object_type_id(ptr) == crate::TYPE_ID_MODULE
                    && crate::object_class_bits(ptr) == crate::builtin_classes(py).module
                {
                    builtins = crate::module_dict_bits(ptr);
                }
            }
            gpu_namespace_value(py, builtins, name)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_gpu_kernel_global(callable: u64, name: *const u8, len: usize) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if name.is_null() {
            return crate::MoltObject::none().bits();
        }
        let Some(function) = crate::obj_from_bits(callable).as_ptr() else {
            return crate::MoltObject::none().bits();
        };
        unsafe {
            if crate::object_type_id(function) != crate::TYPE_ID_FUNCTION {
                return crate::MoltObject::none().bits();
            }
            gpu_namespace_value(
                py,
                crate::function_globals_bits(function),
                std::slice::from_raw_parts(name, len),
            )
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_gpu_module_binding(module: u64, name: *const u8, len: usize) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(module) = crate::obj_from_bits(module).as_ptr() else {
            return crate::raise_exception::<u64>(
                py,
                "RuntimeError",
                "GPU attribute binding requires an exact module",
            );
        };
        unsafe {
            if name.is_null()
                || crate::object_type_id(module) != crate::TYPE_ID_MODULE
                || crate::object_class_bits(module) != crate::builtin_classes(py).module
            {
                return crate::raise_exception::<u64>(
                    py,
                    "RuntimeError",
                    "GPU attribute binding requires an exact module",
                );
            }
            gpu_namespace_value(
                py,
                crate::module_dict_bits(module),
                std::slice::from_raw_parts(name, len),
            )
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn __molt_gpu_intrinsic_matches(bits: u64, name: *const u8, len: usize) -> i32 {
    crate::with_gil_entry_nopanic!(py, {
        if name.is_null() {
            return 0;
        }
        let Ok(name) = std::str::from_utf8(unsafe { std::slice::from_raw_parts(name, len) }) else {
            return 0;
        };
        i32::from(crate::intrinsics::registry::is_named_runtime_materialization(py, bits, name))
    })
}

/// Return the canonical binder's owned ABI-slot tuple, without invoking a body.
#[unsafe(no_mangle)]
pub extern "C" fn __molt_gpu_bind_kernel_arguments(callable: u64, builder: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let arguments =
            match unsafe { crate::call::bind::callargs_positional_snapshot(py, builder) } {
                Ok(arguments) => arguments,
                Err(error) => return error,
            };
        unsafe { crate::call::bind::bind_python_frame_tuple(py, callable, &arguments) }
    })
}

/// Compare the selected Python callable with a compiler-admitted body carried
/// by the kernel's immutable code descriptor. This bridge never materializes a
/// function, executes an attribute lookup, or hashes source in the guest.
#[unsafe(no_mangle)]
pub extern "C" fn __molt_gpu_compiled_body_matches(
    bits: u64,
    symbol: *const u8,
    symbol_len: usize,
    arity: u64,
    slot: u64,
    defaults: *const u64,
    defaults_len: usize,
) -> i32 {
    crate::with_gil_entry_nopanic!(py, {
        if symbol.is_null() || (defaults.is_null() && defaults_len != 0) {
            return 0;
        }
        let Ok(symbol) =
            std::str::from_utf8(unsafe { std::slice::from_raw_parts(symbol, symbol_len) })
        else {
            return 0;
        };
        let defaults = if defaults_len == 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(defaults, defaults_len) }
        };
        i32::from(
            crate::intrinsics::registry::is_compiled_body_materialization(
                py, bits, symbol, arity, slot, defaults,
            ),
        )
    })
}

/// Compare public class/builtin dependencies with the existing builtin owners.
/// Names here select an owner to compare, never a mutable module spelling.
#[unsafe(no_mangle)]
pub extern "C" fn __molt_gpu_builtin_matches(bits: u64, name: *const u8, len: usize) -> i32 {
    crate::with_gil_entry_nopanic!(py, {
        if name.is_null() {
            return 0;
        }
        let Ok(name) = std::str::from_utf8(unsafe { std::slice::from_raw_parts(name, len) }) else {
            return 0;
        };
        let classes = crate::builtin_classes(py);
        let class = match name {
            "int" => Some(classes.int),
            "float" => Some(classes.float),
            "str" => Some(classes.str),
            "bytes" => Some(classes.bytes),
            "bytearray" => Some(classes.bytearray),
            _ => None,
        };
        i32::from(class.map_or_else(
            || crate::intrinsics::registry::is_named_python_builtin_materialization(py, bits, name),
            |class| bits == class,
        ))
    })
}

/// Stage/fill an owned bytearray through the shared mutation/export authority.
/// `same_size` is required at publication; preparation may reserve before the
/// final callback-free admission boundary and before device execution.
#[unsafe(no_mangle)]
pub extern "C" fn __molt_gpu_bytearray_copy(
    bits: u64,
    bytes: *const u8,
    len: usize,
    same_size: i32,
) -> i32 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(ptr) = crate::obj_from_bits(bits).as_ptr() else {
            return 0;
        };
        unsafe {
            if (bytes.is_null() && len != 0)
                || crate::object_type_id(ptr) != crate::TYPE_ID_BYTEARRAY
            {
                return 0;
            }
            if same_size != 0 && crate::object::buffer_exports::bytearray_data(ptr).1 != len {
                return 0;
            }
            let bytes = if len == 0 {
                &[]
            } else {
                std::slice::from_raw_parts(bytes, len)
            };
            i32::from(
                crate::object::buffer_exports::bytearray_mutate(py, ptr, len, |target| {
                    target.resize(len, 0);
                    target.copy_from_slice(bytes);
                })
                .is_some(),
            )
        }
    })
}

/// Publish one preallocated COW destination. Current metadata/namespace and
/// binding owners were admitted and pinned before dispatch. This operation
/// replaces an existing inferred field; it cannot allocate an attribute name,
/// grow a dictionary, invoke a descriptor, or call guest equality/setattr.
#[unsafe(no_mangle)]
pub extern "C" fn __molt_gpu_commit_buffer_data(object: u64, old: u64, new: u64) -> i32 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(object) = crate::obj_from_bits(object).as_ptr() else {
            return 0;
        };
        unsafe {
            if let Ok(Some(dictionary)) =
                crate::object::field_storage::current_dictionary(py, object)
            {
                let Some(dict) = crate::obj_from_bits(dictionary).as_ptr() else {
                    return 0;
                };
                use crate::object::ops::{
                    ExactStringLookup, dict_exact_string_lookup, hash_string_bytes,
                };
                let ExactStringLookup::Found(index) = dict_exact_string_lookup(
                    py,
                    dict,
                    b"_data",
                    hash_string_bytes(py, b"_data") as u64,
                ) else {
                    return 0;
                };
                let (key, value) = {
                    let entry = &crate::dict_entries(dict)[index];
                    (entry.key, entry.value)
                };
                if value != old {
                    return 0;
                }
                // The exact probe proved every same-hash predecessor an exact
                // string; using its retained key cannot invoke guest equality.
                let result = crate::object::ops::dict_set_deferred(py, dict, key, new);
                return i32::from(result.is_ok() && !crate::exception_pending(py));
            }
            if crate::exception_pending(py) {
                return 0;
            }
            let Some(class) = crate::obj_from_bits(crate::object_class_bits(object)).as_ptr()
            else {
                return 0;
            };
            let mut selected = None;
            crate::object::field_storage::for_each_instance_field(
                py,
                object,
                class,
                &mut |field, slot| {
                    let Some(key) = crate::obj_from_bits(field.name).as_ptr() else {
                        return;
                    };
                    if field.kind.is_inferred()
                        && crate::object_type_id(key) == crate::TYPE_ID_STRING
                        && std::slice::from_raw_parts(
                            crate::string_bytes(key),
                            crate::string_len(key),
                        ) == b"_data"
                        && *slot == old
                    {
                        selected = Some(field.offset);
                    }
                },
            );
            let Some(offset) = selected else {
                return 0;
            };
            crate::object::accessors::object_field_set_ptr_raw(py, object, offset, new);
            i32::from(!crate::exception_pending(py))
        }
    })
}
