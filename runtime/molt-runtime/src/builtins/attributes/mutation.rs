use super::*;
use crate::builtins::attr::{
    DescriptorMutation, DescriptorMutationOutcome, class_own_slot_field_offset, descriptor_call1,
    descriptor_call2, descriptor_mutate,
};
use crate::builtins::types::class_model::object_set_class;

unsafe fn readonly_descriptor_metadata(
    py: &PyToken<'_>,
    object: *mut u8,
    name: &str,
) -> Option<u64> {
    if unsafe { object_type_id(object) } == crate::TYPE_ID_NATIVE_DESCRIPTOR
        && native_descriptor_metadata_field(name).is_some()
    {
        Some(raise_exception::<u64>(
            py,
            "AttributeError",
            "readonly attribute",
        ))
    } else {
        None
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum CustomMutationDefaultPolicy {
    InvokeAnyHook,
    ContinueOnObjectDefault,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum CustomMutationDispatch {
    ContinueStorage,
    Handled,
}

/// Resolve and invoke one `__setattr__`/`__delattr__` hook without first
/// materializing a bound method. The descriptor invocation boundary pins the
/// raw hook and receiver, preserves binding exceptions, and owns the ignored
/// call result exactly once.
unsafe fn dispatch_custom_mutation(
    py: &PyToken<'_>,
    class_ptr: *mut u8,
    object_ptr: *mut u8,
    attr_bits: u64,
    mutation: DescriptorMutation,
    default_policy: CustomMutationDefaultPolicy,
) -> CustomMutationDispatch {
    unsafe {
        let names = &runtime_state(py).interned;
        let (hook_name, default_symbol) = match mutation {
            DescriptorMutation::Set(_) => (
                intern_static_name(py, &names.setattr_name, b"__setattr__"),
                fn_addr!(crate::molt_object_setattr),
            ),
            DescriptorMutation::Delete => (
                intern_static_name(py, &names.delattr_name, b"__delattr__"),
                fn_addr!(crate::molt_object_delattr),
            ),
        };
        let Some(raw_hook) = class_attr_lookup_raw_mro(py, class_ptr, hook_name) else {
            return if exception_pending(py) {
                CustomMutationDispatch::Handled
            } else {
                CustomMutationDispatch::ContinueStorage
            };
        };
        if exception_pending(py) {
            return CustomMutationDispatch::Handled;
        }
        if default_policy == CustomMutationDefaultPolicy::ContinueOnObjectDefault
            && crate::call::type_policy::callable_matches_runtime_symbol(
                Some(raw_hook),
                default_symbol,
            )
        {
            return CustomMutationDispatch::ContinueStorage;
        }

        let object_bits = MoltObject::from_ptr(object_ptr).bits();
        let result = match mutation {
            DescriptorMutation::Set(value) => {
                descriptor_call2(py, raw_hook, class_ptr, Some(object_bits), attr_bits, value)
            }
            DescriptorMutation::Delete => {
                descriptor_call1(py, raw_hook, class_ptr, Some(object_bits), attr_bits)
            }
        };
        if let Some(result_bits) = result {
            crate::call::discard_owned_call_result(py, result_bits);
        }
        CustomMutationDispatch::Handled
    }
}

#[inline]
fn finish_exception_publication(_py: &PyToken<'_>, result: Result<(), &'static str>) -> u64 {
    match result {
        Ok(()) => MoltObject::none().bits(),
        Err(_) if exception_pending(_py) => MoltObject::none().bits(),
        Err(message) => raise_exception::<u64>(_py, "SystemError", message),
    }
}

/// Translate protocol-level mutation outcomes at the attribute boundary, where
/// the owner and attribute name needed by CPython-compatible diagnostics live.
/// `None` means the class entry is not a data descriptor and ordinary storage
/// mutation should continue.
#[inline]
unsafe fn apply_descriptor_mutation(
    py: &PyToken<'_>,
    descriptor_bits: u64,
    instance_bits: u64,
    mutation: DescriptorMutation,
) -> Option<u64> {
    match unsafe { descriptor_mutate(py, descriptor_bits, instance_bits, mutation) } {
        DescriptorMutationOutcome::Applied | DescriptorMutationOutcome::Error => {
            Some(MoltObject::none().bits())
        }
        DescriptorMutationOutcome::NotDescriptor => None,
    }
}

#[inline]
unsafe fn mutate_cell_descriptor(
    py: &PyToken<'_>,
    cell_ptr: *mut u8,
    name_bits: u64,
    mutation: DescriptorMutation,
) -> Option<u64> {
    let class_bits = crate::builtins::types::cell_class(py);
    obj_from_bits(class_bits)
        .as_ptr()
        .filter(|class| unsafe { object_type_id(*class) } == TYPE_ID_TYPE)
        .and_then(|class| unsafe { class_attr_lookup_raw_mro(py, class, name_bits) })
        .and_then(|descriptor| unsafe {
            apply_descriptor_mutation(
                py,
                descriptor,
                MoltObject::from_ptr(cell_ptr).bits(),
                mutation,
            )
        })
}

/// One namespace transaction for ordinary class assignment and deletion.
/// Key lookup may call Python; after it commits, displaced values stay owned
/// until declaration metadata and the type-cache version are coherent.
unsafe fn mutate_class_namespace(
    py: &PyToken<'_>,
    class_ptr: *mut u8,
    name_bits: u64,
    name: &str,
    value: Option<u64>,
) -> bool {
    unsafe {
        let Some(dict_ptr) = obj_from_bits(class_dict_bits(class_ptr)).as_ptr() else {
            return false;
        };
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            return false;
        }
        let retired = match value {
            Some(bits) => {
                match crate::object::ops::dict_set_deferred(py, dict_ptr, name_bits, bits) {
                    Ok(retired) => retired,
                    Err(()) => return false,
                }
            }
            None => match crate::object::ops::dict_del_deferred(py, dict_ptr, name_bits) {
                Some(retired) => retired,
                None => return false,
            },
        };
        if name == "__del__" {
            crate::object::class_refresh_declared_finalizer_flag(py, class_ptr);
        }
        class_bump_layout_version(class_ptr);
        drop(retired);
        true
    }
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_set_attr_generic(
    obj_ptr: *mut u8,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
    val_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let Some(attr_name_len) = usize_from_bits(attr_name_len_bits) else {
                return raise_exception::<u64>(_py, "OverflowError", "attribute name is too large");
            };
            if obj_ptr.is_null() {
                return raise_exception::<_>(_py, "AttributeError", "object has no attribute");
            }
            let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
            let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
            let type_id = object_type_id(obj_ptr);
            if let Some(result) = readonly_descriptor_metadata(_py, obj_ptr, attr_name) {
                return result;
            }
            // Foreign (C-extension) object: route through its own `tp_setattro`
            // via the ABI bridge. This is the shared setattr helper every entry
            // point funnels through, so foreign dispatch lives here.
            if type_id == crate::TYPE_ID_FOREIGN {
                let c_ptr = crate::object::foreign::foreign_ptr_from_obj(obj_ptr);
                let Some(attr_bits) = attr_name_bits_from_bytes(_py, slice) else {
                    return MoltObject::none().bits();
                };
                let rc = molt_cpython_abi::bridge::molt_foreign_setattr(
                    c_ptr,
                    attr_bits,
                    Some(val_bits),
                );
                dec_ref_bits(_py, attr_bits);
                if rc == 0 {
                    return MoltObject::none().bits();
                }
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                return attr_error(
                    _py,
                    type_name(_py, MoltObject::from_ptr(obj_ptr)),
                    attr_name,
                );
            }
            if type_id == TYPE_ID_MODULE {
                if attr_name == "__class__" {
                    return object_set_class(_py, obj_ptr, val_bits);
                }
                let Some(attr_bits) = attr_name_bits_from_bytes(_py, slice) else {
                    return MoltObject::none().bits();
                };
                let module_bits = MoltObject::from_ptr(obj_ptr).bits();
                let res = molt_module_set_attr(module_bits, attr_bits, val_bits);
                dec_ref_bits(_py, attr_bits);
                return res;
            }
            if type_id == TYPE_ID_TYPE {
                let class_bits = MoltObject::from_ptr(obj_ptr).bits();
                if is_builtin_class_bits(_py, class_bits)
                    || crate::object::class_is_immutable(_py, obj_ptr)
                {
                    // CPython: setting an attribute on an immutable builtin type
                    // raises `cannot set '<attr>' attribute of immutable type
                    // '<type>'` (version-stable across 3.12/3.13/3.14).
                    let class_label = class_name_for_error(class_bits);
                    let msg = format!(
                        "cannot set '{attr_name}' attribute of immutable type '{class_label}'"
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                if crate::object::class_definition_is_finished(obj_ptr)
                    && matches!(attr_name, "__molt_layout_size__" | "__molt_field_offsets__")
                {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "class layout metadata is immutable",
                    );
                }
                // A class object is an instance of its metaclass. Its metaclass
                // data descriptors therefore own mutation before the class's
                // local metadata and namespace paths.
                let Some(descriptor_name_bits) = attr_name_bits_from_bytes(_py, slice) else {
                    return MoltObject::none().bits();
                };
                let metaclass_bits = type_of_bits(_py, class_bits);
                let descriptor_result = obj_from_bits(metaclass_bits)
                    .as_ptr()
                    .filter(|ptr| object_type_id(*ptr) == TYPE_ID_TYPE)
                    .and_then(|metaclass_ptr| {
                        class_attr_lookup_raw_mro(_py, metaclass_ptr, descriptor_name_bits)
                            .and_then(|descriptor_bits| {
                                apply_descriptor_mutation(
                                    _py,
                                    descriptor_bits,
                                    class_bits,
                                    DescriptorMutation::Set(val_bits),
                                )
                            })
                    });
                dec_ref_bits(_py, descriptor_name_bits);
                if let Some(result) = descriptor_result {
                    return result;
                }
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                if attr_name == "__name__" || attr_name == "__qualname__" {
                    let val_obj = obj_from_bits(val_bits);
                    let is_str = if let Some(val_ptr) = val_obj.as_ptr() {
                        object_type_id(val_ptr) == TYPE_ID_STRING
                    } else {
                        false
                    };
                    if !is_str {
                        let class_label = class_name_for_error(class_bits);
                        let type_label = type_name(_py, val_obj);
                        let msg = format!(
                            "can only assign string to {class_label}.{attr_name}, not '{}'",
                            type_label
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                    if attr_name == "__name__" {
                        class_set_name_bits(_py, obj_ptr, val_bits);
                    } else {
                        class_set_qualname_bits(_py, obj_ptr, val_bits);
                    }
                    class_bump_layout_version(obj_ptr);
                    return MoltObject::none().bits();
                }
                if attr_name == "__annotate__" && pep649_enabled(_py) {
                    let val_obj = obj_from_bits(val_bits);
                    if !val_obj.is_none() {
                        let callable_ok = is_truthy(_py, obj_from_bits(molt_is_callable(val_bits)));
                        if !callable_ok {
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                "__annotate__ must be callable or None",
                            );
                        }
                        class_set_annotations_bits(_py, obj_ptr, 0u64);
                    }
                    let dict_bits = class_dict_bits(obj_ptr);
                    if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                        && object_type_id(dict_ptr) == TYPE_ID_DICT
                    {
                        let annotate_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.annotate_name,
                            b"__annotate__",
                        );
                        dict_set_in_place(_py, dict_ptr, annotate_bits, val_bits);
                        if !val_obj.is_none() {
                            let annotations_bits = intern_static_name(
                                _py,
                                &runtime_state(_py).interned.annotations_name,
                                b"__annotations__",
                            );
                            dict_del_in_place(_py, dict_ptr, annotations_bits);
                        }
                    }
                    class_set_annotate_bits(_py, obj_ptr, val_bits);
                    class_bump_layout_version(obj_ptr);
                    return MoltObject::none().bits();
                }
                if attr_name == "__annotations__" {
                    let dict_bits = class_dict_bits(obj_ptr);
                    if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                        && object_type_id(dict_ptr) == TYPE_ID_DICT
                    {
                        let annotations_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.annotations_name,
                            b"__annotations__",
                        );
                        dict_set_in_place(_py, dict_ptr, annotations_bits, val_bits);
                        let annotate_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.annotate_name,
                            b"__annotate__",
                        );
                        let none_bits = MoltObject::none().bits();
                        if pep649_enabled(_py) {
                            dict_set_in_place(_py, dict_ptr, annotate_bits, none_bits);
                        }
                    }
                    class_set_annotations_bits(_py, obj_ptr, val_bits);
                    if pep649_enabled(_py) {
                        class_set_annotate_bits(_py, obj_ptr, MoltObject::none().bits());
                    }
                    class_bump_layout_version(obj_ptr);
                    return MoltObject::none().bits();
                }
                let Some(attr_bits) = attr_name_bits_from_bytes(_py, slice) else {
                    return MoltObject::none().bits();
                };
                if mutate_class_namespace(_py, obj_ptr, attr_bits, attr_name, Some(val_bits))
                    || exception_pending(_py)
                {
                    dec_ref_bits(_py, attr_bits);
                    return MoltObject::none().bits();
                }
                dec_ref_bits(_py, attr_bits);
                return attr_error(_py, "type", attr_name);
            }
            if type_id == TYPE_ID_EXCEPTION {
                let Some(attr_bits) = attr_name_bits_from_bytes(_py, slice) else {
                    return MoltObject::none().bits();
                };
                let name = string_obj_to_owned(obj_from_bits(attr_bits)).unwrap_or_default();
                if name == "__class__" {
                    dec_ref_bits(_py, attr_bits);
                    return object_set_class(_py, obj_ptr, val_bits);
                }
                if let Some(result) = exception_typed_field_replace(
                    _py,
                    MoltObject::from_ptr(obj_ptr).bits(),
                    &name,
                    val_bits,
                ) {
                    dec_ref_bits(_py, attr_bits);
                    return match result {
                        Ok(()) => MoltObject::none().bits(),
                        Err(_) if exception_pending(_py) => MoltObject::none().bits(),
                        Err(message) => raise_exception::<u64>(_py, "AttributeError", message),
                    };
                }
                if name == "__cause__" || name == "__context__" {
                    let val_obj = obj_from_bits(val_bits);
                    if !val_obj.is_none() {
                        let Some(val_ptr) = val_obj.as_ptr() else {
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                if name == "__cause__" {
                                    "exception cause must be an exception or None"
                                } else {
                                    "exception context must be an exception or None"
                                },
                            );
                        };
                        if object_type_id(val_ptr) != TYPE_ID_EXCEPTION {
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                if name == "__cause__" {
                                    "exception cause must be an exception or None"
                                } else {
                                    "exception context must be an exception or None"
                                },
                            );
                        }
                    }
                    let field = if name == "__cause__" {
                        ExceptionFieldSlot::Cause
                    } else {
                        ExceptionFieldSlot::Context
                    };
                    let result = exception_replace_field_bits(
                        _py,
                        MoltObject::from_ptr(obj_ptr).bits(),
                        field,
                        val_bits,
                    );
                    dec_ref_bits(_py, attr_bits);
                    return finish_exception_publication(_py, result);
                }
                if name == "args" {
                    let args_bits = exception_args_from_iterable(_py, val_bits);
                    if obj_from_bits(args_bits).is_none() {
                        dec_ref_bits(_py, attr_bits);
                        return MoltObject::none().bits();
                    }
                    let class_bits = object_class_bits(obj_ptr);
                    let msg_bits = crate::exception_message_for_storage(_py, class_bits, args_bits);
                    if obj_from_bits(msg_bits).is_none() {
                        dec_ref_bits(_py, args_bits);
                        dec_ref_bits(_py, attr_bits);
                        return MoltObject::none().bits();
                    }
                    if !exception_store_args_and_message(_py, obj_ptr, args_bits, msg_bits) {
                        dec_ref_bits(_py, attr_bits);
                        return MoltObject::none().bits();
                    }
                    dec_ref_bits(_py, attr_bits);
                    return MoltObject::none().bits();
                }
                if name == "__suppress_context__" {
                    let suppress = is_truthy(_py, obj_from_bits(val_bits));
                    let result = exception_replace_suppress_context(
                        _py,
                        MoltObject::from_ptr(obj_ptr).bits(),
                        suppress,
                    );
                    dec_ref_bits(_py, attr_bits);
                    return finish_exception_publication(_py, result);
                }
                if name == "__notes__" {
                    let result = exception_replace_field_bits(
                        _py,
                        MoltObject::from_ptr(obj_ptr).bits(),
                        ExceptionFieldSlot::Notes,
                        val_bits,
                    );
                    dec_ref_bits(_py, attr_bits);
                    return finish_exception_publication(_py, result);
                }
                if name == "__dict__" {
                    let val_obj = obj_from_bits(val_bits);
                    let Some(val_ptr) = val_obj.as_ptr() else {
                        let msg = format!(
                            "__dict__ must be set to a dictionary, not a '{}'",
                            type_name(_py, val_obj)
                        );
                        dec_ref_bits(_py, attr_bits);
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    };
                    if object_type_id(val_ptr) != TYPE_ID_DICT {
                        let msg = format!(
                            "__dict__ must be set to a dictionary, not a '{}'",
                            type_name(_py, val_obj)
                        );
                        dec_ref_bits(_py, attr_bits);
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                    let result = exception_replace_field_bits(
                        _py,
                        MoltObject::from_ptr(obj_ptr).bits(),
                        ExceptionFieldSlot::Dict,
                        val_bits,
                    );
                    dec_ref_bits(_py, attr_bits);
                    return finish_exception_publication(_py, result);
                }
                let mut dict_bits = exception_dict_bits(obj_ptr);
                if obj_from_bits(dict_bits).is_none() || dict_bits == 0 {
                    let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                    if !dict_ptr.is_null() {
                        dict_bits = MoltObject::from_ptr(dict_ptr).bits();
                        if exception_replace_field_bits(
                            _py,
                            MoltObject::from_ptr(obj_ptr).bits(),
                            ExceptionFieldSlot::Dict,
                            dict_bits,
                        )
                        .is_err()
                        {
                            dec_ref_bits(_py, dict_bits);
                            dec_ref_bits(_py, attr_bits);
                            return MoltObject::none().bits();
                        }
                        dec_ref_bits(_py, dict_bits);
                    }
                }
                if !obj_from_bits(dict_bits).is_none()
                    && dict_bits != 0
                    && let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                    && object_type_id(dict_ptr) == TYPE_ID_DICT
                {
                    dict_set_in_place(_py, dict_ptr, attr_bits, val_bits);
                    dec_ref_bits(_py, attr_bits);
                    return MoltObject::none().bits();
                }
                dec_ref_bits(_py, attr_bits);
                return attr_error(_py, "exception", attr_name);
            }
            if type_id == crate::TYPE_ID_CELL {
                let Some(attr_bits) = attr_name_bits_from_bytes(_py, slice) else {
                    return MoltObject::none().bits();
                };
                let result = mutate_cell_descriptor(
                    _py,
                    obj_ptr,
                    attr_bits,
                    DescriptorMutation::Set(val_bits),
                );
                dec_ref_bits(_py, attr_bits);
                if let Some(result) = result {
                    return result;
                }
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                return attr_error(_py, "cell", attr_name);
            }
            if type_id == TYPE_ID_FUNCTION {
                if attr_name == "__module__"
                    && let Some(ok) = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                        .set_cfunction_module(MoltObject::from_ptr(obj_ptr).bits(), Some(val_bits))
                {
                    if !ok {
                        crate::cpython_abi_hooks::transfer_pending_cpython_exception();
                    }
                    return MoltObject::none().bits();
                }
                if attr_name == "__code__" {
                    if builtin_classes(_py).is_builtin_callable_class(object_class_bits(obj_ptr)) {
                        return raise_exception::<_>(
                            _py,
                            "AttributeError",
                            &format!(
                                "'{}' object has no attribute '__code__'",
                                type_name(_py, MoltObject::from_ptr(obj_ptr)),
                            ),
                        );
                    }
                    let val_obj = obj_from_bits(val_bits);
                    let Some(val_ptr) = val_obj.as_ptr() else {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "function __code__ must be a code object",
                        );
                    };
                    if object_type_id(val_ptr) != TYPE_ID_CODE {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "function __code__ must be a code object",
                        );
                    }
                    let Some(code_identity) =
                        crate::object::layout::code_callable_identity(val_ptr)
                    else {
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "function __code__ must have a published callable identity",
                        );
                    };
                    if !code_identity.call_abi.is_reconstructible() {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "cannot assign opaque runtime-context code to a Python function",
                        );
                    }
                    let function_abi = function_call_abi(obj_ptr);
                    if !function_abi.is_reconstructible() {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "cannot replace code on an opaque runtime-context function",
                        );
                    }
                    if function_abi != code_identity.call_abi {
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "function and code object use different callable context ABIs",
                        );
                    }
                    let closure_bits = function_closure_bits(obj_ptr);
                    let closure_len = if closure_bits == 0 || obj_from_bits(closure_bits).is_none()
                    {
                        0
                    } else {
                        let Some(closure) = crate::object::cells::inspect_cell_tuple(closure_bits)
                        else {
                            return raise_exception::<_>(
                                _py,
                                "SystemError",
                                "function closure is not a tuple",
                            );
                        };
                        if closure.first_non_cell.is_some() {
                            return raise_exception::<_>(
                                _py,
                                "SystemError",
                                "function closure contains a non-cell value",
                            );
                        }
                        closure.len
                    };
                    let freevars_bits = code_freevars_bits(val_ptr);
                    let Some(freevars_ptr) = obj_from_bits(freevars_bits).as_ptr() else {
                        return raise_exception::<_>(
                            _py,
                            "SystemError",
                            "code freevars metadata is not a tuple",
                        );
                    };
                    if object_type_id(freevars_ptr) != TYPE_ID_TUPLE {
                        return raise_exception::<_>(
                            _py,
                            "SystemError",
                            "code freevars metadata is not a tuple",
                        );
                    }
                    let freevars_len = crate::object::seq_access::len(freevars_ptr);
                    if closure_len != freevars_len {
                        let name =
                            string_obj_to_owned(obj_from_bits(function_name_bits(_py, obj_ptr)))
                                .unwrap_or_else(|| "<function>".to_string());
                        let msg = format!(
                            "{name}() requires a code object with {closure_len} free vars, not {freevars_len}"
                        );
                        return raise_exception::<_>(_py, "ValueError", &msg);
                    }
                    if !crate::builtins::functions::function_replace_code_bits(
                        _py, obj_ptr, val_bits,
                    ) {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::none().bits();
                }
                if attr_name == "__closure__" {
                    return raise_exception::<_>(_py, "AttributeError", "readonly attribute");
                }
                if attr_name == "__annotate__" && pep649_enabled(_py) {
                    let val_obj = obj_from_bits(val_bits);
                    if !val_obj.is_none() {
                        let callable_ok = is_truthy(_py, obj_from_bits(molt_is_callable(val_bits)));
                        if !callable_ok {
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                "__annotate__ must be callable or None",
                            );
                        }
                        function_set_annotations_bits(_py, obj_ptr, 0);
                    }
                    function_set_annotate_bits(_py, obj_ptr, val_bits);
                    return MoltObject::none().bits();
                }
                if attr_name == "__annotations__" {
                    let val_obj = obj_from_bits(val_bits);
                    let ann_bits = if val_obj.is_none() {
                        let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                        if dict_ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        MoltObject::from_ptr(dict_ptr).bits()
                    } else {
                        let Some(val_ptr) = val_obj.as_ptr() else {
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                "__annotations__ must be set to a dict object",
                            );
                        };
                        if object_type_id(val_ptr) != TYPE_ID_DICT {
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                "__annotations__ must be set to a dict object",
                            );
                        }
                        val_bits
                    };
                    function_set_annotations_bits(_py, obj_ptr, ann_bits);
                    if pep649_enabled(_py) {
                        function_set_annotate_bits(_py, obj_ptr, MoltObject::none().bits());
                    }
                    return MoltObject::none().bits();
                }
                let Some(attr_bits) = attr_name_bits_from_bytes(_py, slice) else {
                    return MoltObject::none().bits();
                };
                if let Ok(publication) = crate::call::class_init::function_set_attr_bits_deferred(
                    _py, obj_ptr, attr_bits, val_bits,
                ) {
                    crate::call::function::commit_function_metadata_change(
                        _py,
                        obj_ptr,
                        attr_name.as_bytes(),
                        true,
                    );
                    drop(publication);
                }
                dec_ref_bits(_py, attr_bits);
                return MoltObject::none().bits();
            }
            if type_id == TYPE_ID_CODE {
                return attr_error(_py, "code", attr_name);
            }
            if type_id == TYPE_ID_DATACLASS {
                let Some(attr_bits) = attr_name_bits_from_bytes(_py, slice) else {
                    return MoltObject::none().bits();
                };
                if let Some(class_ptr) = obj_from_bits(object_class_bits(obj_ptr)).as_ptr()
                    && object_type_id(class_ptr) == TYPE_ID_TYPE
                    && dispatch_custom_mutation(
                        _py,
                        class_ptr,
                        obj_ptr,
                        attr_bits,
                        DescriptorMutation::Set(val_bits),
                        CustomMutationDefaultPolicy::InvokeAnyHook,
                    ) == CustomMutationDispatch::Handled
                {
                    dec_ref_bits(_py, attr_bits);
                    return MoltObject::none().bits();
                }
                let result =
                    dataclass_setattr_inner(_py, obj_ptr, attr_bits, attr_name, val_bits, true);
                dec_ref_bits(_py, attr_bits);
                return result;
            }
            if crate::object::heap_kind_has_class_shape(type_id) {
                let _header = header_from_obj_ptr(obj_ptr);
                if type_id == TYPE_ID_OBJECT && crate::object::object_poll_fn(obj_ptr) != 0 {
                    return attr_error_with_obj(
                        _py,
                        "object",
                        attr_name,
                        MoltObject::from_ptr(obj_ptr).bits(),
                    );
                }
                let payload = object_payload_size(obj_ptr);
                if payload < std::mem::size_of::<u64>() {
                    return attr_error_with_obj(
                        _py,
                        "object",
                        attr_name,
                        MoltObject::from_ptr(obj_ptr).bits(),
                    );
                }
                let Some(attr_bits) = attr_name_bits_from_bytes(_py, slice) else {
                    return MoltObject::none().bits();
                };
                let class_bits = object_class_bits(obj_ptr);
                let mut slots_info = None;
                if class_bits != 0
                    && let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
                    && object_type_id(class_ptr) == TYPE_ID_TYPE
                {
                    slots_info = class_slots_info(_py, class_ptr);
                    if dispatch_custom_mutation(
                        _py,
                        class_ptr,
                        obj_ptr,
                        attr_bits,
                        DescriptorMutation::Set(val_bits),
                        CustomMutationDefaultPolicy::ContinueOnObjectDefault,
                    ) == CustomMutationDispatch::Handled
                    {
                        dec_ref_bits(_py, attr_bits);
                        return MoltObject::none().bits();
                    }
                    if let Some(offset) = class_own_slot_field_offset(_py, class_ptr, attr_bits) {
                        let res = object_field_set_ptr_raw(_py, obj_ptr, offset, val_bits);
                        dec_ref_bits(_py, attr_bits);
                        return res;
                    }
                    if let Some(desc_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)
                        && let Some(result) = apply_descriptor_mutation(
                            _py,
                            desc_bits,
                            instance_bits_for_call(obj_ptr),
                            DescriptorMutation::Set(val_bits),
                        )
                    {
                        dec_ref_bits(_py, attr_bits);
                        return result;
                    }
                    if attr_name == "__class__" {
                        dec_ref_bits(_py, attr_bits);
                        return object_set_class(_py, obj_ptr, val_bits);
                    }
                    if let Some(offset) = class_field_offset(_py, class_ptr, attr_bits) {
                        object_field_set_ptr_raw(_py, obj_ptr, offset, val_bits);
                        dec_ref_bits(_py, attr_bits);
                        return MoltObject::none().bits();
                    }
                }
                if attr_name == "__class__" {
                    dec_ref_bits(_py, attr_bits);
                    return object_set_class(_py, obj_ptr, val_bits);
                }
                if let Some(info) = slots_info
                    && !info.allows_dict
                {
                    dec_ref_bits(_py, attr_bits);
                    // A `__slots__` instance with no `__dict__` rejecting an
                    // attribute that is not one of its slots. CPython 3.13+ adds
                    // "and no __dict__ for setting new attributes" on the SET path.
                    let type_label = class_name_for_error(class_bits);
                    return setattr_no_attr_error_with_obj(
                        _py,
                        type_label,
                        attr_name,
                        MoltObject::from_ptr(obj_ptr).bits(),
                    );
                }
                if attr_name == "__dict__" {
                    crate::object::field_storage::replace_dictionary(_py, obj_ptr, Some(val_bits));
                    dec_ref_bits(_py, attr_bits);
                    return MoltObject::none().bits();
                }
                let Some(dict_bits) = crate::object::field_storage::materialize(_py, obj_ptr)
                else {
                    dec_ref_bits(_py, attr_bits);
                    return MoltObject::none().bits();
                };
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                    && object_type_id(dict_ptr) == TYPE_ID_DICT
                {
                    inc_ref_bits(_py, dict_bits);
                    dict_set_in_place(_py, dict_ptr, attr_bits, val_bits);
                    dec_ref_bits(_py, dict_bits);
                    dec_ref_bits(_py, attr_bits);
                    return MoltObject::none().bits();
                }
                dec_ref_bits(_py, attr_bits);
                return setattr_no_attr_error_with_obj(
                    _py,
                    "object",
                    attr_name,
                    MoltObject::from_ptr(obj_ptr).bits(),
                );
            }
            setattr_no_attr_error_with_obj(
                _py,
                type_name(_py, MoltObject::from_ptr(obj_ptr)),
                attr_name,
                MoltObject::from_ptr(obj_ptr).bits(),
            )
        })
    }
}

