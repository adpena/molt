use crate::state::tls::FRAME_STACK;
use crate::{
    alloc_class_obj, alloc_dict_with_pairs, alloc_exception_from_class_bits,
    alloc_instance_for_class, alloc_object, alloc_string, alloc_tuple, apply_class_slots_layout,
    attr_lookup_ptr, attr_lookup_ptr_allow_missing, attr_name_bits_from_bytes, bits_from_ptr,
    bound_method_func_bits, bound_method_self_bits, builtin_classes, call_callable0,
    call_callable1, call_class_init_with_args, call_function_obj_vec, class_attr_lookup,
    class_attr_lookup_raw_mro, class_dict_bits, class_name_bits, class_name_for_error,
    code_filename_bits, code_name_bits, dec_ref_bits, dict_fromkeys_method, dict_get_in_place,
    dict_get_method, dict_order, dict_pop_method, dict_setdefault_method, dict_update_apply,
    dict_update_method, dict_update_set_in_place, dict_update_set_via_store, exception_class_bits,
    exception_pending, exception_type_bits_from_name, function_arity, function_attr_bits,
    function_closure_bits, function_fn_ptr, function_name_bits, generic_alias_origin_bits,
    inc_ref_bits, init_atomic_bits, intern_static_name, is_builtin_class_bits, is_truthy,
    isinstance_bits, issubclass_bits, list_len, lookup_call_attr, maybe_ptr_from_bits,
    missing_bits, molt_bytearray_count_slice, molt_bytearray_decode, molt_bytearray_endswith_slice,
    molt_bytearray_find_slice, molt_bytearray_hex, molt_bytearray_rfind_slice,
    molt_bytearray_rsplit_max, molt_bytearray_split_max, molt_bytearray_splitlines,
    molt_bytearray_startswith_slice, molt_bytes_count_slice, molt_bytes_decode,
    molt_bytes_endswith_slice, molt_bytes_find_slice, molt_bytes_hex, molt_bytes_rfind_slice,
    molt_bytes_rsplit_max, molt_bytes_split_max, molt_bytes_splitlines,
    molt_bytes_startswith_slice, molt_class_set_base, molt_dataclass_new, molt_dataclass_set_class,
    molt_dict_from_obj, molt_dict_new, molt_file_reconfigure, molt_frozenset_copy_method,
    molt_frozenset_difference_multi, molt_frozenset_intersection_multi, molt_frozenset_isdisjoint,
    molt_frozenset_issubset, molt_frozenset_issuperset, molt_frozenset_symmetric_difference,
    molt_frozenset_union_multi, molt_function_default_kind, molt_generator_new, molt_int_new,
    molt_iter, molt_iter_next, molt_list_index_range, molt_list_pop, molt_list_sort,
    molt_memoryview_cast, molt_object_init, molt_object_init_subclass, molt_object_new_bound,
    molt_open_builtin, molt_set_clear, molt_set_copy_method, molt_set_difference_multi,
    molt_set_difference_update_multi, molt_set_intersection_multi,
    molt_set_intersection_update_multi, molt_set_isdisjoint, molt_set_issubset,
    molt_set_issuperset, molt_set_symmetric_difference, molt_set_symmetric_difference_update,
    molt_set_union_multi, molt_set_update_multi, molt_string_count_slice, molt_string_encode,
    molt_string_endswith_slice, molt_string_find_slice, molt_string_format_method,
    molt_string_rfind_slice, molt_string_rsplit_max, molt_string_split_max, molt_string_splitlines,
    molt_string_startswith_slice, molt_type_call, molt_type_init, molt_type_new, obj_eq,
    obj_from_bits, object_class_bits, object_set_class_bits, object_type_id, ptr_from_bits,
    raise_exception, raise_not_callable, raise_not_iterable, runtime_state, seq_vec_ref,
    string_obj_to_owned, tuple_len, type_name, type_of_bits, MoltHeader, MoltObject, PtrDropGuard,
    PyToken, BIND_KIND_OPEN, FUNC_DEFAULT_DICT_POP, FUNC_DEFAULT_DICT_UPDATE, FUNC_DEFAULT_IO_RAW,
    FUNC_DEFAULT_IO_TEXT_WRAPPER, FUNC_DEFAULT_MISSING, FUNC_DEFAULT_NEG_ONE, FUNC_DEFAULT_NONE,
    FUNC_DEFAULT_NONE2, FUNC_DEFAULT_REPLACE_COUNT, FUNC_DEFAULT_ZERO, GEN_CONTROL_SIZE,
    TYPE_ID_BOUND_METHOD, TYPE_ID_CALLARGS, TYPE_ID_DATACLASS, TYPE_ID_DICT, TYPE_ID_EXCEPTION,
    TYPE_ID_FROZENSET, TYPE_ID_FUNCTION, TYPE_ID_GENERIC_ALIAS, TYPE_ID_LIST, TYPE_ID_OBJECT,
    TYPE_ID_SET, TYPE_ID_STRING, TYPE_ID_TUPLE, TYPE_ID_TYPE,
};
pub(crate) struct CallArgs {
    pos: Vec<u64>,
    kw_names: Vec<u64>,
    kw_values: Vec<u64>,
}

unsafe fn is_default_type_call(_py: &PyToken<'_>, call_bits: u64) -> bool {
    let call_obj = obj_from_bits(call_bits);
    let Some(call_ptr) = call_obj.as_ptr() else {
        return false;
    };
    match object_type_id(call_ptr) {
        TYPE_ID_BOUND_METHOD => {
            let func_bits = bound_method_func_bits(call_ptr);
            is_default_type_call(_py, func_bits)
        }
        TYPE_ID_FUNCTION => function_fn_ptr(call_ptr) == fn_addr!(molt_type_call),
        _ => false,
    }
}

unsafe fn alloc_dataclass_for_class(_py: &PyToken<'_>, class_ptr: *mut u8) -> Option<u64> {
    let Some(field_names_name) = attr_name_bits_from_bytes(_py, b"__molt_dataclass_field_names__")
    else {
        return None;
    };
    let field_names_bits = class_attr_lookup_raw_mro(_py, class_ptr, field_names_name);
    dec_ref_bits(_py, field_names_name);
    let Some(field_names_bits) = field_names_bits else {
        return None;
    };
    let Some(field_names_ptr) = obj_from_bits(field_names_bits).as_ptr() else {
        return Some(raise_exception::<_>(
            _py,
            "TypeError",
            "dataclass field names must be a list/tuple of str",
        ));
    };
    let field_count = match object_type_id(field_names_ptr) {
        TYPE_ID_TUPLE => tuple_len(field_names_ptr),
        TYPE_ID_LIST => list_len(field_names_ptr),
        _ => {
            return Some(raise_exception::<_>(
                _py,
                "TypeError",
                "dataclass field names must be a list/tuple of str",
            ))
        }
    };
    let missing = missing_bits(_py);
    let mut values = Vec::with_capacity(field_count);
    values.resize(field_count, missing);
    let values_ptr = alloc_tuple(_py, &values);
    if values_ptr.is_null() {
        return Some(MoltObject::none().bits());
    }
    let values_bits = MoltObject::from_ptr(values_ptr).bits();
    let flags_bits =
        if let Some(flags_name) = attr_name_bits_from_bytes(_py, b"__molt_dataclass_flags__") {
            let bits = class_attr_lookup_raw_mro(_py, class_ptr, flags_name)
                .unwrap_or_else(|| MoltObject::from_int(0).bits());
            dec_ref_bits(_py, flags_name);
            bits
        } else {
            MoltObject::from_int(0).bits()
        };
    let name_bits = class_name_bits(class_ptr);
    let inst_bits = molt_dataclass_new(name_bits, field_names_bits, values_bits, flags_bits);
    dec_ref_bits(_py, values_bits);
    if exception_pending(_py) {
        return Some(MoltObject::none().bits());
    }
    let class_bits = MoltObject::from_ptr(class_ptr).bits();
    let _ = molt_dataclass_set_class(inst_bits, class_bits);
    if exception_pending(_py) {
        return Some(MoltObject::none().bits());
    }
    Some(inst_bits)
}

