use super::*;

fn property_doc_set(_py: &PyToken<'_>, prop_ptr: *mut u8, val_bits: u64) {
    let mut guard = property_docs(_py).lock().unwrap();
    let key = PtrSlot(prop_ptr);
    if obj_from_bits(val_bits).is_none() {
        if let Some(old_bits) = guard.remove(&key) {
            dec_ref_bits(_py, old_bits);
        }
        return;
    }
    inc_ref_bits(_py, val_bits);
    if let Some(old_bits) = guard.insert(key, val_bits) {
        dec_ref_bits(_py, old_bits);
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
) -> i64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let attr_name_len = usize_from_bits(attr_name_len_bits);
            if obj_ptr.is_null() {
                return raise_exception::<_>(_py, "AttributeError", "object has no attribute");
            }
            let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
            let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
            let type_id = object_type_id(obj_ptr);
            if type_id == TYPE_ID_MODULE {
                let Some(attr_bits) = attr_name_bits_from_bytes(_py, slice) else {
                    return MoltObject::none().bits() as i64;
                };
                let module_bits = MoltObject::from_ptr(obj_ptr).bits();
                let res = molt_module_set_attr(module_bits, attr_bits, val_bits);
                dec_ref_bits(_py, attr_bits);
                return res as i64;
            }
            if type_id == TYPE_ID_PROPERTY {
                if attr_name == "__doc__" {
                    property_doc_set(_py, obj_ptr, val_bits);
                    return MoltObject::none().bits() as i64;
                }
                return attr_error_with_obj(
                    _py,
                    "property",
                    attr_name,
                    MoltObject::from_ptr(obj_ptr).bits(),
                );
            }
            if type_id == TYPE_ID_TYPE {
                let class_bits = MoltObject::from_ptr(obj_ptr).bits();
                if is_builtin_class_bits(_py, class_bits) {
                    // CPython: setting an attribute on an immutable builtin type
                    // raises `cannot set '<attr>' attribute of immutable type
                    // '<type>'` (version-stable across 3.12/3.13/3.14).
                    let class_label = class_name_for_error(class_bits);
                    let msg = format!(
                        "cannot set '{attr_name}' attribute of immutable type '{class_label}'"
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
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
                    return MoltObject::none().bits() as i64;
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
                    return MoltObject::none().bits() as i64;
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
                    return MoltObject::none().bits() as i64;
                }
                let Some(attr_bits) = attr_name_bits_from_bytes(_py, slice) else {
                    return MoltObject::none().bits() as i64;
                };
                let dict_bits = class_dict_bits(obj_ptr);
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                    && object_type_id(dict_ptr) == TYPE_ID_DICT
                {
                    dict_set_in_place(_py, dict_ptr, attr_bits, val_bits);
                    if attr_name == "__del__" {
                        crate::object::class_refresh_finalizer_flag(_py, obj_ptr);
                    }
                    class_bump_layout_version(obj_ptr);
                    dec_ref_bits(_py, attr_bits);
                    return MoltObject::none().bits() as i64;
                }
                dec_ref_bits(_py, attr_bits);
                return attr_error(_py, "type", attr_name);
            }
            if type_id == TYPE_ID_EXCEPTION {
                let Some(attr_bits) = attr_name_bits_from_bytes(_py, slice) else {
                    return MoltObject::none().bits() as i64;
                };
                let name = string_obj_to_owned(obj_from_bits(attr_bits)).unwrap_or_default();
                if name == "name" || name == "obj" {
                    let kind_bits = exception_kind_bits(obj_ptr);
                    let mut is_attrerr = false;
                    if let Some(kind_ptr) = obj_from_bits(kind_bits).as_ptr()
                        && object_type_id(kind_ptr) == TYPE_ID_STRING
                    {
                        let kind_len = string_len(kind_ptr);
                        let kind_bytes =
                            std::slice::from_raw_parts(string_bytes(kind_ptr), kind_len);
                        is_attrerr = kind_bytes == b"AttributeError";
                    }
                    if is_attrerr {
                        let members_bits = exception_value_bits(obj_ptr);
                        let (old_name_bits, old_obj_bits) =
                            if let Some(members_ptr) = obj_from_bits(members_bits).as_ptr() {
                                if object_type_id(members_ptr) == TYPE_ID_TUPLE {
                                    let elems = seq_vec_ref(members_ptr);
                                    (
                                        elems
                                            .first()
                                            .copied()
                                            .unwrap_or_else(|| MoltObject::none().bits()),
                                        elems
                                            .get(1)
                                            .copied()
                                            .unwrap_or_else(|| MoltObject::none().bits()),
                                    )
                                } else {
                                    (MoltObject::none().bits(), MoltObject::none().bits())
                                }
                            } else {
                                (MoltObject::none().bits(), MoltObject::none().bits())
                            };
                        let new_name_bits = if name == "name" {
                            val_bits
                        } else {
                            old_name_bits
                        };
                        let new_obj_bits = if name == "obj" {
                            val_bits
                        } else {
                            old_obj_bits
                        };
                        let tuple_ptr = alloc_tuple(_py, &[new_name_bits, new_obj_bits]);
                        if tuple_ptr.is_null() {
                            dec_ref_bits(_py, attr_bits);
                            return MoltObject::none().bits() as i64;
                        }
                        let tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
                        let slot = obj_ptr.add(6 * std::mem::size_of::<u64>()) as *mut u64;
                        let old_bits = *slot;
                        if old_bits != tuple_bits {
                            dec_ref_bits(_py, old_bits);
                            inc_ref_bits(_py, tuple_bits);
                            *slot = tuple_bits;
                        }
                        dec_ref_bits(_py, tuple_bits);
                        dec_ref_bits(_py, attr_bits);
                        return MoltObject::none().bits() as i64;
                    }
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
                    let slot = if name == "__cause__" {
                        obj_ptr.add(2 * std::mem::size_of::<u64>())
                    } else {
                        obj_ptr.add(3 * std::mem::size_of::<u64>())
                    } as *mut u64;
                    let old_bits = *slot;
                    if old_bits != val_bits {
                        dec_ref_bits(_py, old_bits);
                        inc_ref_bits(_py, val_bits);
                        *slot = val_bits;
                    }
                    if name == "__cause__" {
                        let suppress_bits = MoltObject::from_bool(true).bits();
                        let suppress_slot = obj_ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64;
                        let old_bits = *suppress_slot;
                        if old_bits != suppress_bits {
                            dec_ref_bits(_py, old_bits);
                            inc_ref_bits(_py, suppress_bits);
                            *suppress_slot = suppress_bits;
                        }
                    }
                    dec_ref_bits(_py, attr_bits);
                    return MoltObject::none().bits() as i64;
                }
                if name == "args" {
                    let args_bits = exception_args_from_iterable(_py, val_bits);
                    if obj_from_bits(args_bits).is_none() {
                        dec_ref_bits(_py, attr_bits);
                        return MoltObject::none().bits() as i64;
                    }
                    let class_bits = exception_class_bits(obj_ptr);
                    let kind_bits = exception_kind_bits(obj_ptr);
                    let msg_bits =
                        crate::exception_message_for_storage(_py, kind_bits, class_bits, args_bits);
                    if obj_from_bits(msg_bits).is_none() {
                        dec_ref_bits(_py, args_bits);
                        dec_ref_bits(_py, attr_bits);
                        return MoltObject::none().bits() as i64;
                    }
                    exception_store_args_and_message(_py, obj_ptr, args_bits, msg_bits);
                    dec_ref_bits(_py, attr_bits);
                    return MoltObject::none().bits() as i64;
                }
                if name == "__suppress_context__" {
                    let suppress = is_truthy(_py, obj_from_bits(val_bits));
                    let suppress_bits = MoltObject::from_bool(suppress).bits();
                    let slot = obj_ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64;
                    let old_bits = *slot;
                    if old_bits != suppress_bits {
                        dec_ref_bits(_py, old_bits);
                        inc_ref_bits(_py, suppress_bits);
                        *slot = suppress_bits;
                    }
                    dec_ref_bits(_py, attr_bits);
                    return MoltObject::none().bits() as i64;
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
                    let slot = obj_ptr.add(9 * std::mem::size_of::<u64>()) as *mut u64;
                    let old_bits = *slot;
                    if old_bits != val_bits {
                        dec_ref_bits(_py, old_bits);
                        inc_ref_bits(_py, val_bits);
                        *slot = val_bits;
                    }
                    dec_ref_bits(_py, attr_bits);
                    return MoltObject::none().bits() as i64;
                }
                if name == "value" {
                    let kind = string_obj_to_owned(obj_from_bits(exception_kind_bits(obj_ptr)))
                        .unwrap_or_default();
                    if kind != "StopIteration" {
                        dec_ref_bits(_py, attr_bits);
                        return attr_error(_py, "exception", attr_name);
                    }
                    let slot = obj_ptr.add(6 * std::mem::size_of::<u64>()) as *mut u64;
                    let old_bits = *slot;
                    if old_bits != val_bits {
                        dec_ref_bits(_py, old_bits);
                        inc_ref_bits(_py, val_bits);
                        *slot = val_bits;
                    }
                    dec_ref_bits(_py, attr_bits);
                    return MoltObject::none().bits() as i64;
                }
                let mut dict_bits = exception_dict_bits(obj_ptr);
                if obj_from_bits(dict_bits).is_none() || dict_bits == 0 {
                    let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                    if !dict_ptr.is_null() {
                        dict_bits = MoltObject::from_ptr(dict_ptr).bits();
                        let slot = obj_ptr.add(9 * std::mem::size_of::<u64>()) as *mut u64;
                        let old_bits = *slot;
                        if old_bits != dict_bits {
                            dec_ref_bits(_py, old_bits);
                            *slot = dict_bits;
                        }
                    }
                }
                if !obj_from_bits(dict_bits).is_none()
                    && dict_bits != 0
                    && let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                    && object_type_id(dict_ptr) == TYPE_ID_DICT
                {
                    dict_set_in_place(_py, dict_ptr, attr_bits, val_bits);
                    dec_ref_bits(_py, attr_bits);
                    return MoltObject::none().bits() as i64;
                }
                dec_ref_bits(_py, attr_bits);
                return attr_error(_py, "exception", attr_name);
            }
            if type_id == TYPE_ID_FUNCTION {
                if attr_name == "__code__" {
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
                    function_set_code_bits(_py, obj_ptr, val_bits);
                    return MoltObject::none().bits() as i64;
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
                    return MoltObject::none().bits() as i64;
                }
                if attr_name == "__annotations__" {
                    let val_obj = obj_from_bits(val_bits);
                    let ann_bits = if val_obj.is_none() {
                        let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                        if dict_ptr.is_null() {
                            return MoltObject::none().bits() as i64;
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
                    return MoltObject::none().bits() as i64;
                }
                let Some(attr_bits) = attr_name_bits_from_bytes(_py, slice) else {
                    return MoltObject::none().bits() as i64;
                };
                let mut dict_bits = function_dict_bits(obj_ptr);
                if dict_bits == 0 {
                    let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                    if dict_ptr.is_null() {
                        dec_ref_bits(_py, attr_bits);
                        return MoltObject::none().bits() as i64;
                    }
                    dict_bits = MoltObject::from_ptr(dict_ptr).bits();
                    function_set_dict_bits(obj_ptr, dict_bits);
                }
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                    && object_type_id(dict_ptr) == TYPE_ID_DICT
                {
                    dict_set_in_place(_py, dict_ptr, attr_bits, val_bits);
                    if is_task_trampoline_attr_name(attr_name) {
                        refresh_function_task_trampoline_cache(_py, obj_ptr);
                    }
                    // Reassigning `__defaults__`/`__kwdefaults__` invalidates any
                    // compile-time-baked literal default the devirtualizer may
                    // have emitted at a direct call site: bump the function's
                    // defaults version so the guarded fast path deopts to a live
                    // read (CPython binds defaults at call time). This is the
                    // ONLY user-reachable mutation entry point — function
                    // CREATION sets these via `function_set_attr_bits`, which
                    // does not bump, keeping a fresh function at version 0.
                    if attr_name == "__defaults__" || attr_name == "__kwdefaults__" {
                        function_bump_defaults_version(obj_ptr);
                    }
                    if matches!(
                        attr_name,
                        "__molt_bind_kind__"
                            | "__molt_vararg__"
                            | "__molt_varkw__"
                            | "__molt_kwonly_names__"
                            | "__defaults__"
                            | "__kwdefaults__"
                    ) {
                        crate::call::bind::refresh_function_requires_binder_flag(_py, obj_ptr);
                    }
                    dec_ref_bits(_py, attr_bits);
                    return MoltObject::none().bits() as i64;
                }
                dec_ref_bits(_py, attr_bits);
                return attr_error(_py, "function", attr_name);
            }
            if type_id == TYPE_ID_CODE {
                return attr_error(_py, "code", attr_name);
            }
            if type_id == TYPE_ID_DATACLASS {
                let desc_ptr = dataclass_desc_ptr(obj_ptr);
                let Some(attr_bits) = attr_name_bits_from_bytes(_py, slice) else {
                    return MoltObject::none().bits() as i64;
                };
                if !desc_ptr.is_null() {
                    let class_bits = (*desc_ptr).class_bits;
                    if class_bits != 0
                        && let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
                        && object_type_id(class_ptr) == TYPE_ID_TYPE
                    {
                        let setattr_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.setattr_name,
                            b"__setattr__",
                        );
                        if let Some(call_bits) = class_attr_lookup(
                            _py,
                            class_ptr,
                            class_ptr,
                            Some(obj_ptr),
                            setattr_bits,
                        ) {
                            let _ = call_callable2(_py, call_bits, attr_bits, val_bits);
                            dec_ref_bits(_py, attr_bits);
                            return MoltObject::none().bits() as i64;
                        }
                        if let Some(desc_bits) =
                            class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)
                            && descriptor_is_data(_py, desc_bits)
                        {
                            let desc_obj = obj_from_bits(desc_bits);
                            if let Some(desc_ptr) = desc_obj.as_ptr()
                                && object_type_id(desc_ptr) == TYPE_ID_PROPERTY
                            {
                                let set_bits = property_set_bits(desc_ptr);
                                if obj_from_bits(set_bits).is_none() {
                                    dec_ref_bits(_py, attr_bits);
                                    return property_no_setter(
                                        _py,
                                        attr_name,
                                        class_ptr,
                                        MoltObject::from_ptr(obj_ptr).bits(),
                                    );
                                }
                                let inst_bits = instance_bits_for_call(obj_ptr);
                                let _ = call_function_obj2(_py, set_bits, inst_bits, val_bits);
                                dec_ref_bits(_py, attr_bits);
                                return MoltObject::none().bits() as i64;
                            }
                            let set_bits = intern_static_name(
                                _py,
                                &runtime_state(_py).interned.set_name,
                                b"__set__",
                            );
                            if let Some(method_bits) =
                                descriptor_method_bits(_py, desc_bits, set_bits)
                            {
                                let self_bits = desc_bits;
                                let inst_bits = instance_bits_for_call(obj_ptr);
                                let method_obj = obj_from_bits(method_bits);
                                if let Some(method_ptr) = method_obj.as_ptr() {
                                    if object_type_id(method_ptr) == TYPE_ID_FUNCTION {
                                        let _ = call_function_obj3(
                                            _py,
                                            method_bits,
                                            self_bits,
                                            inst_bits,
                                            val_bits,
                                        );
                                    } else {
                                        let _ =
                                            call_callable2(_py, method_bits, inst_bits, val_bits);
                                    }
                                } else {
                                    let _ = call_callable2(_py, method_bits, inst_bits, val_bits);
                                }
                                dec_ref_bits(_py, attr_bits);
                                return MoltObject::none().bits() as i64;
                            }
                            dec_ref_bits(_py, attr_bits);
                            return descriptor_no_setter(
                                _py,
                                attr_name,
                                class_ptr,
                                MoltObject::from_ptr(obj_ptr).bits(),
                            );
                        }
                    }
                    if !(*desc_ptr).allows_dict {
                        dec_ref_bits(_py, attr_bits);
                        let name = &(*desc_ptr).name;
                        let type_label = if name.is_empty() {
                            "dataclass"
                        } else {
                            name.as_str()
                        };
                        // `@dataclass(slots=True)` instance rejecting a non-slot
                        // attribute: version-gated no-`__dict__` SET message (3.13+).
                        return setattr_no_attr_error_with_obj(
                            _py,
                            type_label,
                            attr_name,
                            MoltObject::from_ptr(obj_ptr).bits(),
                        );
                    }
                }
                let mut dict_bits = dataclass_dict_bits(obj_ptr);
                if dict_bits == 0 {
                    let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                    if dict_ptr.is_null() {
                        dec_ref_bits(_py, attr_bits);
                        return MoltObject::none().bits() as i64;
                    }
                    dict_bits = MoltObject::from_ptr(dict_ptr).bits();
                    dataclass_set_dict_bits(_py, obj_ptr, dict_bits);
                }
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                    && object_type_id(dict_ptr) == TYPE_ID_DICT
                {
                    dict_set_in_place(_py, dict_ptr, attr_bits, val_bits);
                    dec_ref_bits(_py, attr_bits);
                    return MoltObject::none().bits() as i64;
                }
                dec_ref_bits(_py, attr_bits);
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
                return setattr_no_attr_error_with_obj(
                    _py,
                    type_label,
                    attr_name,
                    MoltObject::from_ptr(obj_ptr).bits(),
                );
            }
            if type_id == TYPE_ID_OBJECT {
                let _header = header_from_obj_ptr(obj_ptr);
                if crate::object::object_poll_fn(obj_ptr) != 0 {
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
                    return MoltObject::none().bits() as i64;
                };
                let class_bits = object_class_bits(obj_ptr);
                let mut slots_info = None;
                if class_bits != 0
                    && let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
                    && object_type_id(class_ptr) == TYPE_ID_TYPE
                {
                    slots_info = class_slots_info(_py, class_ptr, attr_bits);
                    let setattr_bits = intern_static_name(
                        _py,
                        &runtime_state(_py).interned.setattr_name,
                        b"__setattr__",
                    );
                    let mut use_custom_setattr = false;
                    if let Some(raw_bits) = class_attr_lookup_raw_mro(_py, class_ptr, setattr_bits)
                    {
                        if let Some(default_bits) = object_method_bits(_py, "__setattr__") {
                            if !obj_eq(_py, obj_from_bits(raw_bits), obj_from_bits(default_bits)) {
                                use_custom_setattr = true;
                            }
                        } else {
                            use_custom_setattr = true;
                        }
                    }
                    if use_custom_setattr
                        && let Some(call_bits) = class_attr_lookup(
                            _py,
                            class_ptr,
                            class_ptr,
                            Some(obj_ptr),
                            setattr_bits,
                        )
                    {
                        let _ = call_callable2(_py, call_bits, attr_bits, val_bits);
                        dec_ref_bits(_py, attr_bits);
                        return MoltObject::none().bits() as i64;
                    }
                    if let Some(offset) = class_own_slot_field_offset(_py, class_ptr, attr_bits) {
                        let res = object_field_set_ptr_raw(_py, obj_ptr, offset, val_bits);
                        dec_ref_bits(_py, attr_bits);
                        return res as i64;
                    }
                    if let Some(desc_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)
                        && descriptor_is_data(_py, desc_bits)
                    {
                        let desc_obj = obj_from_bits(desc_bits);
                        if let Some(desc_ptr) = desc_obj.as_ptr()
                            && object_type_id(desc_ptr) == TYPE_ID_PROPERTY
                        {
                            let set_bits = property_set_bits(desc_ptr);
                            if obj_from_bits(set_bits).is_none() {
                                dec_ref_bits(_py, attr_bits);
                                return property_no_setter(
                                    _py,
                                    attr_name,
                                    class_ptr,
                                    MoltObject::from_ptr(obj_ptr).bits(),
                                );
                            }
                            let inst_bits = instance_bits_for_call(obj_ptr);
                            let _ = call_function_obj2(_py, set_bits, inst_bits, val_bits);
                            dec_ref_bits(_py, attr_bits);
                            return MoltObject::none().bits() as i64;
                        }
                        let set_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.set_name,
                            b"__set__",
                        );
                        if let Some(method_bits) = descriptor_method_bits(_py, desc_bits, set_bits)
                        {
                            let self_bits = desc_bits;
                            let inst_bits = instance_bits_for_call(obj_ptr);
                            let method_obj = obj_from_bits(method_bits);
                            if let Some(method_ptr) = method_obj.as_ptr() {
                                if object_type_id(method_ptr) == TYPE_ID_FUNCTION {
                                    let _ = call_function_obj3(
                                        _py,
                                        method_bits,
                                        self_bits,
                                        inst_bits,
                                        val_bits,
                                    );
                                } else {
                                    let _ = call_callable2(_py, method_bits, inst_bits, val_bits);
                                }
                            } else {
                                let _ = call_callable2(_py, method_bits, inst_bits, val_bits);
                            }
                            dec_ref_bits(_py, attr_bits);
                            return MoltObject::none().bits() as i64;
                        }
                        dec_ref_bits(_py, attr_bits);
                        return descriptor_no_setter(
                            _py,
                            attr_name,
                            class_ptr,
                            MoltObject::from_ptr(obj_ptr).bits(),
                        );
                    }
                    if let Some(offset) = class_field_offset(_py, class_ptr, attr_bits) {
                        object_field_set_ptr_raw(_py, obj_ptr, offset, val_bits);
                        dec_ref_bits(_py, attr_bits);
                        return MoltObject::none().bits() as i64;
                    }
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
                let mut dict_bits = instance_dict_bits(obj_ptr);
                if dict_bits == 0 {
                    let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                    if dict_ptr.is_null() {
                        dec_ref_bits(_py, attr_bits);
                        return MoltObject::none().bits() as i64;
                    }
                    dict_bits = MoltObject::from_ptr(dict_ptr).bits();
                    instance_set_dict_bits(_py, obj_ptr, dict_bits);
                }
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                    && object_type_id(dict_ptr) == TYPE_ID_DICT
                {
                    dict_set_in_place(_py, dict_ptr, attr_bits, val_bits);
                    dec_ref_bits(_py, attr_bits);
                    return MoltObject::none().bits() as i64;
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
) -> i64 {
    unsafe {
        let type_id = object_type_id(obj_ptr);
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
                        return MoltObject::none().bits() as i64;
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
                    return MoltObject::none().bits() as i64;
                }
            }
            let module_name =
                string_obj_to_owned(obj_from_bits(module_name_bits(obj_ptr))).unwrap_or_default();
            let msg = format!("module '{module_name}' has no attribute '{attr_name}'");
            return attr_error_with_message(_py, &msg);
        }
        if type_id == TYPE_ID_TYPE {
            let class_bits = MoltObject::from_ptr(obj_ptr).bits();
            if is_builtin_class_bits(_py, class_bits) {
                // CPython routes `del <builtin_type>.<attr>` through the same
                // immutable-type guard as set, yielding `cannot set '<attr>'
                // attribute of immutable type '<type>'` (version-stable).
                let class_label = class_name_for_error(class_bits);
                let msg =
                    format!("cannot set '{attr_name}' attribute of immutable type '{class_label}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
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
                    return MoltObject::none().bits() as i64;
                }
                let class_name = string_obj_to_owned(obj_from_bits(class_name_bits(obj_ptr)))
                    .unwrap_or_default();
                let msg = format!("type object '{class_name}' has no attribute '{attr_name}'");
                return raise_exception::<_>(_py, "AttributeError", &msg);
            }
            let dict_bits = class_dict_bits(obj_ptr);
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                && object_type_id(dict_ptr) == TYPE_ID_DICT
                && dict_del_in_place(_py, dict_ptr, attr_bits)
            {
                if attr_name == "__del__" {
                    crate::object::class_refresh_finalizer_flag(_py, obj_ptr);
                }
                class_bump_layout_version(obj_ptr);
                return MoltObject::none().bits() as i64;
            }
            let class_name =
                string_obj_to_owned(obj_from_bits(class_name_bits(obj_ptr))).unwrap_or_default();
            let msg = format!("type object '{class_name}' has no attribute '{attr_name}'");
            return attr_error_with_message(_py, &msg);
        }
        if type_id == TYPE_ID_EXCEPTION {
            if attr_name == "__cause__" || attr_name == "__context__" {
                let slot = if attr_name == "__cause__" {
                    obj_ptr.add(2 * std::mem::size_of::<u64>())
                } else {
                    obj_ptr.add(3 * std::mem::size_of::<u64>())
                } as *mut u64;
                let old_bits = *slot;
                if !obj_from_bits(old_bits).is_none() {
                    dec_ref_bits(_py, old_bits);
                    let none_bits = MoltObject::none().bits();
                    inc_ref_bits(_py, none_bits);
                    *slot = none_bits;
                }
                if attr_name == "__cause__" {
                    let suppress_bits = MoltObject::from_bool(false).bits();
                    let suppress_slot = obj_ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64;
                    let old_bits = *suppress_slot;
                    if old_bits != suppress_bits {
                        dec_ref_bits(_py, old_bits);
                        inc_ref_bits(_py, suppress_bits);
                        *suppress_slot = suppress_bits;
                    }
                }
                return MoltObject::none().bits() as i64;
            }
            if attr_name == "__suppress_context__" {
                let suppress_bits = MoltObject::from_bool(false).bits();
                let slot = obj_ptr.add(4 * std::mem::size_of::<u64>()) as *mut u64;
                let old_bits = *slot;
                if old_bits != suppress_bits {
                    dec_ref_bits(_py, old_bits);
                    inc_ref_bits(_py, suppress_bits);
                    *slot = suppress_bits;
                }
                return MoltObject::none().bits() as i64;
            }
            let dict_bits = exception_dict_bits(obj_ptr);
            if !obj_from_bits(dict_bits).is_none()
                && dict_bits != 0
                && let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                && object_type_id(dict_ptr) == TYPE_ID_DICT
                && dict_del_in_place(_py, dict_ptr, attr_bits)
            {
                return MoltObject::none().bits() as i64;
            }
            return attr_error(_py, "exception", attr_name);
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
                return MoltObject::none().bits() as i64;
            }
            let dict_bits = function_dict_bits(obj_ptr);
            if dict_bits != 0
                && let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                && object_type_id(dict_ptr) == TYPE_ID_DICT
                && dict_del_in_place(_py, dict_ptr, attr_bits)
            {
                if is_task_trampoline_attr_name(attr_name) {
                    refresh_function_task_trampoline_cache(_py, obj_ptr);
                }
                return MoltObject::none().bits() as i64;
            }
            return attr_error(_py, "function", attr_name);
        }
        if type_id == TYPE_ID_DATACLASS {
            let desc_ptr = dataclass_desc_ptr(obj_ptr);
            if !desc_ptr.is_null() {
                let class_bits = (*desc_ptr).class_bits;
                if class_bits != 0
                    && let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
                    && object_type_id(class_ptr) == TYPE_ID_TYPE
                {
                    let delattr_bits = intern_static_name(
                        _py,
                        &runtime_state(_py).interned.delattr_name,
                        b"__delattr__",
                    );
                    if let Some(call_bits) =
                        class_attr_lookup(_py, class_ptr, class_ptr, Some(obj_ptr), delattr_bits)
                    {
                        let _ = call_callable1(_py, call_bits, attr_bits);
                        return MoltObject::none().bits() as i64;
                    }
                    if let Some(desc_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits) {
                        if let Some(desc_ptr) = maybe_ptr_from_bits(desc_bits)
                            && object_type_id(desc_ptr) == TYPE_ID_PROPERTY
                        {
                            let del_bits = property_del_bits(desc_ptr);
                            if obj_from_bits(del_bits).is_none() {
                                return property_no_deleter(
                                    _py,
                                    attr_name,
                                    class_ptr,
                                    MoltObject::from_ptr(obj_ptr).bits(),
                                );
                            }
                            let inst_bits = instance_bits_for_call(obj_ptr);
                            let _ = call_function_obj1(_py, del_bits, inst_bits);
                            return MoltObject::none().bits() as i64;
                        }
                        let del_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.delete_name,
                            b"__delete__",
                        );
                        if let Some(method_bits) = descriptor_method_bits(_py, desc_bits, del_bits)
                        {
                            let self_bits = desc_bits;
                            let inst_bits = instance_bits_for_call(obj_ptr);
                            let method_obj = obj_from_bits(method_bits);
                            if let Some(method_ptr) = method_obj.as_ptr() {
                                if object_type_id(method_ptr) == TYPE_ID_FUNCTION {
                                    let _ =
                                        call_function_obj2(_py, method_bits, self_bits, inst_bits);
                                } else {
                                    let _ = call_callable1(_py, method_bits, inst_bits);
                                }
                            } else {
                                let _ = call_callable1(_py, method_bits, inst_bits);
                            }
                            return MoltObject::none().bits() as i64;
                        }
                        let set_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.set_name,
                            b"__set__",
                        );
                        if descriptor_method_bits(_py, desc_bits, set_bits).is_some() {
                            return descriptor_no_deleter(
                                _py,
                                attr_name,
                                class_ptr,
                                MoltObject::from_ptr(obj_ptr).bits(),
                            );
                        }
                    }
                }
                if (*desc_ptr).frozen {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "cannot delete frozen dataclass field",
                    );
                }
                if !(*desc_ptr).allows_dict {
                    let name = &(*desc_ptr).name;
                    let type_label = if name.is_empty() {
                        "dataclass"
                    } else {
                        name.as_str()
                    };
                    return attr_error(_py, type_label, attr_name);
                }
            }
            let dict_bits = dataclass_dict_bits(obj_ptr);
            if dict_bits != 0
                && let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                && object_type_id(dict_ptr) == TYPE_ID_DICT
                && dict_del_in_place(_py, dict_ptr, attr_bits)
            {
                return MoltObject::none().bits() as i64;
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
            return attr_error(_py, type_label, attr_name);
        }
        if type_id == TYPE_ID_OBJECT {
            let _header = header_from_obj_ptr(obj_ptr);
            if crate::object::object_poll_fn(obj_ptr) != 0 {
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
                let delattr_bits = intern_static_name(
                    _py,
                    &runtime_state(_py).interned.delattr_name,
                    b"__delattr__",
                );
                if let Some(call_bits) =
                    class_attr_lookup(_py, class_ptr, class_ptr, Some(obj_ptr), delattr_bits)
                {
                    // Short-circuit default object.__delattr__ to avoid the
                    // bound method call overhead and ensure field slots +
                    // instance dict are both cleared correctly.
                    if let Some(call_ptr) = obj_from_bits(call_bits).as_ptr() {
                        let is_default = match object_type_id(call_ptr) {
                            TYPE_ID_BOUND_METHOD => {
                                let inner = bound_method_func_bits(call_ptr);
                                crate::call::type_policy::callable_matches_runtime_symbol(
                                    Some(inner),
                                    fn_addr!(crate::molt_object_delattr),
                                )
                            }
                            TYPE_ID_FUNCTION => {
                                crate::call::type_policy::callable_matches_runtime_symbol(
                                    Some(call_bits),
                                    fn_addr!(crate::molt_object_delattr),
                                )
                            }
                            _ => false,
                        };
                        if is_default {
                            return object_delattr_raw(_py, obj_ptr, attr_bits, attr_name);
                        }
                    }
                    let _ = call_callable1(_py, call_bits, attr_bits);
                    return MoltObject::none().bits() as i64;
                }
                if let Some(desc_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits) {
                    if let Some(desc_ptr) = maybe_ptr_from_bits(desc_bits)
                        && object_type_id(desc_ptr) == TYPE_ID_PROPERTY
                    {
                        let del_bits = property_del_bits(desc_ptr);
                        if obj_from_bits(del_bits).is_none() {
                            return property_no_deleter(
                                _py,
                                attr_name,
                                class_ptr,
                                MoltObject::from_ptr(obj_ptr).bits(),
                            );
                        }
                        let inst_bits = instance_bits_for_call(obj_ptr);
                        let _ = call_function_obj1(_py, del_bits, inst_bits);
                        return MoltObject::none().bits() as i64;
                    }
                    let del_bits = intern_static_name(
                        _py,
                        &runtime_state(_py).interned.delete_name,
                        b"__delete__",
                    );
                    if let Some(method_bits) = descriptor_method_bits(_py, desc_bits, del_bits) {
                        let self_bits = desc_bits;
                        let inst_bits = instance_bits_for_call(obj_ptr);
                        let method_obj = obj_from_bits(method_bits);
                        if let Some(method_ptr) = method_obj.as_ptr() {
                            if object_type_id(method_ptr) == TYPE_ID_FUNCTION {
                                let _ = call_function_obj2(_py, method_bits, self_bits, inst_bits);
                            } else {
                                let _ = call_callable1(_py, method_bits, inst_bits);
                            }
                        } else {
                            let _ = call_callable1(_py, method_bits, inst_bits);
                        }
                        return MoltObject::none().bits() as i64;
                    }
                    let set_bits =
                        intern_static_name(_py, &runtime_state(_py).interned.set_name, b"__set__");
                    if descriptor_method_bits(_py, desc_bits, set_bits).is_some() {
                        return descriptor_no_deleter(
                            _py,
                            attr_name,
                            class_ptr,
                            MoltObject::from_ptr(obj_ptr).bits(),
                        );
                    }
                }
            }
            if class_bits != 0
                && let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
                && object_type_id(class_ptr) == TYPE_ID_TYPE
                && let Some(offset) = class_field_offset(_py, class_ptr, attr_bits)
            {
                let slot = obj_ptr.add(offset) as *const u64;
                if is_missing_bits(_py, *slot) {
                    return attr_error(_py, "object", attr_name);
                }
                let missing = missing_bits(_py);
                let _ = object_field_set_ptr_raw(_py, obj_ptr, offset, missing);
                // Also remove from instance dict (dual storage).
                let dict_bits = instance_dict_bits(obj_ptr);
                if dict_bits != 0
                    && let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                    && object_type_id(dict_ptr) == TYPE_ID_DICT
                {
                    dict_del_in_place(_py, dict_ptr, attr_bits);
                }
                return MoltObject::none().bits() as i64;
            }
            let dict_bits = instance_dict_bits(obj_ptr);
            if dict_bits != 0
                && let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                && object_type_id(dict_ptr) == TYPE_ID_DICT
                && dict_del_in_place(_py, dict_ptr, attr_bits)
            {
                return MoltObject::none().bits() as i64;
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
) -> i64 {
    unsafe {
        let _header = header_from_obj_ptr(obj_ptr);
        if crate::object::object_poll_fn(obj_ptr) != 0 {
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
            slots_info = class_slots_info(_py, class_ptr, attr_bits);
            if let Some(offset) = class_own_slot_field_offset(_py, class_ptr, attr_bits) {
                return object_field_set_ptr_raw(_py, obj_ptr, offset, val_bits) as i64;
            }
            if let Some(desc_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)
                && descriptor_is_data(_py, desc_bits)
            {
                let desc_obj = obj_from_bits(desc_bits);
                if let Some(desc_ptr) = desc_obj.as_ptr()
                    && object_type_id(desc_ptr) == TYPE_ID_PROPERTY
                {
                    let set_bits = property_set_bits(desc_ptr);
                    if obj_from_bits(set_bits).is_none() {
                        return property_no_setter(
                            _py,
                            attr_name,
                            class_ptr,
                            MoltObject::from_ptr(obj_ptr).bits(),
                        );
                    }
                    let inst_bits = instance_bits_for_call(obj_ptr);
                    let _ = call_function_obj2(_py, set_bits, inst_bits, val_bits);
                    return MoltObject::none().bits() as i64;
                }
                let set_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.set_name, b"__set__");
                if let Some(method_bits) = descriptor_method_bits(_py, desc_bits, set_bits) {
                    let inst_bits = instance_bits_for_call(obj_ptr);
                    let method_obj = obj_from_bits(method_bits);
                    if let Some(method_ptr) = method_obj.as_ptr() {
                        if object_type_id(method_ptr) == TYPE_ID_FUNCTION {
                            let _ = call_function_obj3(
                                _py,
                                method_bits,
                                desc_bits,
                                inst_bits,
                                val_bits,
                            );
                        } else {
                            let _ = call_callable2(_py, method_bits, inst_bits, val_bits);
                        }
                    } else {
                        let _ = call_callable2(_py, method_bits, inst_bits, val_bits);
                    }
                    return MoltObject::none().bits() as i64;
                }
                return descriptor_no_setter(
                    _py,
                    attr_name,
                    class_ptr,
                    MoltObject::from_ptr(obj_ptr).bits(),
                );
            }
            if let Some(offset) = class_field_offset(_py, class_ptr, attr_bits) {
                return object_field_set_ptr_raw(_py, obj_ptr, offset, val_bits) as i64;
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
        let mut dict_bits = instance_dict_bits(obj_ptr);
        if dict_bits != 0 {
            let valid = obj_from_bits(dict_bits)
                .as_ptr()
                .is_some_and(|ptr| object_type_id(ptr) == TYPE_ID_DICT);
            if !valid {
                dict_bits = 0;
                instance_set_dict_bits(_py, obj_ptr, 0);
            }
        }
        if dict_bits == 0 {
            let dict_ptr = alloc_dict_with_pairs(_py, &[]);
            if dict_ptr.is_null() {
                return MoltObject::none().bits() as i64;
            }
            dict_bits = MoltObject::from_ptr(dict_ptr).bits();
            instance_set_dict_bits(_py, obj_ptr, dict_bits);
        }
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
            && object_type_id(dict_ptr) == TYPE_ID_DICT
        {
            dict_set_in_place(_py, dict_ptr, attr_bits, val_bits);
            return MoltObject::none().bits() as i64;
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
) -> i64 {
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
            let class_bits = (*desc_ptr).class_bits;
            if class_bits != 0
                && let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
                && object_type_id(class_ptr) == TYPE_ID_TYPE
                && class_own_slot_field_offset(_py, class_ptr, attr_bits).is_some()
                && let Some(&index) = (*desc_ptr).field_name_to_index.get(attr_name)
            {
                let fields = dataclass_fields_mut(obj_ptr);
                if index < fields.len() {
                    let old_bits = fields[index];
                    if old_bits != val_bits {
                        dec_ref_bits(_py, old_bits);
                        inc_ref_bits(_py, val_bits);
                        fields[index] = val_bits;
                    }
                }
                return MoltObject::none().bits() as i64;
            }
            if class_bits != 0
                && let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
                && object_type_id(class_ptr) == TYPE_ID_TYPE
                && let Some(desc_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)
                && descriptor_is_data(_py, desc_bits)
            {
                let desc_obj = obj_from_bits(desc_bits);
                if let Some(desc_ptr) = desc_obj.as_ptr()
                    && object_type_id(desc_ptr) == TYPE_ID_PROPERTY
                {
                    let set_bits = property_set_bits(desc_ptr);
                    if obj_from_bits(set_bits).is_none() {
                        return property_no_setter(
                            _py,
                            attr_name,
                            class_ptr,
                            MoltObject::from_ptr(obj_ptr).bits(),
                        );
                    }
                    let inst_bits = instance_bits_for_call(obj_ptr);
                    let _ = call_function_obj2(_py, set_bits, inst_bits, val_bits);
                    return MoltObject::none().bits() as i64;
                }
                let set_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.set_name, b"__set__");
                if let Some(method_bits) = descriptor_method_bits(_py, desc_bits, set_bits) {
                    let inst_bits = instance_bits_for_call(obj_ptr);
                    let method_obj = obj_from_bits(method_bits);
                    if let Some(method_ptr) = method_obj.as_ptr() {
                        if object_type_id(method_ptr) == TYPE_ID_FUNCTION {
                            let _ = call_function_obj3(
                                _py,
                                method_bits,
                                desc_bits,
                                inst_bits,
                                val_bits,
                            );
                        } else {
                            let _ = call_callable2(_py, method_bits, inst_bits, val_bits);
                        }
                    } else {
                        let _ = call_callable2(_py, method_bits, inst_bits, val_bits);
                    }
                    return MoltObject::none().bits() as i64;
                }
                return descriptor_no_setter(
                    _py,
                    attr_name,
                    class_ptr,
                    MoltObject::from_ptr(obj_ptr).bits(),
                );
            }
            if let Some(&index) = (*desc_ptr).field_name_to_index.get(attr_name) {
                let fields = dataclass_fields_mut(obj_ptr);
                if index < fields.len() {
                    let old_bits = fields[index];
                    if old_bits != val_bits {
                        dec_ref_bits(_py, old_bits);
                        inc_ref_bits(_py, val_bits);
                        fields[index] = val_bits;
                    }
                }
                if !(*desc_ptr).slots {
                    let dict_bits = dataclass_dict_bits(obj_ptr);
                    if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                        && object_type_id(dict_ptr) == TYPE_ID_DICT
                    {
                        dict_set_in_place(_py, dict_ptr, attr_bits, val_bits);
                    }
                }
                return MoltObject::none().bits() as i64;
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
        let mut dict_bits = dataclass_dict_bits(obj_ptr);
        if dict_bits == 0 {
            let dict_ptr = alloc_dict_with_pairs(_py, &[]);
            if dict_ptr.is_null() {
                return MoltObject::none().bits() as i64;
            }
            dict_bits = MoltObject::from_ptr(dict_ptr).bits();
            dataclass_set_dict_bits(_py, obj_ptr, dict_bits);
        }
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
            && object_type_id(dict_ptr) == TYPE_ID_DICT
        {
            dict_set_in_place(_py, dict_ptr, attr_bits, val_bits);
            return MoltObject::none().bits() as i64;
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
pub(crate) unsafe fn dataclass_setattr_raw(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    attr_name: &str,
    val_bits: u64,
) -> i64 {
    unsafe { dataclass_setattr_inner(_py, obj_ptr, attr_bits, attr_name, val_bits, true) }
}

pub(crate) unsafe fn dataclass_setattr_raw_unchecked(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    attr_name: &str,
    val_bits: u64,
) -> i64 {
    unsafe { dataclass_setattr_inner(_py, obj_ptr, attr_bits, attr_name, val_bits, false) }
}

pub(crate) unsafe fn object_delattr_raw(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    attr_name: &str,
) -> i64 {
    unsafe {
        let obj_bits = MoltObject::from_ptr(obj_ptr).bits();
        let _header = header_from_obj_ptr(obj_ptr);
        if crate::object::object_poll_fn(obj_ptr) != 0 {
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
        {
            if let Some(desc_ptr) = maybe_ptr_from_bits(desc_bits)
                && object_type_id(desc_ptr) == TYPE_ID_PROPERTY
            {
                let del_bits = property_del_bits(desc_ptr);
                if obj_from_bits(del_bits).is_none() {
                    return property_no_deleter(
                        _py,
                        attr_name,
                        class_ptr,
                        MoltObject::from_ptr(obj_ptr).bits(),
                    );
                }
                let inst_bits = instance_bits_for_call(obj_ptr);
                let _ = call_function_obj1(_py, del_bits, inst_bits);
                return MoltObject::none().bits() as i64;
            }
            let del_bits =
                intern_static_name(_py, &runtime_state(_py).interned.delete_name, b"__delete__");
            if let Some(method_bits) = descriptor_method_bits(_py, desc_bits, del_bits) {
                let inst_bits = instance_bits_for_call(obj_ptr);
                let method_obj = obj_from_bits(method_bits);
                if let Some(method_ptr) = method_obj.as_ptr() {
                    if object_type_id(method_ptr) == TYPE_ID_FUNCTION {
                        let _ = call_function_obj2(_py, method_bits, desc_bits, inst_bits);
                    } else {
                        let _ = call_callable1(_py, method_bits, inst_bits);
                    }
                } else {
                    let _ = call_callable1(_py, method_bits, inst_bits);
                }
                return MoltObject::none().bits() as i64;
            }
            let set_bits =
                intern_static_name(_py, &runtime_state(_py).interned.set_name, b"__set__");
            if descriptor_method_bits(_py, desc_bits, set_bits).is_some() {
                return descriptor_no_deleter(
                    _py,
                    attr_name,
                    class_ptr,
                    MoltObject::from_ptr(obj_ptr).bits(),
                );
            }
        }
        if class_bits != 0
            && let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
            && object_type_id(class_ptr) == TYPE_ID_TYPE
            && let Some(offset) = class_field_offset(_py, class_ptr, attr_bits)
        {
            let slot = obj_ptr.add(offset) as *const u64;
            if is_missing_bits(_py, *slot) {
                return attr_error(_py, class_name_for_error(class_bits), attr_name);
            }
            let missing = missing_bits(_py);
            let _ = object_field_set_ptr_raw(_py, obj_ptr, offset, missing);
            // Also remove from instance dict (dual storage: __init__
            // stores in both field slot and instance dict for correctness).
            let dict_bits = instance_dict_bits(obj_ptr);
            if dict_bits != 0
                && let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                && object_type_id(dict_ptr) == TYPE_ID_DICT
            {
                dict_del_in_place(_py, dict_ptr, attr_bits);
            }
            return MoltObject::none().bits() as i64;
        }
        let dict_bits = instance_dict_bits(obj_ptr);
        if dict_bits != 0
            && let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
            && object_type_id(dict_ptr) == TYPE_ID_DICT
            && dict_del_in_place(_py, dict_ptr, attr_bits)
        {
            return MoltObject::none().bits() as i64;
        }
        // Deleting a non-existent attribute. CPython appends "and no __dict__ for
        // setting new attributes" (3.13+) ONLY when the instance has no `__dict__`
        // — i.e. a `__slots__`-only class. A class that allows a `__dict__` keeps
        // the bare `'X' object has no attribute 'Y'` message on every version.
        let slots_only = class_bits != 0
            && obj_from_bits(class_bits).as_ptr().is_some_and(|class_ptr| {
                object_type_id(class_ptr) == TYPE_ID_TYPE
                    && class_slots_info(_py, class_ptr, attr_bits)
                        .is_some_and(|info| !info.allows_dict)
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
) -> i64 {
    unsafe {
        let desc_ptr = dataclass_desc_ptr(obj_ptr);
        if !desc_ptr.is_null() {
            let class_bits = (*desc_ptr).class_bits;
            if class_bits != 0
                && let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
                && object_type_id(class_ptr) == TYPE_ID_TYPE
                && let Some(desc_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)
            {
                if let Some(desc_ptr) = maybe_ptr_from_bits(desc_bits)
                    && object_type_id(desc_ptr) == TYPE_ID_PROPERTY
                {
                    let del_bits = property_del_bits(desc_ptr);
                    if obj_from_bits(del_bits).is_none() {
                        return property_no_deleter(
                            _py,
                            attr_name,
                            class_ptr,
                            MoltObject::from_ptr(obj_ptr).bits(),
                        );
                    }
                    let inst_bits = instance_bits_for_call(obj_ptr);
                    let _ = call_function_obj1(_py, del_bits, inst_bits);
                    return MoltObject::none().bits() as i64;
                }
                let del_bits = intern_static_name(
                    _py,
                    &runtime_state(_py).interned.delete_name,
                    b"__delete__",
                );
                if let Some(method_bits) = descriptor_method_bits(_py, desc_bits, del_bits) {
                    let inst_bits = instance_bits_for_call(obj_ptr);
                    let method_obj = obj_from_bits(method_bits);
                    if let Some(method_ptr) = method_obj.as_ptr() {
                        if object_type_id(method_ptr) == TYPE_ID_FUNCTION {
                            let _ = call_function_obj2(_py, method_bits, desc_bits, inst_bits);
                        } else {
                            let _ = call_callable1(_py, method_bits, inst_bits);
                        }
                    } else {
                        let _ = call_callable1(_py, method_bits, inst_bits);
                    }
                    return MoltObject::none().bits() as i64;
                }
                let set_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.set_name, b"__set__");
                if descriptor_method_bits(_py, desc_bits, set_bits).is_some() {
                    return descriptor_no_deleter(
                        _py,
                        attr_name,
                        class_ptr,
                        MoltObject::from_ptr(obj_ptr).bits(),
                    );
                }
            }
            if enforce_frozen && (*desc_ptr).frozen {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "cannot delete frozen dataclass field",
                );
            }
            if let Some(&index) = (*desc_ptr).field_name_to_index.get(attr_name) {
                let fields = dataclass_fields_mut(obj_ptr);
                if index < fields.len() {
                    let old_bits = fields[index];
                    if !is_missing_bits(_py, old_bits) {
                        let missing = missing_bits(_py);
                        dec_ref_bits(_py, old_bits);
                        inc_ref_bits(_py, missing);
                        fields[index] = missing;
                    }
                }
                if !(*desc_ptr).slots {
                    let dict_bits = dataclass_dict_bits(obj_ptr);
                    if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                        && object_type_id(dict_ptr) == TYPE_ID_DICT
                    {
                        let _ = dict_del_in_place(_py, dict_ptr, attr_bits);
                    }
                }
                return MoltObject::none().bits() as i64;
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
        let dict_bits = dataclass_dict_bits(obj_ptr);
        if dict_bits != 0
            && let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
            && object_type_id(dict_ptr) == TYPE_ID_DICT
            && dict_del_in_place(_py, dict_ptr, attr_bits)
        {
            return MoltObject::none().bits() as i64;
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
) -> i64 {
    unsafe { dataclass_delattr_inner(_py, obj_ptr, attr_bits, attr_name, true) }
}

pub(crate) unsafe fn dataclass_delattr_raw_unchecked(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    attr_name: &str,
) -> i64 {
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
) -> i64 {
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
) -> i64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let attr_name_len = usize_from_bits(attr_name_len_bits);
            if obj_ptr.is_null() {
                return raise_exception::<_>(_py, "AttributeError", "object has no attribute");
            }
            let slice = std::slice::from_raw_parts(attr_name_ptr, attr_name_len);
            let attr_name = std::str::from_utf8(slice).unwrap_or("<attr>");
            let Some(attr_bits) = attr_name_bits_from_bytes(_py, slice) else {
                return MoltObject::none().bits() as i64;
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
) -> i64 {
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
) -> i64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let attr_name_len = usize_from_bits(attr_name_len_bits);
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
) -> i64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let attr_name_len = usize_from_bits(attr_name_len_bits);
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
                let bytes = string_bytes(name_ptr);
                let len = string_len(name_ptr);
                return molt_set_attr_generic(obj_ptr, bytes, len as u64, val_bits) as u64;
            }
        }
        let obj = obj_from_bits(obj_bits);
        let name =
            string_obj_to_owned(obj_from_bits(name_bits)).unwrap_or_else(|| "<attr>".to_string());
        attr_error(_py, type_name(_py, obj), &name) as u64
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
                return del_attr_ptr(_py, obj_ptr, name_bits, &attr_name) as u64;
            }
            let obj = obj_from_bits(obj_bits);
            attr_error(_py, type_name(_py, obj), &attr_name) as u64
        }
    })
}