pub(crate) unsafe fn del_attr_ptr(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    attr_name: &str,
) -> u64 {
    unsafe {
        let type_id = object_type_id(obj_ptr);
        if let Some(result) = readonly_descriptor_metadata(_py, obj_ptr, attr_name) {
            return result;
        }
        if type_id == crate::TYPE_ID_FOREIGN {
            let c_ptr = crate::object::foreign::foreign_ptr_from_obj(obj_ptr);
            let rc = molt_cpython_abi::bridge::molt_foreign_setattr(c_ptr, attr_bits, None);
            if rc == 0 || exception_pending(_py) {
                return MoltObject::none().bits();
            }
            return attr_error(
                _py,
                type_name(_py, MoltObject::from_ptr(obj_ptr)),
                attr_name,
            );
        }
        if type_id == TYPE_ID_MODULE {
            let dict_bits = module_dict_bits(obj_ptr);
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                && object_type_id(dict_ptr) == TYPE_ID_DICT
            {
                let annotations_bits = intern_static_name(
                    _py,
                    &runtime_state(_py).interned.annotations_name,
                    b"__annotations__",
                );
                if obj_eq(
                    _py,
                    obj_from_bits(attr_bits),
                    obj_from_bits(annotations_bits),
                ) {
                    if dict_del_in_place(_py, dict_ptr, annotations_bits) {
                        if pep649_enabled(_py) {
                            let annotate_bits = intern_static_name(
                                _py,
                                &runtime_state(_py).interned.annotate_name,
                                b"__annotate__",
                            );
                            let none_bits = MoltObject::none().bits();
                            dict_set_in_place(_py, dict_ptr, annotate_bits, none_bits);
                        }
                        return MoltObject::none().bits();
                    }
                    let module_name = string_obj_to_owned(obj_from_bits(module_name_bits(obj_ptr)))
                        .unwrap_or_default();
                    let msg = format!("module '{module_name}' has no attribute '{attr_name}'");
                    return raise_exception::<_>(_py, "AttributeError", &msg);
                }
                let annotate_bits = intern_static_name(
                    _py,
                    &runtime_state(_py).interned.annotate_name,
                    b"__annotate__",
                );
                if obj_eq(_py, obj_from_bits(attr_bits), obj_from_bits(annotate_bits))
                    && pep649_enabled(_py)
                {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "cannot delete __annotate__ attribute",
                    );
                }
                if dict_del_in_place(_py, dict_ptr, attr_bits) {
                    return MoltObject::none().bits();
                }
            }
            let module_name =
                string_obj_to_owned(obj_from_bits(module_name_bits(obj_ptr))).unwrap_or_default();
            let msg = format!("module '{module_name}' has no attribute '{attr_name}'");
            return attr_error_with_message(_py, &msg);
        }
        if type_id == TYPE_ID_TYPE {
            let class_bits = MoltObject::from_ptr(obj_ptr).bits();
            if is_builtin_class_bits(_py, class_bits)
                || crate::object::class_is_immutable(_py, obj_ptr)
            {
                // CPython routes `del <builtin_type>.<attr>` through the same
                // immutable-type guard as set, yielding `cannot set '<attr>'
                // attribute of immutable type '<type>'` (version-stable).
                let class_label = class_name_for_error(class_bits);
                let msg =
                    format!("cannot set '{attr_name}' attribute of immutable type '{class_label}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            if crate::object::class_definition_is_finished(obj_ptr)
                && matches!(attr_name, "__molt_layout_size__" | "__molt_field_offsets__")
            {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "class layout metadata is immutable",
                );
            }
            let metaclass_bits = type_of_bits(_py, class_bits);
            let descriptor_result = obj_from_bits(metaclass_bits)
                .as_ptr()
                .filter(|ptr| object_type_id(*ptr) == TYPE_ID_TYPE)
                .and_then(|metaclass_ptr| {
                    class_attr_lookup_raw_mro(_py, metaclass_ptr, attr_bits).and_then(
                        |descriptor_bits| {
                            apply_descriptor_mutation(
                                _py,
                                descriptor_bits,
                                class_bits,
                                DescriptorMutation::Delete,
                            )
                        },
                    )
                });
            if let Some(result) = descriptor_result {
                return result;
            }
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if attr_name == "__annotate__" && pep649_enabled(_py) {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "cannot delete __annotate__ attribute",
                );
            }
            if attr_name == "__annotations__" {
                let dict_bits = class_dict_bits(obj_ptr);
                let mut removed = false;
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                    && object_type_id(dict_ptr) == TYPE_ID_DICT
                {
                    let annotations_bits = intern_static_name(
                        _py,
                        &runtime_state(_py).interned.annotations_name,
                        b"__annotations__",
                    );
                    if dict_del_in_place(_py, dict_ptr, annotations_bits) {
                        removed = true;
                    }
                    if removed && pep649_enabled(_py) {
                        let annotate_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.annotate_name,
                            b"__annotate__",
                        );
                        let none_bits = MoltObject::none().bits();
                        dict_set_in_place(_py, dict_ptr, annotate_bits, none_bits);
                    }
                }
                if !removed && class_annotations_bits(obj_ptr) != 0 {
                    removed = true;
                }
                if removed {
                    class_set_annotations_bits(_py, obj_ptr, 0u64);
                    if pep649_enabled(_py) {
                        class_set_annotate_bits(_py, obj_ptr, MoltObject::none().bits());
                    }
                    class_bump_layout_version(obj_ptr);
                    return MoltObject::none().bits();
                }
                let class_name = string_obj_to_owned(obj_from_bits(class_name_bits(obj_ptr)))
                    .unwrap_or_default();
                let msg = format!("type object '{class_name}' has no attribute '{attr_name}'");
                return raise_exception::<_>(_py, "AttributeError", &msg);
            }
            if mutate_class_namespace(_py, obj_ptr, attr_bits, attr_name, None)
                || exception_pending(_py)
            {
                return MoltObject::none().bits();
            }
            let class_name =
                string_obj_to_owned(obj_from_bits(class_name_bits(obj_ptr))).unwrap_or_default();
            let msg = format!("type object '{class_name}' has no attribute '{attr_name}'");
            return attr_error_with_message(_py, &msg);
        }
        if type_id == TYPE_ID_EXCEPTION {
            if let Some(result) =
                exception_typed_field_delete(_py, MoltObject::from_ptr(obj_ptr).bits(), attr_name)
            {
                return match result {
                    Ok(()) => MoltObject::none().bits(),
                    Err(_) if exception_pending(_py) => MoltObject::none().bits(),
                    Err(message) => raise_exception::<u64>(_py, "AttributeError", message),
                };
            }
            if attr_name == "__cause__" || attr_name == "__context__" {
                let field = if attr_name == "__cause__" {
                    ExceptionFieldSlot::Cause
                } else {
                    ExceptionFieldSlot::Context
                };
                let result = exception_replace_field_bits(
                    _py,
                    MoltObject::from_ptr(obj_ptr).bits(),
                    field,
                    MoltObject::none().bits(),
                );
                if result.is_err() {
                    return finish_exception_publication(_py, result);
                }
                if attr_name == "__cause__" {
                    let result = exception_replace_suppress_context(
                        _py,
                        MoltObject::from_ptr(obj_ptr).bits(),
                        false,
                    );
                    if result.is_err() {
                        return finish_exception_publication(_py, result);
                    }
                }
                return MoltObject::none().bits();
            }
            if attr_name == "__suppress_context__" {
                let result = exception_replace_suppress_context(
                    _py,
                    MoltObject::from_ptr(obj_ptr).bits(),
                    false,
                );
                return finish_exception_publication(_py, result);
            }
            if attr_name == "__notes__" {
                let result = exception_replace_field_bits(
                    _py,
                    MoltObject::from_ptr(obj_ptr).bits(),
                    ExceptionFieldSlot::Notes,
                    MoltObject::none().bits(),
                );
                return finish_exception_publication(_py, result);
            }
            let dict_bits = exception_dict_bits(obj_ptr);
            if !obj_from_bits(dict_bits).is_none()
                && dict_bits != 0
                && let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                && object_type_id(dict_ptr) == TYPE_ID_DICT
                && dict_del_in_place(_py, dict_ptr, attr_bits)
            {
                return MoltObject::none().bits();
            }
            return attr_error(_py, "exception", attr_name);
        }
        if type_id == crate::TYPE_ID_CELL {
            let result =
                mutate_cell_descriptor(_py, obj_ptr, attr_bits, DescriptorMutation::Delete);
            if let Some(result) = result {
                return result;
            }
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            return attr_error(_py, "cell", attr_name);
        }
        if type_id == TYPE_ID_FUNCTION {
            if attr_name == "__annotate__" && pep649_enabled(_py) {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "cannot delete __annotate__ attribute",
                );
            }
            if attr_name == "__annotations__" {
                function_set_annotations_bits(_py, obj_ptr, 0);
                if pep649_enabled(_py) {
                    function_set_annotate_bits(_py, obj_ptr, MoltObject::none().bits());
                }
                return MoltObject::none().bits();
            }
            if attr_name == "__module__" {
                if let Some(ok) = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                    .set_cfunction_module(MoltObject::from_ptr(obj_ptr).bits(), None)
                {
                    if !ok {
                        crate::cpython_abi_hooks::transfer_pending_cpython_exception();
                    }
                    return MoltObject::none().bits();
                }
                // CPython function metadata retains an explicit None after
                // deletion; falling through to the type descriptor is wrong.
                let _ = crate::call::class_init::function_set_attr_bits(
                    _py,
                    obj_ptr,
                    attr_bits,
                    MoltObject::none().bits(),
                );
                return MoltObject::none().bits();
            }
            let dict_bits = function_dict_bits(obj_ptr);
            if dict_bits == 0 {
                return attr_error(_py, "function", attr_name);
            }
            let Some(dict_ptr) = crate::call::class_init::function_ensure_dict(_py, obj_ptr) else {
                return MoltObject::none().bits();
            };
            if let Some(publication) =
                crate::object::ops::dict_del_deferred(_py, dict_ptr, attr_bits)
            {
                crate::call::function::commit_function_metadata_change(
                    _py,
                    obj_ptr,
                    attr_name.as_bytes(),
                    true,
                );
                drop(publication);
                return MoltObject::none().bits();
            }
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            return attr_error(_py, "function", attr_name);
        }
        if type_id == TYPE_ID_DATACLASS {
            if let Some(class_ptr) = obj_from_bits(object_class_bits(obj_ptr)).as_ptr()
                && object_type_id(class_ptr) == TYPE_ID_TYPE
                && dispatch_custom_mutation(
                    _py,
                    class_ptr,
                    obj_ptr,
                    attr_bits,
                    DescriptorMutation::Delete,
                    CustomMutationDefaultPolicy::InvokeAnyHook,
                ) == CustomMutationDispatch::Handled
            {
                return MoltObject::none().bits();
            }
            return dataclass_delattr_inner(_py, obj_ptr, attr_bits, attr_name, true);
        }
        if crate::object::heap_kind_has_class_shape(type_id) {
            let _header = header_from_obj_ptr(obj_ptr);
            if type_id == TYPE_ID_OBJECT && crate::object::object_poll_fn(obj_ptr) != 0 {
                return attr_error(_py, "object", attr_name);
            }
            let payload = object_payload_size(obj_ptr);
            if payload < std::mem::size_of::<u64>() {
                return attr_error(_py, "object", attr_name);
            }
            let class_bits = object_class_bits(obj_ptr);
            if class_bits != 0
                && let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
                && object_type_id(class_ptr) == TYPE_ID_TYPE
            {
                if dispatch_custom_mutation(
                    _py,
                    class_ptr,
                    obj_ptr,
                    attr_bits,
                    DescriptorMutation::Delete,
                    CustomMutationDefaultPolicy::ContinueOnObjectDefault,
                ) == CustomMutationDispatch::Handled
                {
                    return MoltObject::none().bits();
                }
                if let Some(desc_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)
                    && let Some(result) = apply_descriptor_mutation(
                        _py,
                        desc_bits,
                        instance_bits_for_call(obj_ptr),
                        DescriptorMutation::Delete,
                    )
                {
                    return result;
                }
            }
            if attr_name == "__dict__" {
                crate::object::field_storage::replace_dictionary(_py, obj_ptr, None);
                return MoltObject::none().bits();
            }
            if class_bits != 0
                && let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
                && object_type_id(class_ptr) == TYPE_ID_TYPE
                && let Some(offset) = class_field_offset(_py, class_ptr, attr_bits)
            {
                if !crate::object::accessors::object_field_delete_ptr_raw(_py, obj_ptr, offset) {
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    return attr_error(_py, "object", attr_name);
                }
                return MoltObject::none().bits();
            }
            if crate::object::accessors::instance_attribute_delete(_py, obj_ptr, attr_bits) {
                return MoltObject::none().bits();
            }
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            return attr_error(_py, "object", attr_name);
        }
        // Final fallthrough: DEL of a missing attribute on a no-`__dict__` heap
        // builtin (str/tuple/bytes/frozenset/...). CPython routes del through the
        // generic-setattr-with-NULL path, so the message carries the same
        // version-gated "no __dict__ for setting new attributes" clause (3.13+).
        setattr_no_attr_error_with_obj(
            _py,
            type_name(_py, MoltObject::from_ptr(obj_ptr)),
            attr_name,
            MoltObject::from_ptr(obj_ptr).bits(),
        )
    }
}