unsafe fn call_type_with_builder(
    _py: &PyToken<'_>,
    call_ptr: *mut u8,
    builder_ptr: *mut u8,
    builder_bits: u64,
    builder_guard: &mut PtrDropGuard,
) -> u64 {
    let class_bits = MoltObject::from_ptr(call_ptr).bits();
    let builtins = builtin_classes(_py);
    let args_ptr = if builder_ptr.is_null() {
        None
    } else {
        let ptr = callargs_ptr(builder_ptr);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        Some(ptr)
    };
    if let Some(ptr) = args_ptr {
        let pos_args = (*ptr).pos.as_slice();
        let kw_names = (*ptr).kw_names.as_slice();
        let kw_values = (*ptr).kw_values.as_slice();
        if class_bits == builtins.type_obj && pos_args.len() == 3 {
            return build_class_from_args(
                _py,
                class_bits,
                pos_args[0],
                pos_args[1],
                pos_args[2],
                kw_names,
                kw_values,
            );
        }
        if class_bits == builtins.type_obj && pos_args.len() == 1 && kw_names.is_empty() {
            let bits = type_of_bits(_py, pos_args[0]);
            inc_ref_bits(_py, bits);
            return bits;
        }
    }
    if is_builtin_class_bits(_py, class_bits) {
        if class_bits == builtins.dict {
            let (pos_args, kw_names, kw_values) = if let Some(ptr) = args_ptr {
                (
                    (*ptr).pos.as_slice(),
                    (*ptr).kw_names.as_slice(),
                    (*ptr).kw_values.as_slice(),
                )
            } else {
                (&[] as &[u64], &[] as &[u64], &[] as &[u64])
            };
            let dict_bits = match pos_args.len() {
                0 => molt_dict_new(0),
                1 => molt_dict_from_obj(pos_args[0]),
                _ => {
                    let msg = format!("dict expected at most 1 argument, got {}", pos_args.len());
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            if obj_from_bits(dict_bits).is_none() {
                return MoltObject::none().bits();
            }
            for (name_bits, val_bits) in kw_names.iter().copied().zip(kw_values.iter().copied()) {
                dict_update_set_via_store(_py, dict_bits, name_bits, val_bits);
                if exception_pending(_py) {
                    dec_ref_bits(_py, dict_bits);
                    return MoltObject::none().bits();
                }
            }
            return dict_bits;
        }
        if class_bits == builtins.text_io_wrapper {
            if let Some(ptr) = args_ptr {
                if !(*ptr).kw_names.is_empty() {
                    if let Some(bound_args) = bind_builtin_class_text_io_wrapper(_py, &*ptr) {
                        return call_class_init_with_args(_py, call_ptr, &bound_args);
                    }
                    return MoltObject::none().bits();
                }
            }
        }
        if let Some(ptr) = args_ptr {
            if !(*ptr).kw_names.is_empty() {
                let class_name = class_name_for_error(class_bits);
                let msg = format!("{class_name}() takes no keyword arguments");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            return call_class_init_with_args(_py, call_ptr, &(*ptr).pos);
        }
        return call_class_init_with_args(_py, call_ptr, &[]);
    }
    let mut default_new = false;
    let inst_bits = if issubclass_bits(class_bits, builtins.base_exception) {
        let new_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.new_name, b"__new__");
        if let Some(new_bits) = class_attr_lookup_raw_mro(_py, call_ptr, new_name_bits) {
            let (pos_len, kw_len) = if builder_ptr.is_null() {
                (1usize, 0usize)
            } else {
                let args_ptr = callargs_ptr(builder_ptr);
                if args_ptr.is_null() {
                    (1usize, 0usize)
                } else {
                    (1 + (*args_ptr).pos.len(), (*args_ptr).kw_names.len())
                }
            };
            let new_builder_bits = molt_callargs_new(pos_len as u64, kw_len as u64);
            if new_builder_bits == 0 {
                return MoltObject::none().bits();
            }
            let _ = molt_callargs_push_pos(new_builder_bits, class_bits);
            if !builder_ptr.is_null() {
                let args_ptr = callargs_ptr(builder_ptr);
                if !args_ptr.is_null() {
                    for &arg in (*args_ptr).pos.iter() {
                        let _ = molt_callargs_push_pos(new_builder_bits, arg);
                    }
                    for (&name_bits, &val_bits) in (*args_ptr)
                        .kw_names
                        .iter()
                        .zip((*args_ptr).kw_values.iter())
                    {
                        let _ = molt_callargs_push_kw(new_builder_bits, name_bits, val_bits);
                    }
                }
            }
            let inst_bits = molt_call_bind(new_bits, new_builder_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if !isinstance_bits(_py, inst_bits, class_bits) {
                return inst_bits;
            }
            inst_bits
        } else {
            let args_bits = if builder_ptr.is_null() {
                let args_ptr = alloc_tuple(_py, &[]);
                if args_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                MoltObject::from_ptr(args_ptr).bits()
            } else {
                let args_ptr = callargs_ptr(builder_ptr);
                let tuple_ptr = if args_ptr.is_null() {
                    alloc_tuple(_py, &[])
                } else {
                    alloc_tuple(_py, &(*args_ptr).pos)
                };
                if tuple_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                MoltObject::from_ptr(tuple_ptr).bits()
            };
            let exc_ptr = alloc_exception_from_class_bits(_py, class_bits, args_bits);
            dec_ref_bits(_py, args_bits);
            if exc_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(exc_ptr).bits()
        }
    } else {
        let new_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.new_name, b"__new__");
        if let Some(dict_ptr) = obj_from_bits(class_dict_bits(call_ptr)).as_ptr() {
            if object_type_id(dict_ptr) == TYPE_ID_DICT {
                if let Some(val_bits) = unsafe { dict_get_in_place(_py, dict_ptr, new_name_bits) } {
                    if let Some(val_ptr) = obj_from_bits(val_bits).as_ptr() {
                        let val_func_bits = if object_type_id(val_ptr) == TYPE_ID_BOUND_METHOD {
                            bound_method_func_bits(val_ptr)
                        } else {
                            val_bits
                        };
                        if let Some(val_func_ptr) = obj_from_bits(val_func_bits).as_ptr() {
                            if object_type_id(val_func_ptr) == TYPE_ID_FUNCTION
                                && function_fn_ptr(val_func_ptr) == fn_addr!(molt_object_new_bound)
                            {
                                default_new = true;
                            }
                        }
                    }
                } else {
                    default_new = true;
                }
            }
        }
        if let Some(new_bits) = class_attr_lookup_raw_mro(_py, call_ptr, new_name_bits) {
            if let Some(new_ptr) = obj_from_bits(new_bits).as_ptr() {
                let new_func_bits = if object_type_id(new_ptr) == TYPE_ID_BOUND_METHOD {
                    bound_method_func_bits(new_ptr)
                } else {
                    new_bits
                };
                if let Some(new_func_ptr) = obj_from_bits(new_func_bits).as_ptr() {
                    if object_type_id(new_func_ptr) == TYPE_ID_FUNCTION
                        && function_fn_ptr(new_func_ptr) == fn_addr!(molt_object_new_bound)
                    {
                        default_new = true;
                    }
                }
            }
            if default_new {
                if let Some(inst_bits) = alloc_dataclass_for_class(_py, call_ptr) {
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    inst_bits
                } else {
                    let (pos_len, kw_len) = if builder_ptr.is_null() {
                        (1usize, 0usize)
                    } else {
                        let args_ptr = callargs_ptr(builder_ptr);
                        if args_ptr.is_null() {
                            (1usize, 0usize)
                        } else {
                            (1 + (*args_ptr).pos.len(), (*args_ptr).kw_names.len())
                        }
                    };
                    let new_builder_bits = molt_callargs_new(pos_len as u64, kw_len as u64);
                    if new_builder_bits == 0 {
                        return MoltObject::none().bits();
                    }
                    let _ = molt_callargs_push_pos(new_builder_bits, class_bits);
                    if !builder_ptr.is_null() {
                        let args_ptr = callargs_ptr(builder_ptr);
                        if !args_ptr.is_null() {
                            for &arg in (*args_ptr).pos.iter() {
                                let _ = molt_callargs_push_pos(new_builder_bits, arg);
                            }
                            for (&name_bits, &val_bits) in (*args_ptr)
                                .kw_names
                                .iter()
                                .zip((*args_ptr).kw_values.iter())
                            {
                                let _ =
                                    molt_callargs_push_kw(new_builder_bits, name_bits, val_bits);
                            }
                        }
                    }
                    let inst_bits = molt_call_bind(new_bits, new_builder_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    if !isinstance_bits(_py, inst_bits, class_bits) {
                        return inst_bits;
                    }
                    inst_bits
                }
            } else {
                let (pos_len, kw_len) = if builder_ptr.is_null() {
                    (1usize, 0usize)
                } else {
                    let args_ptr = callargs_ptr(builder_ptr);
                    if args_ptr.is_null() {
                        (1usize, 0usize)
                    } else {
                        (1 + (*args_ptr).pos.len(), (*args_ptr).kw_names.len())
                    }
                };
                let new_builder_bits = molt_callargs_new(pos_len as u64, kw_len as u64);
                if new_builder_bits == 0 {
                    return MoltObject::none().bits();
                }
                let _ = molt_callargs_push_pos(new_builder_bits, class_bits);
                if !builder_ptr.is_null() {
                    let args_ptr = callargs_ptr(builder_ptr);
                    if !args_ptr.is_null() {
                        for &arg in (*args_ptr).pos.iter() {
                            let _ = molt_callargs_push_pos(new_builder_bits, arg);
                        }
                        for (&name_bits, &val_bits) in (*args_ptr)
                            .kw_names
                            .iter()
                            .zip((*args_ptr).kw_values.iter())
                        {
                            let _ = molt_callargs_push_kw(new_builder_bits, name_bits, val_bits);
                        }
                    }
                }
                let inst_bits = molt_call_bind(new_bits, new_builder_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                if !isinstance_bits(_py, inst_bits, class_bits) {
                    return inst_bits;
                }
                inst_bits
            }
        } else {
            alloc_instance_for_class(_py, call_ptr)
        }
    };
    let init_name_bits =
        intern_static_name(_py, &runtime_state(_py).interned.init_name, b"__init__");
    let Some(init_bits) = class_attr_lookup_raw_mro(_py, call_ptr, init_name_bits) else {
        return inst_bits;
    };
    if default_new && !builder_ptr.is_null() {
        let args_ptr = callargs_ptr(builder_ptr);
        if !args_ptr.is_null() && (!(*args_ptr).pos.is_empty() || !(*args_ptr).kw_names.is_empty())
        {
            if let Some(init_ptr) = obj_from_bits(init_bits).as_ptr() {
                let init_func_bits = if object_type_id(init_ptr) == TYPE_ID_BOUND_METHOD {
                    bound_method_func_bits(init_ptr)
                } else {
                    init_bits
                };
                if let Some(init_func_ptr) = obj_from_bits(init_func_bits).as_ptr() {
                    if object_type_id(init_func_ptr) == TYPE_ID_FUNCTION
                        && function_fn_ptr(init_func_ptr) == fn_addr!(molt_object_init)
                    {
                        let class_name = class_name_for_error(class_bits);
                        let msg = format!("{class_name}() takes no arguments");
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                }
            }
        }
    }
    if builder_ptr.is_null() {
        return inst_bits;
    }
    builder_guard.release();
    let args_ptr = callargs_ptr(builder_ptr);
    if !args_ptr.is_null() {
        (*args_ptr).pos.insert(0, inst_bits);
    }
    let _ = molt_call_bind(init_bits, builder_bits);
    inst_bits
}

unsafe fn build_class_from_args(
    _py: &PyToken<'_>,
    metaclass_bits: u64,
    name_bits: u64,
    bases_bits: u64,
    namespace_bits: u64,
    kw_names: &[u64],
    kw_values: &[u64],
) -> u64 {
    let name_obj = obj_from_bits(name_bits);
    let Some(name_ptr) = name_obj.as_ptr() else {
        return raise_exception::<_>(_py, "TypeError", "class name must be str");
    };
    if object_type_id(name_ptr) != TYPE_ID_STRING {
        return raise_exception::<_>(_py, "TypeError", "class name must be str");
    }

    let mut bases_vec: Vec<u64> = Vec::new();
    let mut bases_tuple_bits = bases_bits;
    let mut bases_owned = false;
    if obj_from_bits(bases_bits).is_none() || bases_bits == 0 {
        let tuple_ptr = alloc_tuple(_py, &[]);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        bases_tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
        bases_owned = true;
    } else if let Some(bases_ptr) = obj_from_bits(bases_bits).as_ptr() {
        match object_type_id(bases_ptr) {
            TYPE_ID_TUPLE => {
                bases_vec = seq_vec_ref(bases_ptr).clone();
            }
            TYPE_ID_TYPE => {
                let tuple_ptr = alloc_tuple(_py, &[bases_bits]);
                if tuple_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                bases_tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
                bases_owned = true;
                bases_vec.push(bases_bits);
            }
            _ => {
                return raise_exception::<_>(_py, "TypeError", "bases must be a tuple of types");
            }
        }
    }

    if bases_vec.is_empty() {
        let builtins = builtin_classes(_py);
        let tuple_ptr = alloc_tuple(_py, &[builtins.object]);
        if tuple_ptr.is_null() {
            if bases_owned {
                dec_ref_bits(_py, bases_tuple_bits);
            }
            return MoltObject::none().bits();
        }
        if bases_owned {
            dec_ref_bits(_py, bases_tuple_bits);
        }
        bases_tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
        bases_owned = true;
        bases_vec.push(builtins.object);
    }

    let class_ptr = alloc_class_obj(_py, name_bits);
    if class_ptr.is_null() {
        if bases_owned {
            dec_ref_bits(_py, bases_tuple_bits);
        }
        return MoltObject::none().bits();
    }
    let class_bits = MoltObject::from_ptr(class_ptr).bits();
    object_set_class_bits(_py, class_ptr, metaclass_bits);
    inc_ref_bits(_py, metaclass_bits);

    let dict_bits = class_dict_bits(class_ptr);
    let _ = dict_update_apply(_py, dict_bits, dict_update_set_in_place, namespace_bits);
    if exception_pending(_py) {
        if bases_owned {
            dec_ref_bits(_py, bases_tuple_bits);
        }
        return MoltObject::none().bits();
    }

    let _ = molt_class_set_base(class_bits, bases_tuple_bits);
    if exception_pending(_py) {
        if bases_owned {
            dec_ref_bits(_py, bases_tuple_bits);
        }
        return MoltObject::none().bits();
    }
    if !apply_class_slots_layout(_py, class_ptr) {
        if bases_owned {
            dec_ref_bits(_py, bases_tuple_bits);
        }
        return MoltObject::none().bits();
    }

    let init_name_bits = intern_static_name(
        _py,
        &runtime_state(_py).interned.init_subclass_name,
        b"__init_subclass__",
    );
    for base_bits in bases_vec.iter().copied() {
        let Some(base_ptr) = obj_from_bits(base_bits).as_ptr() else {
            continue;
        };
        let Some(init_bits) = attr_lookup_ptr_allow_missing(_py, base_ptr, init_name_bits) else {
            continue;
        };
        let builder_bits = molt_callargs_new((1 + kw_names.len()) as u64, kw_names.len() as u64);
        if builder_bits == 0 {
            dec_ref_bits(_py, init_bits);
            if bases_owned {
                dec_ref_bits(_py, bases_tuple_bits);
            }
            return MoltObject::none().bits();
        }
        let _ = molt_callargs_push_pos(builder_bits, class_bits);
        for (&name_bits, &val_bits) in kw_names.iter().zip(kw_values.iter()) {
            let _ = molt_callargs_push_kw(builder_bits, name_bits, val_bits);
        }
        let _ = molt_call_bind(init_bits, builder_bits);
        dec_ref_bits(_py, init_bits);
        if exception_pending(_py) {
            if bases_owned {
                dec_ref_bits(_py, bases_tuple_bits);
            }
            return MoltObject::none().bits();
        }
    }

    if bases_owned {
        dec_ref_bits(_py, bases_tuple_bits);
    }
    class_bits
}

pub(crate) unsafe fn callargs_ptr(ptr: *mut u8) -> *mut CallArgs {
    *(ptr as *mut *mut CallArgs)
}

#[no_mangle]
pub extern "C" fn molt_callargs_new(pos_capacity_bits: u64, kw_capacity_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<*mut CallArgs>();
        let ptr = alloc_object(_py, total, TYPE_ID_CALLARGS);
        if ptr.is_null() {
            return 0;
        }
        unsafe {
            let decode_capacity = |bits: u64| -> Option<usize> {
                let obj = MoltObject::from_bits(bits);
                if obj.is_int() {
                    let val = obj.as_int().unwrap_or(0);
                    return usize::try_from(val).ok();
                }
                if obj.is_bool() {
                    return Some(if obj.as_bool().unwrap_or(false) { 1 } else { 0 });
                }
                if obj.is_ptr() || obj.is_none() || obj.is_pending() {
                    return None;
                }
                if bits <= usize::MAX as u64 {
                    Some(bits as usize)
                } else {
                    None
                }
            };
            let Some(pos_capacity) = decode_capacity(pos_capacity_bits) else {
                let _ = raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "callargs capacity expects an integer",
                );
                return 0;
            };
            let Some(kw_capacity) = decode_capacity(kw_capacity_bits) else {
                let _ = raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "callargs capacity expects an integer",
                );
                return 0;
            };
            let args = Box::new(CallArgs {
                pos: Vec::with_capacity(pos_capacity),
                kw_names: Vec::with_capacity(kw_capacity),
                kw_values: Vec::with_capacity(kw_capacity),
            });
            let args_ptr = Box::into_raw(args);
            *(ptr as *mut *mut CallArgs) = args_ptr;
        }
        bits_from_ptr(ptr)
    })
}

/// # Safety
/// `builder_bits` must be a valid pointer returned by `molt_callargs_new` and
/// remain owned by the caller for the duration of this call.
#[no_mangle]
pub unsafe extern "C" fn molt_callargs_push_pos(builder_bits: u64, val: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let builder_ptr = ptr_from_bits(builder_bits);
        if builder_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let args_ptr = callargs_ptr(builder_ptr);
        if args_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let args = &mut *args_ptr;
        args.pos.push(val);
        MoltObject::none().bits()
    })
}

