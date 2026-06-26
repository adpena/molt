use super::*;

unsafe fn mappingproxy_mapping_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

unsafe fn mappingproxy_set_mapping_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr as *mut u64) = bits;
    }
}

pub(crate) fn mappingproxy_class(_py: &PyToken<'_>) -> u64 {
    let class_bits = types_class(
        _py,
        &types_state(_py).mappingproxy_class,
        "mappingproxy",
        16,
    );
    let new_bits = builtin_func_bits(
        _py,
        &types_state(_py).mappingproxy_new_fn,
        molt_types_mappingproxy_new as *const () as usize as u64,
        2,
    );
    let init_bits = builtin_func_bits(
        _py,
        &types_state(_py).mappingproxy_init_fn,
        molt_types_mappingproxy_init as *const () as usize as u64,
        2,
    );
    let getitem_bits = builtin_func_bits(
        _py,
        &types_state(_py).mappingproxy_getitem_fn,
        molt_types_mappingproxy_getitem as *const () as usize as u64,
        2,
    );
    let iter_bits = builtin_func_bits(
        _py,
        &types_state(_py).mappingproxy_iter_fn,
        molt_types_mappingproxy_iter as *const () as usize as u64,
        1,
    );
    let len_bits = builtin_func_bits(
        _py,
        &types_state(_py).mappingproxy_len_fn,
        molt_types_mappingproxy_len as *const () as usize as u64,
        1,
    );
    let contains_bits = builtin_func_bits(
        _py,
        &types_state(_py).mappingproxy_contains_fn,
        molt_types_mappingproxy_contains as *const () as usize as u64,
        2,
    );
    let get_bits = builtin_func_bits(
        _py,
        &types_state(_py).mappingproxy_get_fn,
        molt_types_mappingproxy_get as *const () as usize as u64,
        3,
    );
    let keys_bits = builtin_func_bits(
        _py,
        &types_state(_py).mappingproxy_keys_fn,
        molt_types_mappingproxy_keys as *const () as usize as u64,
        1,
    );
    let items_bits = builtin_func_bits(
        _py,
        &types_state(_py).mappingproxy_items_fn,
        molt_types_mappingproxy_items as *const () as usize as u64,
        1,
    );
    let values_bits = builtin_func_bits(
        _py,
        &types_state(_py).mappingproxy_values_fn,
        molt_types_mappingproxy_values as *const () as usize as u64,
        1,
    );
    let repr_bits = builtin_func_bits(
        _py,
        &types_state(_py).mappingproxy_repr_fn,
        molt_types_mappingproxy_repr as *const () as usize as u64,
        1,
    );
    let setitem_bits = builtin_func_bits(
        _py,
        &types_state(_py).mappingproxy_setitem_fn,
        molt_types_mappingproxy_setitem as *const () as usize as u64,
        3,
    );
    let delitem_bits = builtin_func_bits(
        _py,
        &types_state(_py).mappingproxy_delitem_fn,
        molt_types_mappingproxy_delitem as *const () as usize as u64,
        2,
    );
    set_class_method(_py, class_bits, "__new__", new_bits);
    set_class_method(_py, class_bits, "__init__", init_bits);
    set_class_method(_py, class_bits, "__getitem__", getitem_bits);
    set_class_method(_py, class_bits, "__iter__", iter_bits);
    set_class_method(_py, class_bits, "__len__", len_bits);
    set_class_method(_py, class_bits, "__contains__", contains_bits);
    set_class_method(_py, class_bits, "get", get_bits);
    set_class_method(_py, class_bits, "keys", keys_bits);
    set_class_method(_py, class_bits, "items", items_bits);
    set_class_method(_py, class_bits, "values", values_bits);
    set_class_method(_py, class_bits, "__repr__", repr_bits);
    set_class_method(_py, class_bits, "__setitem__", setitem_bits);
    set_class_method(_py, class_bits, "__delitem__", delitem_bits);
    mark_vararg_method(_py, get_bits, true);
    class_bits
}

pub(crate) fn mappingproxy_class_bits(_py: &PyToken<'_>) -> u64 {
    mappingproxy_class(_py)
}

pub(crate) fn method_class(_py: &PyToken<'_>) -> u64 {
    let class_bits = types_class(_py, &types_state(_py).method_class, "method", 16);
    let new_bits = builtin_func_bits(
        _py,
        &types_state(_py).method_new_fn,
        molt_types_method_new as *const () as usize as u64,
        3,
    );
    let init_bits = builtin_func_bits(
        _py,
        &types_state(_py).method_init_fn,
        molt_types_method_init as *const () as usize as u64,
        3,
    );
    set_class_method(_py, class_bits, "__new__", new_bits);
    set_class_method(_py, class_bits, "__init__", init_bits);
    class_bits
}