pub(crate) unsafe fn object_setattr_raw(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    attr_name: &str,
    val_bits: u64,
) -> u64 {
    unsafe {
        if let Some(result) = readonly_descriptor_metadata(_py, obj_ptr, attr_name) {
            return result;
        }
        let _header = header_from_obj_ptr(obj_ptr);
        if object_type_id(obj_ptr) == TYPE_ID_OBJECT && crate::object::object_poll_fn(obj_ptr) != 0
        {
            return attr_error_with_obj(
                _py,
                "object",
                attr_name,
                MoltObject::from_ptr(obj_ptr).bits(),
            );
        }
        let payload = object_payload_size(obj_ptr);
        if payload < std::mem::size_of::<u64>() {
            return attr_error_with_obj(
                _py,
                "object",
                attr_name,
                MoltObject::from_ptr(obj_ptr).bits(),
            );
        }
        let class_bits = object_class_bits(obj_ptr);
        let mut slots_info = None;
        if class_bits != 0
            && let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
            && object_type_id(class_ptr) == TYPE_ID_TYPE
        {
            slots_info = class_slots_info(_py, class_ptr);
            if let Some(offset) = class_own_slot_field_offset(_py, class_ptr, attr_bits) {
                return object_field_set_ptr_raw(_py, obj_ptr, offset, val_bits);
            }
            if let Some(desc_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)
                && let Some(result) = apply_descriptor_mutation(
                    _py,
                    desc_bits,
                    instance_bits_for_call(obj_ptr),
                    DescriptorMutation::Set(val_bits),
                )
            {
                return result;
            }
            if attr_name == "__class__" {
                return object_set_class(_py, obj_ptr, val_bits);
            }
            if let Some(offset) = class_field_offset(_py, class_ptr, attr_bits) {
                return object_field_set_ptr_raw(_py, obj_ptr, offset, val_bits);
            }
        }
        if let Some(info) = slots_info
            && !info.allows_dict
        {
            // `__slots__` instance (no `__dict__`) rejecting a non-slot attribute
            // via the `setattr()` builtin path: version-gated no-`__dict__` SET
            // message (3.13+), matching `molt_set_attr_generic`.
            let type_label = class_name_for_error(class_bits);
            return setattr_no_attr_error_with_obj(
                _py,
                type_label,
                attr_name,
                MoltObject::from_ptr(obj_ptr).bits(),
            );
        }
        if attr_name == "__dict__" {
            crate::object::field_storage::replace_dictionary(_py, obj_ptr, Some(val_bits));
            return MoltObject::none().bits();
        }
        let Some(dict_bits) = crate::object::field_storage::materialize(_py, obj_ptr) else {
            return MoltObject::none().bits();
        };
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
            && object_type_id(dict_ptr) == TYPE_ID_DICT
        {
            inc_ref_bits(_py, dict_bits);
            dict_set_in_place(_py, dict_ptr, attr_bits, val_bits);
            dec_ref_bits(_py, dict_bits);
            return MoltObject::none().bits();
        }
        setattr_no_attr_error_with_obj(
            _py,
            "object",
            attr_name,
            MoltObject::from_ptr(obj_ptr).bits(),
        )
    }
}