unsafe fn callargs_push_kw(
    _py: &PyToken<'_>,
    builder_ptr: *mut u8,
    name_bits: u64,
    val_bits: u64,
) -> u64 {
    let name_obj = obj_from_bits(name_bits);
    let Some(name_ptr) = name_obj.as_ptr() else {
        return raise_exception::<_>(_py, "TypeError", "keywords must be strings");
    };
    if object_type_id(name_ptr) != TYPE_ID_STRING {
        return raise_exception::<_>(_py, "TypeError", "keywords must be strings");
    }
    let args_ptr = callargs_ptr(builder_ptr);
    if args_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let args = &mut *args_ptr;
    for existing in args.kw_names.iter().copied() {
        if obj_eq(_py, obj_from_bits(existing), name_obj) {
            let name = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
            let msg = format!("got multiple values for keyword argument '{name}'");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
    }
    args.kw_names.push(name_bits);
    args.kw_values.push(val_bits);
    MoltObject::none().bits()
}

/// # Safety
/// `builder_bits` must be a valid pointer returned by `molt_callargs_new`.
/// `name_bits` must reference a Molt string object.
#[no_mangle]
pub unsafe extern "C" fn molt_callargs_push_kw(
    builder_bits: u64,
    name_bits: u64,
    val_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let builder_ptr = ptr_from_bits(builder_bits);
        if builder_ptr.is_null() {
            return MoltObject::none().bits();
        }
        callargs_push_kw(_py, builder_ptr, name_bits, val_bits)
    })
}

/// # Safety
/// `builder_bits` must be a valid pointer returned by `molt_callargs_new`.
#[no_mangle]
pub unsafe extern "C" fn molt_callargs_expand_star(builder_bits: u64, iterable_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let builder_ptr = ptr_from_bits(builder_bits);
        if builder_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let iter_bits = molt_iter(iterable_bits);
        if obj_from_bits(iter_bits).is_none() {
            return raise_not_iterable(_py, iterable_bits);
        }
        loop {
            let pair_bits = molt_iter_next(iter_bits);
            let pair_obj = obj_from_bits(pair_bits);
            let Some(pair_ptr) = pair_obj.as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                return MoltObject::none().bits();
            }
            let elems = seq_vec_ref(pair_ptr);
            if elems.len() < 2 {
                return MoltObject::none().bits();
            }
            let done_bits = elems[1];
            if is_truthy(_py, obj_from_bits(done_bits)) {
                break;
            }
            let val_bits = elems[0];
            let res = molt_callargs_push_pos(builder_bits, val_bits);
            if obj_from_bits(res).is_none() && exception_pending(_py) {
                return res;
            }
        }
        MoltObject::none().bits()
    })
}