pub(crate) fn simplenamespace_class(_py: &PyToken<'_>) -> u64 {
    let class_bits = types_class(
        _py,
        &types_state(_py).simplenamespace_class,
        "SimpleNamespace",
        8,
    );
    let init_bits = builtin_func_bits(
        _py,
        &types_state(_py).simplenamespace_init_fn,
        molt_types_simplenamespace_init as *const () as usize as u64,
        3,
    );
    let repr_bits = builtin_func_bits(
        _py,
        &types_state(_py).simplenamespace_repr_fn,
        molt_types_simplenamespace_repr as *const () as usize as u64,
        1,
    );
    let eq_bits = builtin_func_bits(
        _py,
        &types_state(_py).simplenamespace_eq_fn,
        molt_types_simplenamespace_eq as *const () as usize as u64,
        2,
    );
    set_class_method(_py, class_bits, "__init__", init_bits);
    set_class_method(_py, class_bits, "__repr__", repr_bits);
    set_class_method(_py, class_bits, "__eq__", eq_bits);
    mark_vararg_method(_py, init_bits, true);
    class_bits
}

pub(crate) fn capsule_class(_py: &PyToken<'_>) -> u64 {
    let class_bits = types_class(_py, &types_state(_py).capsule_class, "capsule", 8);
    let new_bits = builtin_func_bits(
        _py,
        &types_state(_py).capsule_new_fn,
        molt_types_capsule_new as *const () as usize as u64,
        1,
    );
    set_class_method(_py, class_bits, "__new__", new_bits);
    class_bits
}