unsafe fn dataclass_setattr_inner(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    attr_name: &str,
    val_bits: u64,
    enforce_frozen: bool,
) -> u64 {
    unsafe {
        let desc_ptr = dataclass_desc_ptr(obj_ptr);
        if enforce_frozen && !desc_ptr.is_null() && (*desc_ptr).frozen {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "cannot assign to frozen dataclass field",
            );
        }
        if !desc_ptr.is_null() {
            let class_bits = object_class_bits(obj_ptr);
            if let Some(&index) = (*desc_ptr).field_name_to_index.get(attr_name)
                && crate::object::field_storage::field_at_offset(
                    _py,
                    obj_ptr,
                    index * std::mem::size_of::<u64>(),
                )
                .is_some_and(|field| field.declared_slot)
            {
                return crate::object::accessors::object_field_set_ptr_raw(
                    _py,
                    obj_ptr,
                    index * std::mem::size_of::<u64>(),
                    val_bits,
                );
            }
            if class_bits != 0
                && let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
                && object_type_id(class_ptr) == TYPE_ID_TYPE
                && let Some(desc_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)
                && let Some(result) = apply_descriptor_mutation(
                    _py,
                    desc_bits,
                    instance_bits_for_call(obj_ptr),
                    DescriptorMutation::Set(val_bits),
                )
            {
                return result;
            }
            if attr_name == "__class__" {
                return object_set_class(_py, obj_ptr, val_bits);
            }
            if attr_name == "__dict__" {
                crate::object::field_storage::replace_dictionary(_py, obj_ptr, Some(val_bits));
                return MoltObject::none().bits();
            }
            if let Some(&index) = (*desc_ptr).field_name_to_index.get(attr_name) {
                return crate::object::accessors::object_field_set_ptr_raw(
                    _py,
                    obj_ptr,
                    index * std::mem::size_of::<u64>(),
                    val_bits,
                );
            }
            if !(*desc_ptr).allows_dict {
                let name = &(*desc_ptr).name;
                let type_label = if name.is_empty() {
                    "dataclass"
                } else {
                    name.as_str()
                };
                return attr_error_with_obj(
                    _py,
                    type_label,
                    attr_name,
                    MoltObject::from_ptr(obj_ptr).bits(),
                );
            }
        }
        let Some(dict_bits) = crate::object::field_storage::materialize(_py, obj_ptr) else {
            return MoltObject::none().bits();
        };
        inc_ref_bits(_py, dict_bits);
        dict_set_in_place(
            _py,
            obj_from_bits(dict_bits).as_ptr().unwrap(),
            attr_bits,
            val_bits,
        );
        dec_ref_bits(_py, dict_bits);
        MoltObject::none().bits()
    }
}