/// # Safety
/// `builder_bits` must be a valid pointer returned by `molt_callargs_new`.
#[no_mangle]
pub unsafe extern "C" fn molt_callargs_expand_kwstar(builder_bits: u64, mapping_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let builder_ptr = ptr_from_bits(builder_bits);
        if builder_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let mapping_obj = obj_from_bits(mapping_bits);
        let Some(mapping_ptr) = mapping_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "argument after ** must be a mapping");
        };
        if object_type_id(mapping_ptr) == TYPE_ID_DICT {
            let order = dict_order(mapping_ptr);
            for idx in (0..order.len()).step_by(2) {
                let key_bits = order[idx];
                let val_bits = order[idx + 1];
                let res = callargs_push_kw(_py, builder_ptr, key_bits, val_bits);
                if obj_from_bits(res).is_none() && exception_pending(_py) {
                    return res;
                }
            }
            return MoltObject::none().bits();
        }
        let Some(keys_bits) = attr_name_bits_from_bytes(_py, b"keys") else {
            return raise_exception::<_>(_py, "TypeError", "argument after ** must be a mapping");
        };
        let keys_method_bits = attr_lookup_ptr(_py, mapping_ptr, keys_bits);
        dec_ref_bits(_py, keys_bits);
        let Some(keys_method_bits) = keys_method_bits else {
            return raise_exception::<_>(_py, "TypeError", "argument after ** must be a mapping");
        };
        let keys_iterable = call_callable0(_py, keys_method_bits);
        let iter_bits = molt_iter(keys_iterable);
        if obj_from_bits(iter_bits).is_none() {
            return raise_exception::<_>(_py, "TypeError", "argument after ** must be a mapping");
        }
        let Some(getitem_bits) = attr_name_bits_from_bytes(_py, b"__getitem__") else {
            return raise_exception::<_>(_py, "TypeError", "argument after ** must be a mapping");
        };
        let getitem_method_bits = attr_lookup_ptr(_py, mapping_ptr, getitem_bits);
        dec_ref_bits(_py, getitem_bits);
        let Some(getitem_method_bits) = getitem_method_bits else {
            return raise_exception::<_>(_py, "TypeError", "argument after ** must be a mapping");
        };
        loop {
            let pair_bits = molt_iter_next(iter_bits);
            let pair_obj = obj_from_bits(pair_bits);
            let Some(pair_ptr) = pair_obj.as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                return MoltObject::none().bits();
            }
            let elems = seq_vec_ref(pair_ptr);
            if elems.len() < 2 {
                return MoltObject::none().bits();
            }
            let done_bits = elems[1];
            if is_truthy(_py, obj_from_bits(done_bits)) {
                break;
            }
            let key_bits = elems[0];
            let key_obj = obj_from_bits(key_bits);
            let Some(key_ptr) = key_obj.as_ptr() else {
                return raise_exception::<_>(_py, "TypeError", "keywords must be strings");
            };
            if object_type_id(key_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "keywords must be strings");
            }
            let val_bits = call_callable1(_py, getitem_method_bits, key_bits);
            let res = callargs_push_kw(_py, builder_ptr, key_bits, val_bits);
            if obj_from_bits(res).is_none() && exception_pending(_py) {
                return res;
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
/// # Safety
/// Caller must ensure `builder_bits` is valid and points to a list builder.
pub extern "C" fn molt_call_bind(call_bits: u64, builder_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        unsafe {
            let builder_ptr = ptr_from_bits(builder_bits);
            let mut builder_guard = PtrDropGuard::new(builder_ptr);
            let call_obj = obj_from_bits(call_bits);
            let trace = matches!(
                std::env::var("MOLT_TRACE_CALL_BIND").ok().as_deref(),
                Some("1")
            );
            let Some(call_ptr) = call_obj.as_ptr() else {
                if trace {
                    if let Some(frame) = FRAME_STACK.with(|stack| stack.borrow().last().copied()) {
                        if let Some(code_ptr) = maybe_ptr_from_bits(frame.code_bits) {
                            let (name_bits, file_bits) =
                                unsafe { (code_name_bits(code_ptr), code_filename_bits(code_ptr)) };
                            let name = string_obj_to_owned(obj_from_bits(name_bits))
                                .unwrap_or_else(|| "<code>".to_string());
                            let file = string_obj_to_owned(obj_from_bits(file_bits))
                                .unwrap_or_else(|| "<file>".to_string());
                            eprintln!(
                                "molt call_bind frame name={} file={} line={}",
                                name, file, frame.line
                            );
                        }
                    }
                    let none_flag = call_obj.is_none();
                    let bool_flag = call_obj.as_bool();
                    let int_flag = call_obj.as_int();
                    let float_flag = call_obj.as_float();
                    eprintln!(
                        "molt call_bind callee bits=0x{call_bits:x} none={} bool={:?} int={:?} float={:?}",
                        none_flag,
                        bool_flag,
                        int_flag,
                        float_flag,
                    );
                    let bt = std::backtrace::Backtrace::force_capture();
                    eprintln!("molt call_bind: not ptr bits=0x{call_bits:x}\n{bt}",);
                    if !builder_ptr.is_null() {
                        let args_ptr = callargs_ptr(builder_ptr);
                        if !args_ptr.is_null() {
                            let pos_slice = &(*args_ptr).pos;
                            let kw_slice = &(*args_ptr).kw_names;
                            let pos_len = pos_slice.len();
                            let kw_len = kw_slice.len();
                            let first_pos = pos_slice.first().copied();
                            let second_pos = pos_slice.get(1).copied();
                            eprintln!(
                                "molt call_bind args pos_len={} kw_len={} first_pos={:?} second_pos={:?}",
                                pos_len,
                                kw_len,
                                first_pos,
                                second_pos,
                            );
                            if let Some(bits) = second_pos {
                                if let Some(s) = string_obj_to_owned(obj_from_bits(bits)) {
                                    eprintln!("molt call_bind args second_pos_str={}", s);
                                }
                            }
                        } else {
                            eprintln!("molt call_bind args ptr is null");
                        }
                    }
                }
                return raise_not_callable(_py, call_obj);
            };
            let mut func_bits = call_bits;
            let mut self_bits = None;
            match object_type_id(call_ptr) {
                TYPE_ID_FUNCTION => {}
                TYPE_ID_BOUND_METHOD => {
                    func_bits = bound_method_func_bits(call_ptr);
                    self_bits = Some(bound_method_self_bits(call_ptr));
                }
                TYPE_ID_TYPE => {
                    let meta_bits = object_class_bits(call_ptr);
                    if meta_bits != 0 {
                        if let Some(meta_ptr) = obj_from_bits(meta_bits).as_ptr() {
                            if object_type_id(meta_ptr) == TYPE_ID_TYPE {
                                let call_name_bits = intern_static_name(
                                    _py,
                                    &runtime_state(_py).interned.call_name,
                                    b"__call__",
                                );
                                if let Some(call_attr_bits) = class_attr_lookup(
                                    _py,
                                    meta_ptr,
                                    meta_ptr,
                                    Some(call_ptr),
                                    call_name_bits,
                                ) {
                                    if !is_default_type_call(_py, call_attr_bits) {
                                        builder_guard.release();
                                        return molt_call_bind(call_attr_bits, builder_bits);
                                    }
                                }
                            }
                        }
                    }
                    return call_type_with_builder(
                        _py,
                        call_ptr,
                        builder_ptr,
                        builder_bits,
                        &mut builder_guard,
                    );
                }
                TYPE_ID_GENERIC_ALIAS => {
                    let origin_bits = generic_alias_origin_bits(call_ptr);
                    builder_guard.release();
                    return molt_call_bind(origin_bits, builder_bits);
                }
                TYPE_ID_OBJECT | TYPE_ID_DATACLASS => {
                    let Some(call_attr_bits) = lookup_call_attr(_py, call_ptr) else {
                        return raise_not_callable(_py, call_obj);
                    };
                    builder_guard.release();
                    return molt_call_bind(call_attr_bits, builder_bits);
                }
                _ => return raise_not_callable(_py, call_obj),
            }
            let func_obj = obj_from_bits(func_bits);
            let Some(func_ptr) = func_obj.as_ptr() else {
                return raise_exception::<_>(_py, "TypeError", "call expects function object");
            };
            if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
                return raise_exception::<_>(_py, "TypeError", "call expects function object");
            }
            let fn_ptr = function_fn_ptr(func_ptr);
            if fn_ptr == fn_addr!(molt_type_call) {
                let Some(self_bits) = self_bits else {
                    return raise_exception::<_>(_py, "TypeError", "type.__call__ expects type");
                };
                let Some(self_ptr) = obj_from_bits(self_bits).as_ptr() else {
                    return raise_exception::<_>(_py, "TypeError", "type.__call__ expects type");
                };
                if object_type_id(self_ptr) != TYPE_ID_TYPE {
                    return raise_exception::<_>(_py, "TypeError", "type.__call__ expects type");
                }
                return call_type_with_builder(
                    _py,
                    self_ptr,
                    builder_ptr,
                    builder_bits,
                    &mut builder_guard,
                );
            }
            if builder_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let args_ptr = callargs_ptr(builder_ptr);
            if args_ptr.is_null() {
                return MoltObject::none().bits();
            }
            *(builder_ptr as *mut *mut CallArgs) = std::ptr::null_mut();
            let mut args = Box::from_raw(args_ptr);
            if let Some(self_bits) = self_bits {
                args.pos.insert(0, self_bits);
            }
            let bind_kind_bits = function_attr_bits(
                _py,
                func_ptr,
                intern_static_name(
                    _py,
                    &runtime_state(_py).interned.molt_bind_kind,
                    b"__molt_bind_kind__",
                ),
            );
            if let Some(kind_bits) = bind_kind_bits {
                if obj_from_bits(kind_bits).as_int() == Some(BIND_KIND_OPEN) {
                    if let Some(bound_args) = bind_builtin_open(_py, &args) {
                        return call_function_obj_vec(_py, func_bits, bound_args.as_slice());
                    }
                    return MoltObject::none().bits();
                }
            }
            if fn_ptr == fn_addr!(dict_update_method) {
                return bind_builtin_dict_update(_py, &args);
            }
            if fn_ptr == fn_addr!(molt_open_builtin) {
                if let Some(bound_args) = bind_builtin_open(_py, &args) {
                    return call_function_obj_vec(_py, func_bits, bound_args.as_slice());
                }
                return MoltObject::none().bits();
            }

            let arg_names_bits = function_attr_bits(
                _py,
                func_ptr,
                intern_static_name(
                    _py,
                    &runtime_state(_py).interned.molt_arg_names,
                    b"__molt_arg_names__",
                ),
            );
            let arg_names = if let Some(bits) = arg_names_bits {
                let arg_names_ptr = obj_from_bits(bits).as_ptr();
                let Some(arg_names_ptr) = arg_names_ptr else {
                    return raise_exception::<_>(_py, "TypeError", "call expects function object");
                };
                if object_type_id(arg_names_ptr) != TYPE_ID_TUPLE {
                    return raise_exception::<_>(_py, "TypeError", "call expects function object");
                }
                seq_vec_ref(arg_names_ptr).clone()
            } else {
                if let Some(bound_args) = bind_builtin_call(_py, func_bits, func_ptr, &args) {
                    return call_function_obj_vec(_py, func_bits, bound_args.as_slice());
                }
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                return raise_exception::<_>(_py, "TypeError", "call expects function object");
            };

            let posonly_bits = function_attr_bits(
                _py,
                func_ptr,
                intern_static_name(
                    _py,
                    &runtime_state(_py).interned.molt_posonly,
                    b"__molt_posonly__",
                ),
            )
            .unwrap_or_else(|| MoltObject::from_int(0).bits());
            let posonly = obj_from_bits(posonly_bits).as_int().unwrap_or(0).max(0) as usize;

            let kwonly_bits = function_attr_bits(
                _py,
                func_ptr,
                intern_static_name(
                    _py,
                    &runtime_state(_py).interned.molt_kwonly_names,
                    b"__molt_kwonly_names__",
                ),
            )
            .unwrap_or_else(|| MoltObject::none().bits());
            let mut kwonly_names: Vec<u64> = Vec::new();
            if !obj_from_bits(kwonly_bits).is_none() {
                let Some(kw_ptr) = obj_from_bits(kwonly_bits).as_ptr() else {
                    return raise_exception::<_>(_py, "TypeError", "call expects function object");
                };
                if object_type_id(kw_ptr) != TYPE_ID_TUPLE {
                    return raise_exception::<_>(_py, "TypeError", "call expects function object");
                }
                kwonly_names = seq_vec_ref(kw_ptr).clone();
            }

            let vararg_bits = function_attr_bits(
                _py,
                func_ptr,
                intern_static_name(
                    _py,
                    &runtime_state(_py).interned.molt_vararg,
                    b"__molt_vararg__",
                ),
            )
            .unwrap_or_else(|| MoltObject::none().bits());
            let varkw_bits = function_attr_bits(
                _py,
                func_ptr,
                intern_static_name(
                    _py,
                    &runtime_state(_py).interned.molt_varkw,
                    b"__molt_varkw__",
                ),
            )
            .unwrap_or_else(|| MoltObject::none().bits());
            let has_vararg = !obj_from_bits(vararg_bits).is_none();
            let has_varkw = !obj_from_bits(varkw_bits).is_none();

            let defaults_bits = function_attr_bits(
                _py,
                func_ptr,
                intern_static_name(
                    _py,
                    &runtime_state(_py).interned.defaults_name,
                    b"__defaults__",
                ),
            )
            .unwrap_or_else(|| MoltObject::none().bits());
            let mut defaults: Vec<u64> = Vec::new();
            if !obj_from_bits(defaults_bits).is_none() {
                let Some(def_ptr) = obj_from_bits(defaults_bits).as_ptr() else {
                    return raise_exception::<_>(_py, "TypeError", "call expects function object");
                };
                if object_type_id(def_ptr) != TYPE_ID_TUPLE {
                    return raise_exception::<_>(_py, "TypeError", "call expects function object");
                }
                defaults = seq_vec_ref(def_ptr).clone();
            }

            let kwdefaults_bits = function_attr_bits(
                _py,
                func_ptr,
                intern_static_name(
                    _py,
                    &runtime_state(_py).interned.kwdefaults_name,
                    b"__kwdefaults__",
                ),
            )
            .unwrap_or_else(|| MoltObject::none().bits());
            let mut kwdefaults_ptr = None;
            if !obj_from_bits(kwdefaults_bits).is_none() {
                let Some(ptr) = obj_from_bits(kwdefaults_bits).as_ptr() else {
                    return raise_exception::<_>(_py, "TypeError", "call expects function object");
                };
                if object_type_id(ptr) != TYPE_ID_DICT {
                    return raise_exception::<_>(_py, "TypeError", "call expects function object");
                }
                kwdefaults_ptr = Some(ptr);
            }

            let total_pos = arg_names.len();
            let kwonly_start = total_pos + if has_vararg { 1 } else { 0 };
            let total_params = kwonly_start + kwonly_names.len() + if has_varkw { 1 } else { 0 };
            let mut slots: Vec<Option<u64>> = vec![None; total_params];
            let mut extra_pos: Vec<u64> = Vec::new();
            for (idx, val) in args.pos.iter().copied().enumerate() {
                if idx < total_pos {
                    slots[idx] = Some(val);
                } else if has_vararg {
                    extra_pos.push(val);
                } else {
                    return raise_exception::<_>(_py, "TypeError", "too many positional arguments");
                }
            }

            let mut extra_kwargs: Vec<u64> = Vec::new();
            let mut posonly_kw_names: Vec<String> = Vec::new();
            let mut unexpected_kw: Option<String> = None;
            for (name_bits, val_bits) in args
                .kw_names
                .iter()
                .copied()
                .zip(args.kw_values.iter().copied())
            {
                let name_obj = obj_from_bits(name_bits);
                let mut matched = false;
                for (idx, param_bits) in arg_names.iter().copied().enumerate() {
                    if obj_eq(_py, name_obj, obj_from_bits(param_bits)) {
                        if idx < posonly {
                            let name =
                                string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
                            if !posonly_kw_names.contains(&name) {
                                posonly_kw_names.push(name);
                            }
                            matched = true;
                            break;
                        }
                        if slots[idx].is_some() {
                            let name =
                                string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
                            let msg = format!("got multiple values for argument '{name}'");
                            return raise_exception::<_>(_py, "TypeError", &msg);
                        }
                        slots[idx] = Some(val_bits);
                        matched = true;
                        break;
                    }
                }
                if matched {
                    continue;
                }
                for (kw_idx, kw_name_bits) in kwonly_names.iter().copied().enumerate() {
                    if obj_eq(_py, name_obj, obj_from_bits(kw_name_bits)) {
                        let slot_idx = kwonly_start + kw_idx;
                        if slots[slot_idx].is_some() {
                            let name =
                                string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
                            let msg = format!("got multiple values for argument '{name}'");
                            return raise_exception::<_>(_py, "TypeError", &msg);
                        }
                        slots[slot_idx] = Some(val_bits);
                        matched = true;
                        break;
                    }
                }
                if matched {
                    continue;
                }
                if has_varkw {
                    extra_kwargs.push(name_bits);
                    extra_kwargs.push(val_bits);
                } else if unexpected_kw.is_none() {
                    let name = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
                    unexpected_kw = Some(name);
                }
            }

            if !posonly_kw_names.is_empty() {
                let func_name_bits = function_name_bits(_py, func_ptr);
                let func_name = if func_name_bits == 0 || obj_from_bits(func_name_bits).is_none() {
                    "function".to_string()
                } else {
                    string_obj_to_owned(obj_from_bits(func_name_bits))
                        .unwrap_or_else(|| "function".to_string())
                };
                let name_list = posonly_kw_names.join(", ");
                let msg = format!(
                    "{func_name}() got some positional-only arguments passed as keyword arguments: '{name_list}'"
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            if let Some(name) = unexpected_kw {
                let msg = format!("got an unexpected keyword '{name}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }

            let defaults_len = defaults.len();
            let default_start = total_pos.saturating_sub(defaults_len);
            for idx in 0..total_pos {
                if slots[idx].is_some() {
                    continue;
                }
                if idx >= default_start {
                    slots[idx] = Some(defaults[idx - default_start]);
                    continue;
                }
                let name = string_obj_to_owned(obj_from_bits(arg_names[idx]))
                    .unwrap_or_else(|| "?".to_string());
                let msg = format!("missing required argument '{name}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }

            for (kw_idx, name_bits) in kwonly_names.iter().copied().enumerate() {
                let slot_idx = kwonly_start + kw_idx;
                if slots[slot_idx].is_some() {
                    continue;
                }
                let mut default = None;
                if let Some(dict_ptr) = kwdefaults_ptr {
                    default = dict_get_in_place(_py, dict_ptr, name_bits);
                }
                if let Some(val) = default {
                    slots[slot_idx] = Some(val);
                    continue;
                }
                let name = string_obj_to_owned(obj_from_bits(name_bits))
                    .unwrap_or_else(|| "?".to_string());
                let msg = format!("missing required keyword-only argument '{name}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }

            if has_vararg {
                let tuple_ptr = alloc_tuple(_py, extra_pos.as_slice());
                if tuple_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                slots[total_pos] = Some(MoltObject::from_ptr(tuple_ptr).bits());
            }

            if has_varkw {
                let dict_ptr = alloc_dict_with_pairs(_py, extra_kwargs.as_slice());
                if dict_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let varkw_idx = kwonly_start + kwonly_names.len();
                slots[varkw_idx] = Some(MoltObject::from_ptr(dict_ptr).bits());
            }

            let mut final_args: Vec<u64> = Vec::with_capacity(slots.len());
            for slot in slots {
                let Some(val) = slot else {
                    return raise_exception::<_>(_py, "TypeError", "call binding failed");
                };
                final_args.push(val);
            }
            let is_gen = function_attr_bits(
                _py,
                func_ptr,
                intern_static_name(
                    _py,
                    &runtime_state(_py).interned.molt_is_generator,
                    b"__molt_is_generator__",
                ),
            )
            .is_some_and(|bits| is_truthy(_py, obj_from_bits(bits)));
            if is_gen {
                let size_bits = function_attr_bits(
                    _py,
                    func_ptr,
                    intern_static_name(
                        _py,
                        &runtime_state(_py).interned.molt_closure_size,
                        b"__molt_closure_size__",
                    ),
                )
                .unwrap_or_else(|| MoltObject::none().bits());
                let Some(size_val) = obj_from_bits(size_bits).as_int() else {
                    return raise_exception::<_>(_py, "TypeError", "call expects function object");
                };
                if size_val < 0 {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "closure size must be non-negative",
                    );
                }
                let closure_size = size_val as usize;
                let fn_ptr = function_fn_ptr(func_ptr);
                let closure_bits = function_closure_bits(func_ptr);
                let mut payload: Vec<u64> =
                    Vec::with_capacity(final_args.len() + if closure_bits != 0 { 1 } else { 0 });
                if closure_bits != 0 {
                    payload.push(closure_bits);
                }
                payload.extend(final_args.iter().copied());
                let base = GEN_CONTROL_SIZE;
                let needed = base + payload.len() * std::mem::size_of::<u64>();
                if closure_size < needed {
                    return raise_exception::<_>(_py, "TypeError", "call expects function object");
                }
                let obj_bits = molt_generator_new(fn_ptr, closure_size as u64);
                let Some(obj_ptr) = obj_from_bits(obj_bits).as_ptr() else {
                    return MoltObject::none().bits();
                };
                let mut offset = base;
                for val_bits in payload {
                    let slot = obj_ptr.add(offset) as *mut u64;
                    *slot = val_bits;
                    inc_ref_bits(_py, val_bits);
                    offset += std::mem::size_of::<u64>();
                }
                return obj_bits;
            }
            call_function_obj_vec(_py, func_bits, final_args.as_slice())
        }
    })
}

unsafe fn bind_builtin_call(
    _py: &PyToken<'_>,
    func_bits: u64,
    func_ptr: *mut u8,
    args: &CallArgs,
) -> Option<Vec<u64>> {
    let fn_ptr = function_fn_ptr(func_ptr);
    if fn_ptr == fn_addr!(crate::builtins::exceptions::molt_exception_init)
        || fn_ptr == fn_addr!(crate::builtins::exceptions::molt_exception_new_bound)
    {
        return bind_builtin_exception_args(_py, args);
    }
    if fn_ptr == fn_addr!(molt_object_init) || fn_ptr == fn_addr!(molt_object_init_subclass) {
        let self_bits = args
            .pos
            .first()
            .copied()
            .unwrap_or_else(|| MoltObject::none().bits());
        return Some(vec![self_bits]);
    }
    if fn_ptr == fn_addr!(molt_object_new_bound) {
        let self_bits = args
            .pos
            .first()
            .copied()
            .unwrap_or_else(|| MoltObject::none().bits());
        return Some(vec![self_bits]);
    }
    if fn_ptr == fn_addr!(molt_int_new) {
        return bind_builtin_int_new(_py, args);
    }
    if fn_ptr == fn_addr!(molt_open_builtin) {
        return bind_builtin_open(_py, args);
    }
    if fn_ptr == fn_addr!(molt_type_new) || fn_ptr == fn_addr!(molt_type_init) {
        return bind_builtin_type_new_init(_py, args);
    }
    if fn_ptr == fn_addr!(dict_get_method) {
        return bind_builtin_keywords(
            _py,
            args,
            &["key", "default"],
            Some(MoltObject::none().bits()),
            None,
        );
    }
    if fn_ptr == fn_addr!(dict_setdefault_method) {
        return bind_builtin_keywords(
            _py,
            args,
            &["key", "default"],
            Some(MoltObject::none().bits()),
            None,
        );
    }
    if fn_ptr == fn_addr!(dict_fromkeys_method) {
        return bind_builtin_keywords(
            _py,
            args,
            &["iterable", "value"],
            Some(MoltObject::none().bits()),
            None,
        );
    }
    if fn_ptr == fn_addr!(dict_update_method) {
        return bind_builtin_keywords(_py, args, &["other"], Some(missing_bits(_py)), None);
    }
    if fn_ptr == fn_addr!(dict_pop_method) {
        return bind_builtin_pop(_py, args);
    }
    if fn_ptr == fn_addr!(molt_list_sort) {
        return bind_builtin_list_sort(_py, args);
    }
    if fn_ptr == fn_addr!(molt_list_pop) {
        return bind_builtin_list_pop(_py, args);
    }
    if fn_ptr == fn_addr!(molt_list_index_range) {
        return bind_builtin_list_index_range(_py, args);
    }
    if fn_ptr == fn_addr!(molt_string_find_slice) {
        return bind_builtin_string_find(_py, args, "find");
    }
    if fn_ptr == fn_addr!(molt_string_rfind_slice) {
        return bind_builtin_string_find(_py, args, "rfind");
    }
    if fn_ptr == fn_addr!(molt_bytes_find_slice) || fn_ptr == fn_addr!(molt_bytearray_find_slice) {
        return bind_builtin_string_find(_py, args, "find");
    }
    if fn_ptr == fn_addr!(molt_bytes_rfind_slice) || fn_ptr == fn_addr!(molt_bytearray_rfind_slice)
    {
        return bind_builtin_string_find(_py, args, "rfind");
    }
    if fn_ptr == fn_addr!(molt_string_split_max)
        || fn_ptr == fn_addr!(molt_bytes_split_max)
        || fn_ptr == fn_addr!(molt_bytearray_split_max)
    {
        return bind_builtin_split(_py, args, "split");
    }
    if fn_ptr == fn_addr!(molt_string_rsplit_max)
        || fn_ptr == fn_addr!(molt_bytes_rsplit_max)
        || fn_ptr == fn_addr!(molt_bytearray_rsplit_max)
    {
        return bind_builtin_split(_py, args, "rsplit");
    }
    if fn_ptr == fn_addr!(molt_string_count_slice)
        || fn_ptr == fn_addr!(molt_bytes_count_slice)
        || fn_ptr == fn_addr!(molt_bytearray_count_slice)
    {
        return bind_builtin_count(_py, args, "count");
    }
    if fn_ptr == fn_addr!(molt_string_startswith_slice) {
        return bind_builtin_prefix_check(_py, args, "startswith", "prefix");
    }
    if fn_ptr == fn_addr!(molt_string_endswith_slice) {
        return bind_builtin_prefix_check(_py, args, "endswith", "suffix");
    }
    if fn_ptr == fn_addr!(molt_bytes_startswith_slice)
        || fn_ptr == fn_addr!(molt_bytearray_startswith_slice)
    {
        return bind_builtin_prefix_check(_py, args, "startswith", "prefix");
    }
    if fn_ptr == fn_addr!(molt_bytes_endswith_slice)
        || fn_ptr == fn_addr!(molt_bytearray_endswith_slice)
    {
        return bind_builtin_prefix_check(_py, args, "endswith", "suffix");
    }
    if fn_ptr == fn_addr!(molt_bytes_hex) || fn_ptr == fn_addr!(molt_bytearray_hex) {
        return bind_builtin_bytes_hex(_py, args);
    }
    if fn_ptr == fn_addr!(molt_string_format_method) {
        return bind_builtin_string_format(_py, args);
    }
    if fn_ptr == fn_addr!(molt_string_splitlines)
        || fn_ptr == fn_addr!(molt_bytes_splitlines)
        || fn_ptr == fn_addr!(molt_bytearray_splitlines)
    {
        return bind_builtin_splitlines(_py, args);
    }
    if fn_ptr == fn_addr!(molt_set_union_multi) {
        return bind_builtin_set_multi(_py, args, "union", "set", TYPE_ID_SET);
    }
    if fn_ptr == fn_addr!(molt_frozenset_union_multi) {
        return bind_builtin_set_multi(_py, args, "union", "frozenset", TYPE_ID_FROZENSET);
    }
    if fn_ptr == fn_addr!(molt_set_intersection_multi) {
        return bind_builtin_set_multi(_py, args, "intersection", "set", TYPE_ID_SET);
    }
    if fn_ptr == fn_addr!(molt_frozenset_intersection_multi) {
        return bind_builtin_set_multi(_py, args, "intersection", "frozenset", TYPE_ID_FROZENSET);
    }
    if fn_ptr == fn_addr!(molt_set_difference_multi) {
        return bind_builtin_set_multi(_py, args, "difference", "set", TYPE_ID_SET);
    }
    if fn_ptr == fn_addr!(molt_frozenset_difference_multi) {
        return bind_builtin_set_multi(_py, args, "difference", "frozenset", TYPE_ID_FROZENSET);
    }
    if fn_ptr == fn_addr!(molt_set_update_multi) {
        return bind_builtin_set_multi(_py, args, "update", "set", TYPE_ID_SET);
    }
    if fn_ptr == fn_addr!(molt_set_intersection_update_multi) {
        return bind_builtin_set_multi(_py, args, "intersection_update", "set", TYPE_ID_SET);
    }
    if fn_ptr == fn_addr!(molt_set_difference_update_multi) {
        return bind_builtin_set_multi(_py, args, "difference_update", "set", TYPE_ID_SET);
    }
    if fn_ptr == fn_addr!(molt_set_symmetric_difference) {
        return bind_builtin_set_single(_py, args, "symmetric_difference", "set", TYPE_ID_SET);
    }
    if fn_ptr == fn_addr!(molt_frozenset_symmetric_difference) {
        return bind_builtin_set_single(
            _py,
            args,
            "symmetric_difference",
            "frozenset",
            TYPE_ID_FROZENSET,
        );
    }
    if fn_ptr == fn_addr!(molt_set_symmetric_difference_update) {
        return bind_builtin_set_single(
            _py,
            args,
            "symmetric_difference_update",
            "set",
            TYPE_ID_SET,
        );
    }
    if fn_ptr == fn_addr!(molt_set_isdisjoint) {
        return bind_builtin_set_single(_py, args, "isdisjoint", "set", TYPE_ID_SET);
    }
    if fn_ptr == fn_addr!(molt_frozenset_isdisjoint) {
        return bind_builtin_set_single(_py, args, "isdisjoint", "frozenset", TYPE_ID_FROZENSET);
    }
    if fn_ptr == fn_addr!(molt_set_issubset) {
        return bind_builtin_set_single(_py, args, "issubset", "set", TYPE_ID_SET);
    }
    if fn_ptr == fn_addr!(molt_frozenset_issubset) {
        return bind_builtin_set_single(_py, args, "issubset", "frozenset", TYPE_ID_FROZENSET);
    }
    if fn_ptr == fn_addr!(molt_set_issuperset) {
        return bind_builtin_set_single(_py, args, "issuperset", "set", TYPE_ID_SET);
    }
    if fn_ptr == fn_addr!(molt_frozenset_issuperset) {
        return bind_builtin_set_single(_py, args, "issuperset", "frozenset", TYPE_ID_FROZENSET);
    }
    if fn_ptr == fn_addr!(molt_set_copy_method) {
        return bind_builtin_set_noargs(_py, args, "copy", "set", TYPE_ID_SET);
    }
    if fn_ptr == fn_addr!(molt_frozenset_copy_method) {
        return bind_builtin_set_noargs(_py, args, "copy", "frozenset", TYPE_ID_FROZENSET);
    }
    if fn_ptr == fn_addr!(molt_set_clear) {
        return bind_builtin_set_noargs(_py, args, "clear", "set", TYPE_ID_SET);
    }
    if fn_ptr == fn_addr!(molt_string_encode) {
        return bind_builtin_text_codec(_py, args, "encode");
    }
    if fn_ptr == fn_addr!(molt_bytes_decode) || fn_ptr == fn_addr!(molt_bytearray_decode) {
        return bind_builtin_text_codec(_py, args, "decode");
    }
    if fn_ptr == fn_addr!(molt_memoryview_cast) {
        return bind_builtin_memoryview_cast(_py, args);
    }
    if fn_ptr == fn_addr!(molt_file_reconfigure) {
        return bind_builtin_file_reconfigure(_py, args);
    }

    if !args.kw_names.is_empty() {
        return raise_exception::<_>(
            _py,
            "TypeError",
            "keywords are not supported for this builtin",
        );
    }

    let mut out = args.pos.clone();
    let arity = function_arity(func_ptr) as usize;
    if out.len() > arity {
        return raise_exception::<_>(_py, "TypeError", "too many positional arguments");
    }
    let missing = arity - out.len();
    if missing == 0 {
        return Some(out);
    }
    let default_kind = molt_function_default_kind(func_bits);
    if missing == 1 {
        if default_kind == FUNC_DEFAULT_NONE {
            out.push(MoltObject::none().bits());
            return Some(out);
        }
        if default_kind == FUNC_DEFAULT_NONE2 {
            out.push(MoltObject::none().bits());
            return Some(out);
        }
        if default_kind == FUNC_DEFAULT_IO_RAW {
            out.push(MoltObject::none().bits());
            return Some(out);
        }
        if default_kind == FUNC_DEFAULT_IO_TEXT_WRAPPER {
            out.push(MoltObject::from_bool(false).bits());
            return Some(out);
        }
        if default_kind == FUNC_DEFAULT_DICT_POP {
            out.push(MoltObject::from_int(1).bits());
            return Some(out);
        }
        if default_kind == FUNC_DEFAULT_DICT_UPDATE {
            out.push(missing_bits(_py));
            return Some(out);
        }
        if default_kind == FUNC_DEFAULT_REPLACE_COUNT {
            out.push(MoltObject::from_int(-1).bits());
            return Some(out);
        }
        if default_kind == FUNC_DEFAULT_NEG_ONE {
            out.push(MoltObject::from_int(-1).bits());
            return Some(out);
        }
        if default_kind == FUNC_DEFAULT_ZERO {
            out.push(MoltObject::from_int(0).bits());
            return Some(out);
        }
        if default_kind == FUNC_DEFAULT_MISSING {
            out.push(missing_bits(_py));
            return Some(out);
        }
    }
    if missing == 2 {
        if default_kind == FUNC_DEFAULT_DICT_POP {
            out.push(MoltObject::none().bits());
            out.push(MoltObject::from_int(0).bits());
            return Some(out);
        }
        if default_kind == FUNC_DEFAULT_NONE2 {
            out.push(MoltObject::none().bits());
            out.push(MoltObject::none().bits());
            return Some(out);
        }
        if default_kind == FUNC_DEFAULT_IO_RAW {
            out.push(MoltObject::none().bits());
            out.push(MoltObject::none().bits());
            return Some(out);
        }
        if default_kind == FUNC_DEFAULT_IO_TEXT_WRAPPER {
            out.push(MoltObject::from_bool(false).bits());
            out.push(MoltObject::from_bool(false).bits());
            return Some(out);
        }
    }
    if missing == 3 {
        if default_kind == FUNC_DEFAULT_IO_RAW {
            out.push(MoltObject::none().bits());
            out.push(MoltObject::none().bits());
            out.push(MoltObject::none().bits());
            return Some(out);
        }
        if default_kind == FUNC_DEFAULT_IO_TEXT_WRAPPER {
            out.push(MoltObject::none().bits());
            out.push(MoltObject::from_bool(false).bits());
            out.push(MoltObject::from_bool(false).bits());
            return Some(out);
        }
    }
    if missing == 4 && default_kind == FUNC_DEFAULT_IO_TEXT_WRAPPER {
        out.push(MoltObject::none().bits());
        out.push(MoltObject::none().bits());
        out.push(MoltObject::from_bool(false).bits());
        out.push(MoltObject::from_bool(false).bits());
        return Some(out);
    }
    if missing == 5 && default_kind == FUNC_DEFAULT_IO_TEXT_WRAPPER {
        out.push(MoltObject::none().bits());
        out.push(MoltObject::none().bits());
        out.push(MoltObject::none().bits());
        out.push(MoltObject::from_bool(false).bits());
        out.push(MoltObject::from_bool(false).bits());
        return Some(out);
    }
    raise_exception::<_>(_py, "TypeError", "missing required arguments")
}

unsafe fn bind_builtin_exception_args(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required arguments");
    }
    if !args.kw_names.is_empty() {
        let head = args.pos[0];
        let head_obj = obj_from_bits(head);
        let Some(head_ptr) = head_obj.as_ptr() else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "keywords are not supported for this builtin",
            );
        };
        let allow_kw = match object_type_id(head_ptr) {
            TYPE_ID_TYPE => true,
            TYPE_ID_EXCEPTION => {
                let oserror_bits = exception_type_bits_from_name(_py, "OSError");
                issubclass_bits(exception_class_bits(head_ptr), oserror_bits)
            }
            _ => false,
        };
        if !allow_kw {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "keywords are not supported for this builtin",
            );
        }
    }
    let head = args.pos[0];
    let rest = &args.pos[1..];
    let tuple_ptr = alloc_tuple(_py, rest);
    if tuple_ptr.is_null() {
        return None;
    }
    let tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
    Some(vec![head, tuple_bits])
}

unsafe fn bind_builtin_int_new(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'cls'");
    }
    if args.pos.len() > 3 {
        return raise_exception::<_>(_py, "TypeError", "too many positional arguments");
    }
    let cls_bits = args.pos[0];
    let mut value_bits = args.pos.get(1).copied();
    let mut base_bits = args.pos.get(2).copied();
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        match name_str.as_str() {
            "x" => {
                if value_bits.is_some() {
                    let msg = format!("got multiple values for argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                value_bits = Some(val_bits);
            }
            "base" => {
                if base_bits.is_some() {
                    let msg = format!("got multiple values for argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                base_bits = Some(val_bits);
            }
            _ => {
                let msg = format!("got an unexpected keyword '{name_str}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
    }
    let value_bits = value_bits.unwrap_or_else(|| MoltObject::from_int(0).bits());
    let base_bits = base_bits.unwrap_or_else(|| missing_bits(_py));
    Some(vec![cls_bits, value_bits, base_bits])
}

unsafe fn bind_builtin_dict_update(_py: &PyToken<'_>, args: &CallArgs) -> u64 {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    let positional = args.pos.len().saturating_sub(1);
    if positional > 1 {
        let msg = format!("update expected at most 1 argument, got {}", positional);
        return raise_exception::<_>(_py, "TypeError", &msg);
    }
    let dict_bits = args.pos[0];
    if positional == 1 {
        let other_bits = args.pos[1];
        let dict_obj = obj_from_bits(dict_bits);
        if let Some(dict_ptr) = dict_obj.as_ptr() {
            if object_type_id(dict_ptr) == TYPE_ID_DICT {
                let _ = dict_update_apply(_py, dict_bits, dict_update_set_in_place, other_bits);
            } else {
                let _ = dict_update_apply(_py, dict_bits, dict_update_set_via_store, other_bits);
            }
        } else {
            let _ = dict_update_apply(_py, dict_bits, dict_update_set_via_store, other_bits);
        }
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
    }
    if !args.kw_names.is_empty() {
        for (name_bits, val_bits) in args
            .kw_names
            .iter()
            .copied()
            .zip(args.kw_values.iter().copied())
        {
            let name_obj = obj_from_bits(name_bits);
            let Some(name_ptr) = name_obj.as_ptr() else {
                return raise_exception::<_>(_py, "TypeError", "keywords must be strings");
            };
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "keywords must be strings");
            }
            dict_update_set_via_store(_py, dict_bits, name_bits, val_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
        }
    }
    MoltObject::none().bits()
}

fn default_open_mode_bits(_py: &PyToken<'_>) -> u64 {
    init_atomic_bits(
        _py,
        &runtime_state(_py).special_cache.open_default_mode,
        || {
            let ptr = alloc_string(_py, b"r");
            if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            }
        },
    )
}

unsafe fn bind_builtin_bytes_hex(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    if args.pos.len() > 3 {
        return raise_exception::<_>(_py, "TypeError", "too many positional arguments");
    }
    let self_bits = args.pos[0];
    let mut sep_bits = args.pos.get(1).copied();
    let mut bytes_per_sep_bits = args.pos.get(2).copied();
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        match name_str.as_str() {
            "sep" => {
                if sep_bits.is_some() {
                    let msg = format!("got multiple values for argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                sep_bits = Some(val_bits);
            }
            "bytes_per_sep" => {
                if bytes_per_sep_bits.is_some() {
                    let msg = format!("got multiple values for argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                bytes_per_sep_bits = Some(val_bits);
            }
            _ => {
                let msg = format!("got an unexpected keyword '{name_str}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
    }
    let sep_bits = sep_bits.unwrap_or_else(|| missing_bits(_py));
    let bytes_per_sep_bits = bytes_per_sep_bits.unwrap_or_else(|| missing_bits(_py));
    Some(vec![self_bits, sep_bits, bytes_per_sep_bits])
}

unsafe fn bind_builtin_keywords(
    _py: &PyToken<'_>,
    args: &CallArgs,
    names: &[&str],
    default_bits: Option<u64>,
    extra_bits: Option<u64>,
) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    let mut out = vec![args.pos[0]];
    let mut values: Vec<Option<u64>> = vec![None; names.len()];
    let mut pos_idx = 1usize;
    while pos_idx < args.pos.len() {
        let idx = pos_idx - 1;
        if idx >= names.len() {
            return raise_exception::<_>(_py, "TypeError", "too many positional arguments");
        }
        values[idx] = Some(args.pos[pos_idx]);
        pos_idx += 1;
    }
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        let mut matched = false;
        for (idx, expected) in names.iter().enumerate() {
            if name_str == *expected {
                if values[idx].is_some() {
                    let msg = format!("got multiple values for argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                values[idx] = Some(val_bits);
                matched = true;
                break;
            }
        }
        if !matched {
            let msg = format!("got an unexpected keyword '{name_str}'");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
    }
    for (idx, val) in values.iter_mut().enumerate() {
        if val.is_none() {
            if let Some(bits) = default_bits {
                *val = Some(bits);
                continue;
            }
            let name = names[idx];
            let msg = format!("missing required argument '{name}'");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
    }
    for val in values.into_iter().flatten() {
        out.push(val);
    }
    if let Some(extra) = extra_bits {
        out.push(extra);
    }
    Some(out)
}

unsafe fn bind_builtin_class_text_io_wrapper(
    _py: &PyToken<'_>,
    args: &CallArgs,
) -> Option<Vec<u64>> {
    const NAMES: [&str; 6] = [
        "buffer",
        "encoding",
        "errors",
        "newline",
        "line_buffering",
        "write_through",
    ];
    if args.pos.len() > NAMES.len() {
        return raise_exception::<_>(_py, "TypeError", "too many positional arguments");
    }
    let mut values: Vec<Option<u64>> = vec![None; NAMES.len()];
    for (idx, &val) in args.pos.iter().enumerate() {
        values[idx] = Some(val);
    }
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        let mut matched = false;
        for (idx, expected) in NAMES.iter().enumerate() {
            if name_str == *expected {
                if values[idx].is_some() {
                    let msg = format!("got multiple values for argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                values[idx] = Some(val_bits);
                matched = true;
                break;
            }
        }
        if !matched {
            let msg = format!("got an unexpected keyword '{name_str}'");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
    }
    if values[0].is_none() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'buffer'");
    }
    for idx in 1..=3 {
        if values[idx].is_none() {
            values[idx] = Some(MoltObject::none().bits());
        }
    }
    for idx in 4..=5 {
        if values[idx].is_none() {
            values[idx] = Some(MoltObject::from_bool(false).bits());
        }
    }
    Some(values.into_iter().flatten().collect())
}

unsafe fn bind_builtin_open(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    const NAMES: [&str; 8] = [
        "file",
        "mode",
        "buffering",
        "encoding",
        "errors",
        "newline",
        "closefd",
        "opener",
    ];
    let mut values: [Option<u64>; 8] = [None; 8];
    for (idx, val) in args.pos.iter().copied().enumerate() {
        if idx >= values.len() {
            let msg = format!(
                "open() takes at most 8 arguments ({} given)",
                args.pos.len()
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        values[idx] = Some(val);
    }
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        let mut matched = false;
        for (idx, expected) in NAMES.iter().enumerate() {
            if name_str == *expected {
                if values[idx].is_some() {
                    let msg = if idx < args.pos.len() {
                        format!(
                            "argument for open() given by name ('{name_str}') and position ({})",
                            idx + 1
                        )
                    } else {
                        format!("open() got multiple values for argument '{name_str}'")
                    };
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                values[idx] = Some(val_bits);
                matched = true;
                break;
            }
        }
        if !matched {
            let msg = format!("open() got an unexpected keyword argument '{name_str}'");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
    }
    if values[0].is_none() {
        return raise_exception::<_>(
            _py,
            "TypeError",
            "open() missing required argument 'file' (pos 1)",
        );
    }
    if values[1].is_none() {
        let mode_bits = default_open_mode_bits(_py);
        if obj_from_bits(mode_bits).is_none() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        values[1] = Some(mode_bits);
    }
    if values[2].is_none() {
        values[2] = Some(MoltObject::from_int(-1).bits());
    }
    if values[3].is_none() {
        values[3] = Some(MoltObject::none().bits());
    }
    if values[4].is_none() {
        values[4] = Some(MoltObject::none().bits());
    }
    if values[5].is_none() {
        values[5] = Some(MoltObject::none().bits());
    }
    if values[6].is_none() {
        values[6] = Some(MoltObject::from_bool(true).bits());
    }
    if values[7].is_none() {
        values[7] = Some(MoltObject::none().bits());
    }
    let mut out = Vec::with_capacity(values.len());
    for val in values {
        out.push(val.unwrap());
    }
    Some(out)
}

unsafe fn bind_builtin_type_new_init(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'cls'");
    }
    let mut values: [Option<u64>; 3] = [None, None, None];
    for (idx, val) in args.pos.iter().copied().enumerate().skip(1) {
        let pos_idx = idx - 1;
        if pos_idx >= values.len() {
            return raise_exception::<_>(_py, "TypeError", "too many positional arguments");
        }
        values[pos_idx] = Some(val);
    }
    let mut extra_pairs: Vec<u64> = Vec::new();
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "keywords must be strings");
        };
        if object_type_id(name_ptr) != TYPE_ID_STRING {
            return raise_exception::<_>(_py, "TypeError", "keywords must be strings");
        }
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        let slot = match name_str.as_str() {
            "name" => Some(0usize),
            "bases" => Some(1usize),
            "dict" | "namespace" => Some(2usize),
            _ => None,
        };
        if let Some(idx) = slot {
            if values[idx].is_some() {
                let msg = format!("got multiple values for argument '{name_str}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            values[idx] = Some(val_bits);
        } else {
            extra_pairs.push(name_bits);
            extra_pairs.push(val_bits);
        }
    }
    let names = ["name", "bases", "dict"];
    for (idx, val) in values.iter().enumerate() {
        if val.is_none() {
            let msg = format!("missing required argument '{}'", names[idx]);
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
    }
    let mut out = vec![args.pos[0]];
    for val in values.into_iter().flatten() {
        out.push(val);
    }
    if extra_pairs.is_empty() {
        out.push(MoltObject::none().bits());
        return Some(out);
    }
    let dict_ptr = alloc_dict_with_pairs(_py, &extra_pairs);
    if dict_ptr.is_null() {
        return Some(out);
    }
    out.push(MoltObject::from_ptr(dict_ptr).bits());
    Some(out)
}

unsafe fn bind_builtin_list_sort(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    if args.pos.len() > 1 {
        return raise_exception::<_>(_py, "TypeError", "too many positional arguments");
    }
    let mut key_bits = MoltObject::none().bits();
    let mut reverse_bits = MoltObject::from_bool(false).bits();
    let mut key_set = false;
    let mut reverse_set = false;
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        match name_str.as_str() {
            "key" => {
                if key_set {
                    let msg = format!("got multiple values for argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                key_bits = val_bits;
                key_set = true;
            }
            "reverse" => {
                if reverse_set {
                    let msg = format!("got multiple values for argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                reverse_bits = val_bits;
                reverse_set = true;
            }
            _ => {
                let msg = format!("got an unexpected keyword '{name_str}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
    }
    Some(vec![args.pos[0], key_bits, reverse_bits])
}

unsafe fn bind_builtin_list_pop(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    if !args.kw_names.is_empty() {
        return raise_exception::<_>(
            _py,
            "TypeError",
            "keywords are not supported for this builtin",
        );
    }
    if args.pos.len() > 2 {
        return raise_exception::<_>(_py, "TypeError", "too many positional arguments");
    }
    let mut out = args.pos.clone();
    if out.len() == 1 {
        out.push(MoltObject::none().bits());
    }
    Some(out)
}

unsafe fn bind_builtin_list_index_range(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    if !args.kw_names.is_empty() {
        return raise_exception::<_>(
            _py,
            "TypeError",
            "keywords are not supported for this builtin",
        );
    }
    if args.pos.len() < 2 {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'value'");
    }
    if args.pos.len() > 4 {
        return raise_exception::<_>(_py, "TypeError", "too many positional arguments");
    }
    let mut out = args.pos.clone();
    let missing = missing_bits(_py);
    if out.len() == 2 {
        out.push(missing);
        out.push(missing);
    } else if out.len() == 3 {
        out.push(missing);
    }
    Some(out)
}

unsafe fn bind_builtin_string_find(
    _py: &PyToken<'_>,
    args: &CallArgs,
    func_name: &str,
) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    let positional = args.pos.len().saturating_sub(1);
    if positional > 3 {
        let msg = format!(
            "{func_name}() takes at most 3 arguments ({} given)",
            positional
        );
        return raise_exception::<_>(_py, "TypeError", &msg);
    }
    let mut needle_bits: Option<u64> = None;
    let mut start_bits: Option<u64> = None;
    let mut end_bits: Option<u64> = None;
    let mut saw_sub = false;
    let mut saw_start = false;
    let mut saw_end = false;
    if positional >= 1 {
        needle_bits = Some(args.pos[1]);
        saw_sub = true;
    }
    if positional >= 2 {
        start_bits = Some(args.pos[2]);
        saw_start = true;
    }
    if positional >= 3 {
        end_bits = Some(args.pos[3]);
        saw_end = true;
    }
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        let target = match name_str.as_str() {
            "sub" => (&mut needle_bits, &mut saw_sub),
            "start" => (&mut start_bits, &mut saw_start),
            "end" => (&mut end_bits, &mut saw_end),
            _ => {
                let msg = format!("{func_name}() got an unexpected keyword argument '{name_str}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        };
        if target.0.is_some() {
            let msg = format!(
                "{}() got multiple values for argument '{}'",
                func_name, name_str
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        *target.0 = Some(val_bits);
        *target.1 = true;
    }
    let needle_bits = needle_bits.unwrap_or_else(|| {
        raise_exception::<_>(_py, "TypeError", "missing required argument 'sub'")
    });
    let start_bits = start_bits.unwrap_or_else(|| MoltObject::none().bits());
    let end_bits = end_bits.unwrap_or_else(|| MoltObject::none().bits());
    let has_start_bits = MoltObject::from_bool(saw_start).bits();
    let has_end_bits = MoltObject::from_bool(saw_end).bits();
    Some(vec![
        args.pos[0],
        needle_bits,
        start_bits,
        end_bits,
        has_start_bits,
        has_end_bits,
    ])
}

unsafe fn bind_builtin_count(
    _py: &PyToken<'_>,
    args: &CallArgs,
    func_name: &str,
) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    let positional = args.pos.len().saturating_sub(1);
    if positional > 3 {
        let msg = format!(
            "{func_name}() takes at most 3 arguments ({} given)",
            positional
        );
        return raise_exception::<_>(_py, "TypeError", &msg);
    }
    let mut needle_bits: Option<u64> = None;
    let mut start_bits: Option<u64> = None;
    let mut end_bits: Option<u64> = None;
    let mut saw_sub = false;
    let mut saw_start = false;
    let mut saw_end = false;
    if positional >= 1 {
        needle_bits = Some(args.pos[1]);
        saw_sub = true;
    }
    if positional >= 2 {
        start_bits = Some(args.pos[2]);
        saw_start = true;
    }
    if positional >= 3 {
        end_bits = Some(args.pos[3]);
        saw_end = true;
    }
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        let target = match name_str.as_str() {
            "sub" => (&mut needle_bits, &mut saw_sub),
            "start" => (&mut start_bits, &mut saw_start),
            "end" => (&mut end_bits, &mut saw_end),
            _ => {
                let msg = format!("{func_name}() got an unexpected keyword argument '{name_str}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        };
        if target.0.is_some() {
            let msg = format!(
                "{}() got multiple values for argument '{}'",
                func_name, name_str
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        *target.0 = Some(val_bits);
        *target.1 = true;
    }
    let needle_bits = needle_bits.unwrap_or_else(|| {
        raise_exception::<_>(_py, "TypeError", "missing required argument 'sub'")
    });
    let start_bits = start_bits.unwrap_or_else(|| MoltObject::none().bits());
    let end_bits = end_bits.unwrap_or_else(|| MoltObject::none().bits());
    let has_start_bits = MoltObject::from_bool(saw_start).bits();
    let has_end_bits = MoltObject::from_bool(saw_end).bits();
    Some(vec![
        args.pos[0],
        needle_bits,
        start_bits,
        end_bits,
        has_start_bits,
        has_end_bits,
    ])
}

unsafe fn bind_builtin_split(
    _py: &PyToken<'_>,
    args: &CallArgs,
    func_name: &str,
) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    let positional = args.pos.len().saturating_sub(1);
    if positional > 2 {
        let msg = format!(
            "{func_name}() takes at most 2 arguments ({} given)",
            positional
        );
        return raise_exception::<_>(_py, "TypeError", &msg);
    }
    let mut sep_bits: Option<u64> = None;
    let mut maxsplit_bits: Option<u64> = None;
    let mut saw_sep = false;
    let mut saw_maxsplit = false;
    if positional >= 1 {
        sep_bits = Some(args.pos[1]);
        saw_sep = true;
    }
    if positional >= 2 {
        maxsplit_bits = Some(args.pos[2]);
        saw_maxsplit = true;
    }
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        let target = match name_str.as_str() {
            "sep" => (&mut sep_bits, &mut saw_sep),
            "maxsplit" => (&mut maxsplit_bits, &mut saw_maxsplit),
            _ => {
                let msg = format!("{func_name}() got an unexpected keyword argument '{name_str}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        };
        if target.0.is_some() {
            let msg = format!(
                "{}() got multiple values for argument '{}'",
                func_name, name_str
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        *target.0 = Some(val_bits);
        *target.1 = true;
    }
    let sep_bits = sep_bits.unwrap_or_else(|| MoltObject::none().bits());
    let maxsplit_bits = maxsplit_bits.unwrap_or_else(|| MoltObject::from_int(-1).bits());
    Some(vec![args.pos[0], sep_bits, maxsplit_bits])
}

unsafe fn bind_builtin_splitlines(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    let positional = args.pos.len().saturating_sub(1);
    if positional > 1 {
        let msg = format!(
            "splitlines() takes at most 1 argument ({} given)",
            positional
        );
        return raise_exception::<_>(_py, "TypeError", &msg);
    }
    let mut keepends_bits: Option<u64> = None;
    let mut saw_keepends = false;
    if positional == 1 {
        keepends_bits = Some(args.pos[1]);
        saw_keepends = true;
    }
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        if name_str != "keepends" {
            let msg = format!("'{name_str}' is an invalid keyword argument for splitlines()");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        if saw_keepends {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "splitlines() got multiple values for argument 'keepends'",
            );
        }
        keepends_bits = Some(val_bits);
        saw_keepends = true;
    }
    let keepends_bits = keepends_bits.unwrap_or_else(|| MoltObject::from_bool(false).bits());
    Some(vec![args.pos[0], keepends_bits])
}

unsafe fn bind_builtin_set_multi(
    _py: &PyToken<'_>,
    args: &CallArgs,
    method: &str,
    owner_name: &str,
    owner_type_id: u32,
) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    let self_obj = obj_from_bits(args.pos[0]);
    let mut is_owner = false;
    if let Some(self_ptr) = self_obj.as_ptr() {
        is_owner = object_type_id(self_ptr) == owner_type_id;
    }
    if !is_owner {
        let msg = format!(
            "descriptor '{method}' for '{owner_name}' objects doesn't apply to a '{}' object",
            type_name(_py, self_obj)
        );
        return raise_exception::<_>(_py, "TypeError", &msg);
    }
    if !args.kw_names.is_empty() {
        let msg = format!(
            "{}.{method}() takes no keyword arguments",
            type_name(_py, self_obj)
        );
        return raise_exception::<_>(_py, "TypeError", &msg);
    }
    let tuple_ptr = alloc_tuple(_py, &args.pos[1..]);
    if tuple_ptr.is_null() {
        return None;
    }
    let tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
    Some(vec![args.pos[0], tuple_bits])
}

unsafe fn bind_builtin_set_single(
    _py: &PyToken<'_>,
    args: &CallArgs,
    method: &str,
    owner_name: &str,
    owner_type_id: u32,
) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    let self_obj = obj_from_bits(args.pos[0]);
    let mut is_owner = false;
    if let Some(self_ptr) = self_obj.as_ptr() {
        is_owner = object_type_id(self_ptr) == owner_type_id;
    }
    if !is_owner {
        let msg = format!(
            "descriptor '{method}' for '{owner_name}' objects doesn't apply to a '{}' object",
            type_name(_py, self_obj)
        );
        return raise_exception::<_>(_py, "TypeError", &msg);
    }
    if !args.kw_names.is_empty() {
        let msg = format!(
            "{}.{method}() takes no keyword arguments",
            type_name(_py, self_obj)
        );
        return raise_exception::<_>(_py, "TypeError", &msg);
    }
    let positional = args.pos.len().saturating_sub(1);
    if positional != 1 {
        let msg = format!(
            "{}.{method}() takes exactly one argument ({} given)",
            type_name(_py, self_obj),
            positional
        );
        return raise_exception::<_>(_py, "TypeError", &msg);
    }
    Some(vec![args.pos[0], args.pos[1]])
}

unsafe fn bind_builtin_set_noargs(
    _py: &PyToken<'_>,
    args: &CallArgs,
    method: &str,
    owner_name: &str,
    owner_type_id: u32,
) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    let self_obj = obj_from_bits(args.pos[0]);
    let mut is_owner = false;
    if let Some(self_ptr) = self_obj.as_ptr() {
        is_owner = object_type_id(self_ptr) == owner_type_id;
    }
    if !is_owner {
        let msg = format!(
            "descriptor '{method}' for '{owner_name}' objects doesn't apply to a '{}' object",
            type_name(_py, self_obj)
        );
        return raise_exception::<_>(_py, "TypeError", &msg);
    }
    if !args.kw_names.is_empty() {
        let msg = format!(
            "{}.{method}() takes no keyword arguments",
            type_name(_py, self_obj)
        );
        return raise_exception::<_>(_py, "TypeError", &msg);
    }
    let positional = args.pos.len().saturating_sub(1);
    if positional != 0 {
        let msg = format!(
            "{}.{method}() takes no arguments ({} given)",
            type_name(_py, self_obj),
            positional
        );
        return raise_exception::<_>(_py, "TypeError", &msg);
    }
    Some(vec![args.pos[0]])
}

unsafe fn bind_builtin_prefix_check(
    _py: &PyToken<'_>,
    args: &CallArgs,
    func_name: &str,
    needle_name: &str,
) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    let positional = args.pos.len().saturating_sub(1);
    if positional > 3 {
        let msg = format!(
            "{func_name}() takes at most 3 arguments ({} given)",
            positional
        );
        return raise_exception::<_>(_py, "TypeError", &msg);
    }
    let mut needle_bits: Option<u64> = None;
    let mut start_bits: Option<u64> = None;
    let mut end_bits: Option<u64> = None;
    let mut saw_needle = false;
    let mut saw_start = false;
    let mut saw_end = false;
    if positional >= 1 {
        needle_bits = Some(args.pos[1]);
        saw_needle = true;
    }
    if positional >= 2 {
        start_bits = Some(args.pos[2]);
        saw_start = true;
    }
    if positional >= 3 {
        end_bits = Some(args.pos[3]);
        saw_end = true;
    }
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        let target = match name_str.as_str() {
            "start" => (&mut start_bits, &mut saw_start),
            "end" => (&mut end_bits, &mut saw_end),
            _ if name_str == needle_name => (&mut needle_bits, &mut saw_needle),
            _ => {
                let msg = format!("{func_name}() got an unexpected keyword argument '{name_str}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        };
        if target.0.is_some() {
            let msg = format!(
                "{}() got multiple values for argument '{}'",
                func_name, name_str
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        *target.0 = Some(val_bits);
        *target.1 = true;
    }
    let needle_bits = needle_bits.unwrap_or_else(|| {
        let msg = format!("missing required argument '{needle_name}'");
        raise_exception::<_>(_py, "TypeError", &msg)
    });
    let start_bits = start_bits.unwrap_or_else(|| MoltObject::none().bits());
    let end_bits = end_bits.unwrap_or_else(|| MoltObject::none().bits());
    let has_start_bits = MoltObject::from_bool(saw_start).bits();
    let has_end_bits = MoltObject::from_bool(saw_end).bits();
    Some(vec![
        args.pos[0],
        needle_bits,
        start_bits,
        end_bits,
        has_start_bits,
        has_end_bits,
    ])
}

unsafe fn bind_builtin_string_format(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    let tuple_ptr = alloc_tuple(_py, &args.pos[1..]);
    if tuple_ptr.is_null() {
        return None;
    }
    let tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
    let mut pairs = Vec::with_capacity(args.kw_names.len() * 2);
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "keywords must be strings");
        };
        if object_type_id(name_ptr) != TYPE_ID_STRING {
            return raise_exception::<_>(_py, "TypeError", "keywords must be strings");
        }
        pairs.push(name_bits);
        pairs.push(val_bits);
    }
    let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
    if dict_ptr.is_null() {
        return None;
    }
    let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
    Some(vec![args.pos[0], tuple_bits, dict_bits])
}

unsafe fn bind_builtin_memoryview_cast(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    let provided = args.pos.len().saturating_sub(1);
    if provided == 0 {
        return raise_exception::<_>(
            _py,
            "TypeError",
            "cast() missing required argument 'format' (pos 1)",
        );
    }
    if provided > 2 {
        let msg = format!("cast() takes at most 2 arguments ({provided} given)");
        return raise_exception::<_>(_py, "TypeError", &msg);
    }
    let format_bits = args.pos[1];
    let mut shape_bits: Option<u64> = None;
    if provided == 2 {
        shape_bits = Some(args.pos[2]);
    }
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        if name_str != "shape" {
            let msg = format!("cast() got an unexpected keyword argument '{name_str}'");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        if shape_bits.is_some() {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "cast() got multiple values for argument 'shape'",
            );
        }
        shape_bits = Some(val_bits);
    }
    let (shape_bits, has_shape_bits) = if let Some(bits) = shape_bits {
        (bits, MoltObject::from_bool(true).bits())
    } else {
        (
            MoltObject::none().bits(),
            MoltObject::from_bool(false).bits(),
        )
    };
    Some(vec![args.pos[0], format_bits, shape_bits, has_shape_bits])
}

unsafe fn bind_builtin_file_reconfigure(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    if args.pos.len() > 1 {
        return raise_exception::<_>(
            _py,
            "TypeError",
            "reconfigure() takes no positional arguments",
        );
    }
    let missing = missing_bits(_py);
    let mut encoding_bits = missing;
    let mut errors_bits = missing;
    let mut newline_bits = missing;
    let mut line_buffering_bits = missing;
    let mut write_through_bits = missing;
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        match name_str.as_str() {
            "encoding" => {
                if encoding_bits != missing {
                    let msg = format!("got multiple values for keyword argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                encoding_bits = val_bits;
            }
            "errors" => {
                if errors_bits != missing {
                    let msg = format!("got multiple values for keyword argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                errors_bits = val_bits;
            }
            "newline" => {
                if newline_bits != missing {
                    let msg = format!("got multiple values for keyword argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                newline_bits = val_bits;
            }
            "line_buffering" => {
                if line_buffering_bits != missing {
                    let msg = format!("got multiple values for keyword argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                line_buffering_bits = val_bits;
            }
            "write_through" => {
                if write_through_bits != missing {
                    let msg = format!("got multiple values for keyword argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                write_through_bits = val_bits;
            }
            _ => {
                let msg = format!("reconfigure() got an unexpected keyword argument '{name_str}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
    }
    Some(vec![
        args.pos[0],
        encoding_bits,
        errors_bits,
        newline_bits,
        line_buffering_bits,
        write_through_bits,
    ])
}

unsafe fn bind_builtin_text_codec(
    _py: &PyToken<'_>,
    args: &CallArgs,
    func_name: &str,
) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    let positional = args.pos.len().saturating_sub(1);
    if positional > 2 {
        let msg = format!("{func_name}() takes at most 2 arguments ({positional} given)");
        return raise_exception::<_>(_py, "TypeError", &msg);
    }
    let missing = missing_bits(_py);
    let mut encoding_bits = if positional >= 1 {
        args.pos[1]
    } else {
        missing
    };
    let mut errors_bits = if positional >= 2 {
        args.pos[2]
    } else {
        missing
    };
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        match name_str.as_str() {
            "encoding" => {
                if encoding_bits != missing {
                    let msg = format!("{func_name}() got multiple values for argument 'encoding'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                encoding_bits = val_bits;
            }
            "errors" => {
                if errors_bits != missing {
                    let msg = format!("{func_name}() got multiple values for argument 'errors'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                errors_bits = val_bits;
            }
            _ => {
                let msg = format!("{func_name}() got an unexpected keyword argument '{name_str}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
    }
    Some(vec![args.pos[0], encoding_bits, errors_bits])
}

unsafe fn bind_builtin_pop(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    let mut out = vec![args.pos[0]];
    let mut key: Option<u64> = None;
    let mut default: Option<u64> = None;
    let mut pos_idx = 1usize;
    while pos_idx < args.pos.len() {
        if key.is_none() {
            key = Some(args.pos[pos_idx]);
        } else if default.is_none() {
            default = Some(args.pos[pos_idx]);
        } else {
            return raise_exception::<_>(_py, "TypeError", "too many positional arguments");
        }
        pos_idx += 1;
    }
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        if name_str == "key" {
            if key.is_some() {
                let msg = format!("got multiple values for argument '{name_str}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            key = Some(val_bits);
        } else if name_str == "default" {
            if default.is_some() {
                let msg = format!("got multiple values for argument '{name_str}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            default = Some(val_bits);
        } else {
            let msg = format!("got an unexpected keyword '{name_str}'");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
    }
    let Some(key_bits) = key else {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'key'");
    };
    let (default_bits, has_default) = if let Some(bits) = default {
        (bits, MoltObject::from_int(1).bits())
    } else {
        (MoltObject::none().bits(), MoltObject::from_int(0).bits())
    };
    out.push(key_bits);
    out.push(default_bits);
    out.push(has_default);
    Some(out)
}