pub(crate) fn cell_class(_py: &PyToken<'_>) -> u64 {
    let class_bits = types_class(_py, &types_state(_py).cell_class, "cell", 8);
    let new_bits = builtin_func_bits(
        _py,
        &types_state(_py).cell_new_fn,
        molt_types_cell_new as *const () as usize as u64,
        1,
    );
    set_class_method(_py, class_bits, "__new__", new_bits);
    class_bits
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_method_new(_cls_bits: u64, func_bits: u64, self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if obj_from_bits(self_bits).is_none() {
            inc_ref_bits(_py, func_bits);
            return func_bits;
        }
        crate::molt_bound_method_new(func_bits, self_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_method_init(_self_bits: u64, _func_bits: u64, _self_arg: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { MoltObject::none().bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_mappingproxy_new(cls_bits: u64, mapping_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if obj_from_bits(mapping_bits).is_none() {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "mappingproxy() argument cannot be None",
            );
        }
        let cls_obj = obj_from_bits(cls_bits);
        let Some(cls_ptr) = cls_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "mappingproxy() expects type");
        };
        unsafe {
            if object_type_id(cls_ptr) != TYPE_ID_TYPE {
                return raise_exception::<_>(_py, "TypeError", "mappingproxy() expects type");
            }
        }
        let inst_bits = unsafe { alloc_instance_for_class(_py, cls_ptr) };
        if obj_from_bits(inst_bits).is_none() {
            return MoltObject::none().bits();
        }
        let inst_ptr = obj_from_bits(inst_bits).as_ptr().unwrap();
        unsafe {
            mappingproxy_set_mapping_bits(inst_ptr, mapping_bits);
        }
        inc_ref_bits(_py, mapping_bits);
        inst_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_mappingproxy_init(_self_bits: u64, _mapping_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { MoltObject::none().bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_mappingproxy_getitem(self_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let mapping_bits = unsafe { mappingproxy_mapping_bits(self_ptr) };
        molt_index(mapping_bits, key_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_mappingproxy_iter(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let mapping_bits = unsafe { mappingproxy_mapping_bits(self_ptr) };
        let iter_bits = molt_iter(mapping_bits);
        if obj_from_bits(iter_bits).is_none() {
            return raise_not_iterable(_py, mapping_bits);
        }
        iter_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_mappingproxy_len(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let mapping_bits = unsafe { mappingproxy_mapping_bits(self_ptr) };
        molt_len(mapping_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_mappingproxy_contains(self_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let mapping_bits = unsafe { mappingproxy_mapping_bits(self_ptr) };
        molt_contains(mapping_bits, key_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_mappingproxy_get(
    self_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let args_ptr = obj_from_bits(args_bits).as_ptr();
        let Some(args_ptr) = args_ptr else {
            return raise_exception::<_>(_py, "TypeError", "mappingproxy.get() expects arguments");
        };
        unsafe {
            if object_type_id(args_ptr) != TYPE_ID_TUPLE {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "mappingproxy.get() expects arguments",
                );
            }
        }
        let args = unsafe { seq_vec_ref(args_ptr) };
        if args.is_empty() || args.len() > 2 {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "mappingproxy.get() takes 1 or 2 arguments",
            );
        }
        if let Some(kwargs_ptr) = obj_from_bits(kwargs_bits).as_ptr() {
            unsafe {
                if object_type_id(kwargs_ptr) == TYPE_ID_DICT {
                    let order = dict_order(kwargs_ptr);
                    if !order.is_empty() {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "mappingproxy.get() takes no keyword arguments",
                        );
                    }
                }
            }
        }
        let key_bits = args[0];
        let default_bits = if args.len() == 2 {
            args[1]
        } else {
            MoltObject::none().bits()
        };
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let mapping_bits = unsafe { mappingproxy_mapping_bits(self_ptr) };
        // mappingproxy instances in Molt always wrap a class dict, so route to
        // direct dict.get semantics to avoid descriptor re-resolution.
        let Some(mapping_ptr) = obj_from_bits(mapping_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "mappingproxy backing store is invalid");
        };
        unsafe {
            if object_type_id(mapping_ptr) != TYPE_ID_DICT {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "mappingproxy backing store must be a dict",
                );
            }
        }
        molt_dict_get(mapping_bits, key_bits, default_bits)
    })
}

fn mappingproxy_call_noargs(_py: &PyToken<'_>, self_bits: u64, name: &str) -> u64 {
    let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
    let mapping_bits = unsafe { mappingproxy_mapping_bits(self_ptr) };
    let missing = missing_bits(_py);
    let name_ptr = alloc_string(_py, name.as_bytes());
    if name_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let name_bits = MoltObject::from_ptr(name_ptr).bits();
    let method_bits = molt_getattr_builtin(mapping_bits, name_bits, missing);
    dec_ref_bits(_py, name_bits);
    if exception_pending(_py) {
        return MoltObject::none().bits();
    }
    if method_bits == missing {
        return raise_exception::<_>(_py, "AttributeError", name);
    }
    let res_bits = unsafe { call_callable0(_py, method_bits) };
    dec_ref_bits(_py, method_bits);
    res_bits
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_mappingproxy_keys(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { mappingproxy_call_noargs(_py, self_bits, "keys") })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_mappingproxy_items(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { mappingproxy_call_noargs(_py, self_bits, "items") })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_mappingproxy_values(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { mappingproxy_call_noargs(_py, self_bits, "values") })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_mappingproxy_repr(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let mapping_bits = unsafe { mappingproxy_mapping_bits(self_ptr) };
        let mapping_repr_bits = molt_repr_from_obj(mapping_bits);
        let mapping_repr =
            string_obj_to_owned(obj_from_bits(mapping_repr_bits)).unwrap_or_default();
        dec_ref_bits(_py, mapping_repr_bits);
        let out = format!("mappingproxy({mapping_repr})");
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_mappingproxy_setitem(
    _self_bits: u64,
    _key_bits: u64,
    _val_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<_>(
            _py,
            "TypeError",
            "'mappingproxy' object does not support item assignment",
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_mappingproxy_delitem(_self_bits: u64, _key_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<_>(
            _py,
            "TypeError",
            "'mappingproxy' object does not support item deletion",
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_capsule_new(_cls_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<_>(_py, "TypeError", "cannot create 'capsule' instances")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_cell_new(_cls_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<_>(_py, "TypeError", "cannot create 'cell' instances")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_simplenamespace_init(
    self_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let args_ptr = obj_from_bits(args_bits).as_ptr();
        let args = if let Some(args_ptr) = args_ptr {
            unsafe {
                if object_type_id(args_ptr) != TYPE_ID_TUPLE {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "SimpleNamespace expects arguments",
                    );
                }
                seq_vec_ref(args_ptr).clone()
            }
        } else {
            Vec::new()
        };
        if !args.is_empty() {
            return raise_exception::<_>(_py, "TypeError", "no positional arguments expected");
        }
        let dict_ptr = alloc_dict_with_pairs(_py, &[]);
        if dict_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
        if let Some(kwargs_ptr) = obj_from_bits(kwargs_bits).as_ptr() {
            unsafe {
                if object_type_id(kwargs_ptr) == TYPE_ID_DICT {
                    let _ =
                        dict_update_apply(_py, dict_bits, dict_update_set_in_place, kwargs_bits);
                    if exception_pending(_py) {
                        dec_ref_bits(_py, dict_bits);
                        return MoltObject::none().bits();
                    }
                }
            }
        }
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
            unsafe {
                if object_type_id(dict_ptr) != TYPE_ID_DICT {
                    dec_ref_bits(_py, dict_bits);
                    return MoltObject::none().bits();
                }
                let order = dict_order(dict_ptr);
                let mut idx = 0;
                while idx + 1 < order.len() {
                    let key_bits = order[idx];
                    let val_bits = order[idx + 1];
                    let Some(key_ptr) = obj_from_bits(key_bits).as_ptr() else {
                        dec_ref_bits(_py, dict_bits);
                        return MoltObject::none().bits();
                    };
                    if object_type_id(key_ptr) != TYPE_ID_STRING {
                        dec_ref_bits(_py, dict_bits);
                        return raise_exception::<_>(_py, "TypeError", "keywords must be strings");
                    }
                    let _ = molt_object_setattr(self_bits, key_bits, val_bits);
                    if exception_pending(_py) {
                        dec_ref_bits(_py, dict_bits);
                        return MoltObject::none().bits();
                    }
                    idx += 2;
                }
            }
        }
        dec_ref_bits(_py, dict_bits);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_simplenamespace_repr(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let mut out = String::from("namespace(");
        let dict_bits = unsafe { instance_dict_bits(self_ptr) };
        if dict_bits != 0 && !obj_from_bits(dict_bits).is_none() {
            let dict_ptr = obj_from_bits(dict_bits).as_ptr();
            if let Some(dict_ptr) = dict_ptr {
                unsafe {
                    if object_type_id(dict_ptr) == TYPE_ID_DICT {
                        let order = dict_order(dict_ptr);
                        let mut idx = 0;
                        let mut first = true;
                        while idx + 1 < order.len() {
                            let key_bits = order[idx];
                            let val_bits = order[idx + 1];
                            let key_str = string_obj_to_owned(obj_from_bits(key_bits))
                                .unwrap_or_else(|| "<key>".to_string());
                            let val_repr_bits = molt_repr_from_obj(val_bits);
                            let val_repr = string_obj_to_owned(obj_from_bits(val_repr_bits))
                                .unwrap_or_default();
                            dec_ref_bits(_py, val_repr_bits);
                            if !first {
                                out.push_str(", ");
                            }
                            first = false;
                            out.push_str(&key_str);
                            out.push('=');
                            out.push_str(&val_repr);
                            idx += 2;
                        }
                    }
                }
            }
        }
        out.push(')');
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_simplenamespace_eq(self_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let other_ptr = obj_from_bits(other_bits).as_ptr();
        let Some(other_ptr) = other_ptr else {
            return crate::builtins::methods::not_implemented_bits(_py);
        };
        let self_class = unsafe { object_class_bits(self_ptr) };
        let other_class = unsafe { object_class_bits(other_ptr) };
        if self_class == 0 || other_class == 0 || self_class != other_class {
            return crate::builtins::methods::not_implemented_bits(_py);
        }
        let self_dict_bits = unsafe { instance_dict_bits(self_ptr) };
        let other_dict_bits = unsafe { instance_dict_bits(other_ptr) };
        if self_dict_bits == 0 && other_dict_bits == 0 {
            return MoltObject::from_bool(true).bits();
        }
        let mut created = Vec::new();
        let left_bits = if self_dict_bits == 0 {
            let dict_ptr = alloc_dict_with_pairs(_py, &[]);
            if dict_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let bits = MoltObject::from_ptr(dict_ptr).bits();
            created.push(bits);
            bits
        } else {
            self_dict_bits
        };
        let right_bits = if other_dict_bits == 0 {
            let dict_ptr = alloc_dict_with_pairs(_py, &[]);
            if dict_ptr.is_null() {
                for bits in created.iter() {
                    dec_ref_bits(_py, *bits);
                }
                return MoltObject::none().bits();
            }
            let bits = MoltObject::from_ptr(dict_ptr).bits();
            created.push(bits);
            bits
        } else {
            other_dict_bits
        };
        let eq_bits = molt_eq(left_bits, right_bits);
        for bits in created.iter() {
            dec_ref_bits(_py, *bits);
        }
        eq_bits
    })
}

pub(crate) fn types_drop_instance(_py: &PyToken<'_>, ptr: *mut u8) -> bool {
    let class_bits = unsafe { object_class_bits(ptr) };
    if class_bits == 0 {
        return false;
    }
    let mappingproxy = types_state(_py).mappingproxy_class.load(Ordering::Acquire);
    if class_bits == mappingproxy {
        let mapping_bits = unsafe { mappingproxy_mapping_bits(ptr) };
        if mapping_bits != 0 && !obj_from_bits(mapping_bits).is_none() {
            dec_ref_bits(_py, mapping_bits);
        }
        return true;
    }
    false
}