#[allow(dead_code)]
pub(crate) unsafe fn dataclass_setattr_raw(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    attr_name: &str,
    val_bits: u64,
) -> u64 {
    unsafe { dataclass_setattr_inner(_py, obj_ptr, attr_bits, attr_name, val_bits, true) }
}

pub(crate) unsafe fn dataclass_setattr_raw_unchecked(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    attr_name: &str,
    val_bits: u64,
) -> u64 {
    unsafe { dataclass_setattr_inner(_py, obj_ptr, attr_bits, attr_name, val_bits, false) }
}

pub(crate) unsafe fn object_delattr_raw(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    attr_name: &str,
) -> u64 {
    unsafe {
        if let Some(result) = readonly_descriptor_metadata(_py, obj_ptr, attr_name) {
            return result;
        }
        let obj_bits = MoltObject::from_ptr(obj_ptr).bits();
        let _header = header_from_obj_ptr(obj_ptr);
        if object_type_id(obj_ptr) == TYPE_ID_OBJECT && crate::object::object_poll_fn(obj_ptr) != 0
        {
            return attr_error_with_obj(
                _py,
                class_name_for_error(object_class_bits(obj_ptr)),
                attr_name,
                obj_bits,
            );
        }
        let payload = object_payload_size(obj_ptr);
        if payload < std::mem::size_of::<u64>() {
            return attr_error_with_obj(
                _py,
                class_name_for_error(object_class_bits(obj_ptr)),
                attr_name,
                obj_bits,
            );
        }
        let class_bits = object_class_bits(obj_ptr);
        if class_bits != 0
            && let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
            && object_type_id(class_ptr) == TYPE_ID_TYPE
            && let Some(desc_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)
            && let Some(result) = apply_descriptor_mutation(
                _py,
                desc_bits,
                instance_bits_for_call(obj_ptr),
                DescriptorMutation::Delete,
            )
        {
            return result;
        }
        if attr_name == "__dict__" {
            crate::object::field_storage::replace_dictionary(_py, obj_ptr, None);
            return MoltObject::none().bits();
        }
        if class_bits != 0
            && let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
            && object_type_id(class_ptr) == TYPE_ID_TYPE
            && let Some(offset) = class_field_offset(_py, class_ptr, attr_bits)
        {
            if !crate::object::accessors::object_field_delete_ptr_raw(_py, obj_ptr, offset) {
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                return attr_error(_py, class_name_for_error(class_bits), attr_name);
            }
            return MoltObject::none().bits();
        }
        if crate::object::accessors::instance_attribute_delete(_py, obj_ptr, attr_bits) {
            return MoltObject::none().bits();
        }
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        // Deleting a non-existent attribute. CPython appends "and no __dict__ for
        // setting new attributes" (3.13+) ONLY when the instance has no `__dict__`
        // — i.e. a `__slots__`-only class. A class that allows a `__dict__` keeps
        // the bare `'X' object has no attribute 'Y'` message on every version.
        let slots_only = class_bits != 0
            && obj_from_bits(class_bits).as_ptr().is_some_and(|class_ptr| {
                object_type_id(class_ptr) == TYPE_ID_TYPE
                    && class_slots_info(_py, class_ptr).is_some_and(|info| !info.allows_dict)
            });
        if slots_only {
            return setattr_no_attr_error_with_obj(
                _py,
                class_name_for_error(class_bits),
                attr_name,
                obj_bits,
            );
        }
        attr_error(_py, class_name_for_error(class_bits), attr_name)
    }
}

unsafe fn dataclass_delattr_inner(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    attr_name: &str,
    enforce_frozen: bool,
) -> u64 {
    unsafe {
        let desc_ptr = dataclass_desc_ptr(obj_ptr);
        if !desc_ptr.is_null() {
            let class_bits = object_class_bits(obj_ptr);
            if class_bits != 0
                && let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
                && object_type_id(class_ptr) == TYPE_ID_TYPE
                && let Some(desc_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)
                && let Some(result) = apply_descriptor_mutation(
                    _py,
                    desc_bits,
                    instance_bits_for_call(obj_ptr),
                    DescriptorMutation::Delete,
                )
            {
                return result;
            }
            if enforce_frozen && (*desc_ptr).frozen {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "cannot delete frozen dataclass field",
                );
            }
            if attr_name == "__dict__" {
                crate::object::field_storage::replace_dictionary(_py, obj_ptr, None);
                return MoltObject::none().bits();
            }
            if let Some(&index) = (*desc_ptr).field_name_to_index.get(attr_name) {
                if crate::object::accessors::object_field_delete_ptr_raw(
                    _py,
                    obj_ptr,
                    index * std::mem::size_of::<u64>(),
                ) || exception_pending(_py)
                {
                    return MoltObject::none().bits();
                }
                return attr_error_with_obj(
                    _py,
                    &(*desc_ptr).name,
                    attr_name,
                    MoltObject::from_ptr(obj_ptr).bits(),
                );
            }
            if !(*desc_ptr).allows_dict {
                let name = &(*desc_ptr).name;
                let type_label = if name.is_empty() {
                    "dataclass"
                } else {
                    name.as_str()
                };
                return attr_error_with_obj(
                    _py,
                    type_label,
                    attr_name,
                    MoltObject::from_ptr(obj_ptr).bits(),
                );
            }
        }
        if crate::object::accessors::instance_attribute_delete(_py, obj_ptr, attr_bits)
            || exception_pending(_py)
        {
            return MoltObject::none().bits();
        }
        let type_label = if !desc_ptr.is_null() {
            let name = &(*desc_ptr).name;
            if name.is_empty() {
                "dataclass"
            } else {
                name.as_str()
            }
        } else {
            "dataclass"
        };
        attr_error_with_obj(
            _py,
            type_label,
            attr_name,
            MoltObject::from_ptr(obj_ptr).bits(),
        )
    }
}

#[allow(dead_code)]
pub(crate) unsafe fn dataclass_delattr_raw(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    attr_name: &str,
) -> u64 {
    unsafe { dataclass_delattr_inner(_py, obj_ptr, attr_bits, attr_name, true) }
}

pub(crate) unsafe fn dataclass_delattr_raw_unchecked(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    attr_name: &str,
) -> u64 {
    unsafe { dataclass_delattr_inner(_py, obj_ptr, attr_bits, attr_name, false) }
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_set_attr_ptr(
    obj_ptr: *mut u8,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
    val_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            molt_set_attr_generic(obj_ptr, attr_name_ptr, attr_name_len_bits, val_bits)
        })
    }
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_del_attr_generic(
    obj_ptr: *mut u8,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let Some(attr_name_len) = usize_from_bits(attr_name_len_bits) else {
                return raise_exception::<u64>(_py, "OverflowError", "attribute name is too large");
            };
            if obj_ptr.is_null() {
                return raise_exception::<_>(_py, "AttributeError", "object has no attribute");
            }
            let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
            let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
            let Some(attr_bits) = attr_name_bits_from_bytes(_py, slice) else {
                return MoltObject::none().bits();
            };
            let res = del_attr_ptr(_py, obj_ptr, attr_bits, attr_name);
            dec_ref_bits(_py, attr_bits);
            res
        })
    }
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_del_attr_ptr(
    obj_ptr: *mut u8,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            molt_del_attr_generic(obj_ptr, attr_name_ptr, attr_name_len_bits)
        })
    }
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_set_attr_object(
    obj_bits: u64,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
    val_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let Some(attr_name_len) = usize_from_bits(attr_name_len_bits) else {
                return raise_exception::<u64>(_py, "OverflowError", "attribute name is too large");
            };
            if let Some(ptr) = maybe_ptr_from_bits(obj_bits) {
                return molt_set_attr_generic(ptr, attr_name_ptr, attr_name_len_bits, val_bits);
            }
            // Tagged non-pointer receiver (int/str/float/bool/None/...): it has no
            // `__dict__` and no slot to hold the attribute. CPython raises the
            // version-gated "no __dict__ for setting new attributes" AttributeError
            // here on the SET path (3.13+). The codegen `set_attr_generic_ptr`
            // path now routes through this entry point, so this is also where
            // `typing.final(42)` etc. land instead of the old misaligned deref.
            let obj = obj_from_bits(obj_bits);
            let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
            let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
            setattr_no_attr_error_with_obj(_py, type_name(_py, obj), attr_name, obj_bits)
        })
    }
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_del_attr_object(
    obj_bits: u64,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let Some(attr_name_len) = usize_from_bits(attr_name_len_bits) else {
                return raise_exception::<u64>(_py, "OverflowError", "attribute name is too large");
            };
            if let Some(ptr) = maybe_ptr_from_bits(obj_bits) {
                return molt_del_attr_generic(ptr, attr_name_ptr, attr_name_len_bits);
            }
            // Tagged non-pointer receiver: no `__dict__`, no slot. CPython's DEL
            // path raises the same version-gated "no __dict__ for setting new
            // attributes" AttributeError (3.13+) as the SET path.
            let obj = obj_from_bits(obj_bits);
            let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
            let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
            setattr_no_attr_error_with_obj(_py, type_name(_py, obj), attr_name, obj_bits)
        })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_attr_name(obj_bits: u64, name_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_attr_name_type_error(_py, name_bits);
        };
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_attr_name_type_error(_py, name_bits);
            }
            if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
                // Foreign (C-extension) objects are handled inside the shared
                // `molt_set_attr_generic` so every setattr entry point routes
                // them uniformly.
                let bytes = string_bytes(name_ptr);
                let len = string_len(name_ptr);
                let _ = molt_set_attr_generic(obj_ptr, bytes, len as u64, val_bits);
                return MoltObject::none().bits();
            }
        }
        let obj = obj_from_bits(obj_bits);
        let name =
            string_obj_to_owned(obj_from_bits(name_bits)).unwrap_or_else(|| "<attr>".to_string());
        let _ = attr_error(_py, type_name(_py, obj), &name);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_del_attr_name(obj_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_attr_name_type_error(_py, name_bits);
        };
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_attr_name_type_error(_py, name_bits);
            }
            let attr_name = string_obj_to_owned(obj_from_bits(name_bits))
                .unwrap_or_else(|| "<attr>".to_string());
            if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
                return del_attr_ptr(_py, obj_ptr, name_bits, &attr_name);
            }
            let obj = obj_from_bits(obj_bits);
            attr_error(_py, type_name(_py, obj), &attr_name)
        }
    })
}

#[cfg(test)]
mod function_metadata_tests {
    use super::*;
    use crate::object::layout::function_call_target_ptr;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TARGET: AtomicU64 = AtomicU64::new(0);
    static CALLS: AtomicU64 = AtomicU64::new(0);
    static VERSION: AtomicU64 = AtomicU64::new(0);
    static BINDER: AtomicU64 = AtomicU64::new(0);
    static REENTRY: AtomicU64 = AtomicU64::new(0);

    extern "C" fn echo(value: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            inc_ref_bits(py, value);
            value
        })
    }

    extern "C" fn add_pair(first: u64, second: u64) -> u64 {
        let first = obj_from_bits(first).as_int().unwrap();
        let second = obj_from_bits(second).as_int().unwrap();
        MoltObject::from_int(first + second).bits()
    }

    extern "C" fn observe_metadata_release(_self: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let target = TARGET.load(Ordering::SeqCst);
                let ptr = obj_from_bits(target).as_ptr().unwrap();
                CALLS.fetch_add(1, Ordering::SeqCst);
                VERSION.store(
                    crate::object::layout::function_mutation_version(ptr),
                    Ordering::SeqCst,
                );
                BINDER.store(
                    u64::from(crate::call::bind::function_requires_binder_flag(ptr)),
                    Ordering::SeqCst,
                );
                let result = call_callable1(py, target, MoltObject::from_int(91).bits());
                REENTRY.store(result, Ordering::SeqCst);
                dec_ref_bits(py, result);
                MoltObject::none().bits()
            }
        })
    }

    unsafe fn runtime_function(py: &PyToken<'_>, name: &str, target: *const (), arity: u64) -> u64 {
        let ptr = crate::builtins::functions::alloc_runtime_function_obj(
            py,
            crate::builtins::functions::runtime_fn_addr(name, target),
            arity,
        );
        assert!(!ptr.is_null());
        MoltObject::from_ptr(ptr).bits()
    }

    #[test]
    fn function_code_assignment_rejects_opaque_context_identity() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let opaque = runtime_function(py, "opaque_code_source", echo as *const (), 0);
                let opaque_ptr = obj_from_bits(opaque).as_ptr().unwrap();
                let context = alloc_tuple(py, &[MoltObject::from_int(7).bits()]);
                assert!(!context.is_null());
                let context_bits = MoltObject::from_ptr(context).bits();
                function_set_closure_bits(
                    py,
                    opaque_ptr,
                    context_bits,
                    FunctionCallAbi::OpaqueContextFirst,
                );
                dec_ref_bits(py, context_bits);
                let opaque_code = ensure_function_code_bits(py, opaque_ptr);

                let target = runtime_function(py, "opaque_code_target", echo as *const (), 1);
                let target_ptr = obj_from_bits(target).as_ptr().unwrap();
                let original_code = ensure_function_code_bits(py, target_ptr);
                let original_fn = function_fn_ptr(target_ptr);
                let original_trampoline = function_trampoline_ptr(target_ptr);
                let original_arity = function_arity(target_ptr);
                let original_call_target = function_call_target_ptr(target_ptr);
                let result = molt_set_attr_generic(
                    target_ptr,
                    b"__code__".as_ptr(),
                    b"__code__".len() as u64,
                    opaque_code,
                );
                assert!(obj_from_bits(result).is_none());
                assert!(exception_pending(py));
                assert_eq!(function_code_bits(target_ptr), original_code);
                assert_eq!(function_fn_ptr(target_ptr), original_fn);
                assert_eq!(function_trampoline_ptr(target_ptr), original_trampoline);
                assert_eq!(function_arity(target_ptr), original_arity);
                assert_eq!(function_call_target_ptr(target_ptr), original_call_target);
                clear_exception(py);

                dec_ref_bits(py, target);
                dec_ref_bits(py, opaque);
            }
        });
    }

    #[test]
    fn function_code_assignment_retargets_callable_and_projects_code_signature() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let replacement =
                    runtime_function(py, "replacement_pair", add_pair as *const (), 2);
                let replacement_ptr = obj_from_bits(replacement).as_ptr().unwrap();
                let first_name = alloc_string(py, b"first");
                let second_name = alloc_string(py, b"second");
                let empty = alloc_tuple(py, &[]);
                assert!(!first_name.is_null() && !second_name.is_null() && !empty.is_null());
                let first_name_bits = MoltObject::from_ptr(first_name).bits();
                let second_name_bits = MoltObject::from_ptr(second_name).bits();
                let empty_bits = MoltObject::from_ptr(empty).bits();
                let replacement_names = alloc_tuple(py, &[first_name_bits, second_name_bits]);
                assert!(!replacement_names.is_null());
                let replacement_names_bits = MoltObject::from_ptr(replacement_names).bits();
                assert!(crate::call::class_init::function_set_attr_name(
                    py,
                    replacement_ptr,
                    b"__molt_arg_names__",
                    replacement_names_bits,
                ));
                assert!(crate::call::class_init::function_set_attr_name(
                    py,
                    replacement_ptr,
                    b"__molt_kwonly_names__",
                    empty_bits,
                ));
                let replacement_code = ensure_function_code_bits(py, replacement_ptr);

                let target = runtime_function(py, "replacement_owner", echo as *const (), 1);
                let target_ptr = obj_from_bits(target).as_ptr().unwrap();
                let original_name = alloc_string(py, b"value");
                assert!(!original_name.is_null());
                let original_name_bits = MoltObject::from_ptr(original_name).bits();
                let original_names = alloc_tuple(py, &[original_name_bits]);
                let defaults = alloc_tuple(py, &[MoltObject::from_int(97).bits()]);
                assert!(!original_names.is_null() && !defaults.is_null());
                let original_names_bits = MoltObject::from_ptr(original_names).bits();
                let defaults_bits = MoltObject::from_ptr(defaults).bits();
                assert!(crate::call::class_init::function_set_attr_name(
                    py,
                    target_ptr,
                    b"__molt_arg_names__",
                    original_names_bits,
                ));
                assert!(crate::call::class_init::function_set_attr_name(
                    py,
                    target_ptr,
                    b"__molt_kwonly_names__",
                    empty_bits,
                ));
                assert!(crate::call::class_init::function_set_attr_name(
                    py,
                    target_ptr,
                    b"__defaults__",
                    defaults_bits,
                ));

                let result = molt_set_attr_generic(
                    target_ptr,
                    b"__code__".as_ptr(),
                    b"__code__".len() as u64,
                    replacement_code,
                );
                assert!(obj_from_bits(result).is_none());
                assert!(!exception_pending(py));
                assert_eq!(function_code_bits(target_ptr), replacement_code);
                assert_eq!(
                    function_fn_ptr(target_ptr),
                    function_fn_ptr(replacement_ptr)
                );
                assert_eq!(function_arity(target_ptr), 2);
                assert_eq!(
                    crate::call::function::function_metadata_bits(
                        py,
                        target_ptr,
                        b"__molt_arg_names__",
                    ),
                    replacement_names_bits
                );
                assert_eq!(
                    crate::call::function::function_metadata_bits(py, target_ptr, b"__defaults__"),
                    defaults_bits
                );

                let called = call_callable1(py, target, MoltObject::from_int(101).bits());
                assert_eq!(obj_from_bits(called).as_int(), Some(198));
                dec_ref_bits(py, called);

                dec_ref_bits(py, target);
                dec_ref_bits(py, replacement);
                for bits in [
                    defaults_bits,
                    original_names_bits,
                    original_name_bits,
                    replacement_names_bits,
                    empty_bits,
                    second_name_bits,
                    first_name_bits,
                ] {
                    dec_ref_bits(py, bits);
                }
            }
        });
    }

    #[test]
    fn function_metadata_set_and_delete_commit_before_finalizer_reentry() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let target = runtime_function(py, "metadata_reentry_echo", echo as *const (), 1);
                let target_ptr = obj_from_bits(target).as_ptr().unwrap();
                TARGET.store(target, Ordering::SeqCst);
                let name = attr_name_bits_from_bytes(py, b"MetadataFinalizerProbe").unwrap();
                let class = crate::molt_class_new(name);
                crate::molt_class_set_base(class, builtin_classes(py).object);
                let class_ptr = obj_from_bits(class).as_ptr().unwrap();
                crate::object::class_finish_definition(py, class_ptr).unwrap();
                let finalizer = runtime_function(
                    py,
                    "metadata_release_observer",
                    observe_metadata_release as *const (),
                    1,
                );
                let del_name = attr_name_bits_from_bytes(py, b"__del__").unwrap();
                molt_set_attr_name(class, del_name, finalizer);
                assert!(!exception_pending(py));

                let mut version = 0;
                let mut calls = 0;
                CALLS.store(0, Ordering::SeqCst);
                for key in [
                    b"__defaults__".as_slice(),
                    b"__kwdefaults__",
                    b"__molt_vararg__",
                    b"__molt_is_generator__",
                ] {
                    let key_bits = attr_name_bits_from_bytes(py, key).unwrap();
                    for delete in [false, true] {
                        let victim = crate::alloc_instance_for_class(py, class_ptr);
                        let value = if key == b"__defaults__" {
                            let tuple = alloc_tuple(py, &[victim]);
                            assert!(!tuple.is_null());
                            dec_ref_bits(py, victim);
                            MoltObject::from_ptr(tuple).bits()
                        } else if key == b"__kwdefaults__" {
                            let dict = alloc_dict_with_pairs(py, &[name, victim]);
                            assert!(!dict.is_null());
                            dec_ref_bits(py, victim);
                            MoltObject::from_ptr(dict).bits()
                        } else {
                            victim
                        };
                        assert!(crate::call::class_init::function_set_attr_bits(
                            py, target_ptr, key_bits, value
                        ));
                        dec_ref_bits(py, value);
                        assert_eq!(CALLS.load(Ordering::SeqCst), calls);
                        if delete {
                            molt_del_attr_name(target, key_bits);
                        } else {
                            molt_set_attr_name(target, key_bits, MoltObject::none().bits());
                        }
                        assert!(!exception_pending(py));
                        calls += 1;
                        if matches!(
                            key,
                            b"__defaults__" | b"__kwdefaults__" | b"__molt_vararg__"
                        ) {
                            version += 1;
                        }
                        assert_eq!(CALLS.load(Ordering::SeqCst), calls);
                        assert_eq!(VERSION.load(Ordering::SeqCst), version);
                        assert_eq!(BINDER.load(Ordering::SeqCst), 0);
                        assert_eq!(
                            REENTRY.load(Ordering::SeqCst),
                            MoltObject::from_int(91).bits()
                        );
                    }
                    dec_ref_bits(py, key_bits);
                }
                TARGET.store(0, Ordering::SeqCst);
                for bits in [del_name, finalizer, class, name, target] {
                    dec_ref_bits(py, bits);
                }
            }
        });
    }
}
