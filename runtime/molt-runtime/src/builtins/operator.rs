use std::sync::atomic::AtomicU64;

use molt_obj_model::MoltObject;

use crate::builtins::numbers::{index_bigint_from_obj, int_bits_from_bigint};
use crate::{
    PyToken, TYPE_ID_DICT, TYPE_ID_STRING, TYPE_ID_TUPLE, alloc_class_obj, alloc_dict_with_pairs,
    alloc_function_obj, alloc_string, alloc_tuple, attr_lookup_ptr_allow_missing,
    attr_name_bits_from_bytes, bigint_bits, bigint_ptr_from_bits, bigint_ref, bigint_to_inline,
    builtin_classes, call_callable0, class_dict_bits, class_name_for_error, complex_bits,
    complex_from_obj_strict, complex_ptr_from_bits, dec_ref_bits, dict_set_in_place,
    exception_pending, inc_ref_bits, init_atomic_bits, int_bits_from_i128, intern_static_name,
    is_truthy, molt_abs_builtin, molt_add, molt_bit_and, molt_bit_or, molt_bit_xor,
    molt_class_set_base, molt_concat, molt_contains, molt_delitem_method, molt_div,
    molt_eq, molt_exception_clear, molt_floordiv, molt_ge, molt_getattr_builtin,
    molt_getitem_method, molt_gt, molt_inplace_add, molt_inplace_bit_and, molt_inplace_bit_or,
    molt_inplace_bit_xor, molt_inplace_concat, molt_inplace_div, molt_inplace_floordiv,
    molt_inplace_lshift, molt_inplace_matmul, molt_inplace_mod, molt_inplace_mul,
    molt_inplace_pow, molt_inplace_rshift, molt_inplace_sub, molt_index, molt_invert,
    molt_is_truthy, molt_iter_checked, molt_iter_next, molt_le, molt_len, molt_lshift, molt_lt,
    molt_matmul, molt_mod, molt_mul, molt_ne, molt_pow, molt_rshift, molt_setitem_method,
    molt_sub, obj_from_bits, object_class_bits, object_set_class_bits, object_type_id,
    raise_exception, seq_vec_ref, string_obj_to_owned, to_bigint, to_i64, type_name, type_of_bits,
};

static ITEMGETTER_CLASS: AtomicU64 = AtomicU64::new(0);
static ATTRGETTER_CLASS: AtomicU64 = AtomicU64::new(0);
static METHODCALLER_CLASS: AtomicU64 = AtomicU64::new(0);

static ITEMGETTER_CALL: AtomicU64 = AtomicU64::new(0);
static ATTRGETTER_CALL: AtomicU64 = AtomicU64::new(0);
static METHODCALLER_CALL: AtomicU64 = AtomicU64::new(0);

static ITEMGETTER_INIT: AtomicU64 = AtomicU64::new(0);
static ATTRGETTER_INIT: AtomicU64 = AtomicU64::new(0);
static METHODCALLER_INIT: AtomicU64 = AtomicU64::new(0);

fn operator_class(
    _py: &PyToken<'_>,
    slot: &AtomicU64,
    name: &str,
    layout_size: i64,
    call_slot: &AtomicU64,
    call_fn: u64,
) -> u64 {
    init_atomic_bits(_py, slot, || {
        let name_ptr = alloc_string(_py, name.as_bytes());
        if name_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let class_ptr = alloc_class_obj(_py, name_bits);
        dec_ref_bits(_py, name_bits);
        if class_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let class_bits = MoltObject::from_ptr(class_ptr).bits();
        let builtins = builtin_classes(_py);
        unsafe {
            if let Some(ptr) = obj_from_bits(class_bits).as_ptr() {
                object_set_class_bits(_py, ptr, builtins.type_obj);
                inc_ref_bits(_py, builtins.type_obj);
            }
        }
        let _ = molt_class_set_base(class_bits, builtins.object);
        let dict_bits = unsafe { class_dict_bits(class_ptr) };
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
            && unsafe { object_type_id(dict_ptr) } == TYPE_ID_DICT
        {
            let layout_name = intern_static_name(
                _py,
                &crate::runtime_state(_py).interned.molt_layout_size,
                b"__molt_layout_size__",
            );
            let layout_bits = MoltObject::from_int(layout_size).bits();
            let call_bits = builtin_func_bits(_py, call_slot, call_fn, 2);
            let call_name = intern_static_name(
                _py,
                &crate::runtime_state(_py).interned.call_name,
                b"__call__",
            );
            unsafe {
                dict_set_in_place(_py, dict_ptr, layout_name, layout_bits);
                dict_set_in_place(_py, dict_ptr, call_name, call_bits);
            }
        }
        class_bits
    })
}

fn builtin_func_bits(_py: &PyToken<'_>, slot: &AtomicU64, fn_ptr: u64, arity: u64) -> u64 {
    init_atomic_bits(_py, slot, || {
        let ptr = alloc_function_obj(_py, fn_ptr, arity);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            unsafe {
                let builtin_bits = builtin_classes(_py).builtin_function_or_method;
                let old_bits = object_class_bits(ptr);
                if old_bits != builtin_bits {
                    if old_bits != 0 {
                        dec_ref_bits(_py, old_bits);
                    }
                    object_set_class_bits(_py, ptr, builtin_bits);
                    inc_ref_bits(_py, builtin_bits);
                }
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

fn set_class_method(_py: &PyToken<'_>, class_bits: u64, name: &[u8], func_bits: u64) {
    let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
        return;
    };
    let dict_bits = unsafe { class_dict_bits(class_ptr) };
    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
        return;
    };
    if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
        return;
    }
    let Some(name_bits) = attr_name_bits_from_bytes(_py, name) else {
        return;
    };
    unsafe {
        dict_set_in_place(_py, dict_ptr, name_bits, func_bits);
    }
    dec_ref_bits(_py, name_bits);
}

fn mark_vararg(
    _py: &PyToken<'_>,
    func_bits: u64,
    arg_names: &[&[u8]],
    has_vararg: bool,
    has_varkw: bool,
) {
    let Some(func_ptr) = obj_from_bits(func_bits).as_ptr() else {
        return;
    };
    let dict_ptr = alloc_dict_with_pairs(_py, &[]);
    if dict_ptr.is_null() {
        return;
    }
    let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
    unsafe {
        crate::function_set_dict_bits(func_ptr, dict_bits);
    }
    let arg_names_name = intern_static_name(
        _py,
        &crate::runtime_state(_py).interned.molt_arg_names,
        b"__molt_arg_names__",
    );
    if !arg_names.is_empty() {
        let mut arg_bits: Vec<u64> = Vec::with_capacity(arg_names.len());
        for name in arg_names.iter().copied() {
            let name_ptr = alloc_string(_py, name);
            if name_ptr.is_null() {
                return;
            }
            arg_bits.push(MoltObject::from_ptr(name_ptr).bits());
        }
        let arg_names_ptr = alloc_tuple(_py, &arg_bits);
        for bits in arg_bits.iter().copied() {
            dec_ref_bits(_py, bits);
        }
        if !arg_names_ptr.is_null() {
            let arg_names_bits = MoltObject::from_ptr(arg_names_ptr).bits();
            unsafe {
                dict_set_in_place(_py, dict_ptr, arg_names_name, arg_names_bits);
            }
            dec_ref_bits(_py, arg_names_bits);
        }
    }
    if has_vararg {
        let vararg_name = intern_static_name(
            _py,
            &crate::runtime_state(_py).interned.molt_vararg,
            b"__molt_vararg__",
        );
        unsafe {
            dict_set_in_place(_py, dict_ptr, vararg_name, MoltObject::from_bool(true).bits());
        }
    }
    if has_varkw {
        let varkw_name = intern_static_name(
            _py,
            &crate::runtime_state(_py).interned.molt_varkw,
            b"__molt_varkw__",
        );
        unsafe {
            dict_set_in_place(_py, dict_ptr, varkw_name, MoltObject::from_bool(true).bits());
        }
    }
}

unsafe fn itemgetter_items_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

unsafe fn itemgetter_set_items_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr as *mut u64) = bits;
    }
}

unsafe fn attrgetter_attrs_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

unsafe fn attrgetter_set_attrs_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr as *mut u64) = bits;
    }
}

unsafe fn methodcaller_name_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr as *const u64) }
}

unsafe fn methodcaller_args_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(std::mem::size_of::<u64>()) as *const u64) }
}

unsafe fn methodcaller_kwargs_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(2 * std::mem::size_of::<u64>()) as *const u64) }
}

unsafe fn methodcaller_set_name_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr as *mut u64) = bits;
    }
}

unsafe fn methodcaller_set_args_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr.add(std::mem::size_of::<u64>()) as *mut u64) = bits;
    }
}

unsafe fn methodcaller_set_kwargs_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr.add(2 * std::mem::size_of::<u64>()) as *mut u64) = bits;
    }
}

fn itemgetter_class(_py: &PyToken<'_>) -> u64 {
    let class_bits = operator_class(
        _py,
        &ITEMGETTER_CLASS,
        "itemgetter",
        16,
        &ITEMGETTER_CALL,
        crate::molt_operator_itemgetter_call as *const () as usize as u64,
    );
    let init_bits = builtin_func_bits(
        _py,
        &ITEMGETTER_INIT,
        crate::molt_operator_itemgetter_init as *const () as usize as u64,
        2,
    );
    mark_vararg(_py, init_bits, &[b"self"], true, false);
    set_class_method(_py, class_bits, b"__init__", init_bits);
    class_bits
}

fn attrgetter_class(_py: &PyToken<'_>) -> u64 {
    let class_bits = operator_class(
        _py,
        &ATTRGETTER_CLASS,
        "attrgetter",
        16,
        &ATTRGETTER_CALL,
        crate::molt_operator_attrgetter_call as *const () as usize as u64,
    );
    let init_bits = builtin_func_bits(
        _py,
        &ATTRGETTER_INIT,
        crate::molt_operator_attrgetter_init as *const () as usize as u64,
        2,
    );
    mark_vararg(_py, init_bits, &[b"self"], true, false);
    set_class_method(_py, class_bits, b"__init__", init_bits);
    class_bits
}

fn methodcaller_class(_py: &PyToken<'_>) -> u64 {
    let class_bits = operator_class(
        _py,
        &METHODCALLER_CLASS,
        "methodcaller",
        32,
        &METHODCALLER_CALL,
        crate::molt_operator_methodcaller_call as *const () as usize as u64,
    );
    let init_bits = builtin_func_bits(
        _py,
        &METHODCALLER_INIT,
        crate::molt_operator_methodcaller_init as *const () as usize as u64,
        4,
    );
    mark_vararg(_py, init_bits, &[b"self", b"name"], true, true);
    set_class_method(_py, class_bits, b"__init__", init_bits);
    class_bits
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_index(obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let err = format!(
            "'{}' object cannot be interpreted as an integer",
            type_name(_py, obj_from_bits(obj_bits))
        );
        let Some(value) = index_bigint_from_obj(_py, obj_bits, &err) else {
            return MoltObject::none().bits();
        };
        int_bits_from_bigint(_py, value)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_itemgetter_init(self_bits: u64, items_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let items_obj = obj_from_bits(items_bits);
        let Some(items_ptr) = items_obj.as_ptr() else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "itemgetter expected at least 1 argument",
            );
        };
        unsafe {
            if object_type_id(items_ptr) != TYPE_ID_TUPLE {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "itemgetter expected at least 1 argument",
                );
            }
            let items = seq_vec_ref(items_ptr);
            if items.is_empty() {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "itemgetter expected at least 1 argument",
                );
            }
        }
        let Some(self_ptr) = obj_from_bits(self_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            itemgetter_set_items_bits(self_ptr, items_bits);
        }
        inc_ref_bits(_py, items_bits);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_attrgetter_init(self_bits: u64, attrs_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let attrs_obj = obj_from_bits(attrs_bits);
        let Some(attrs_ptr) = attrs_obj.as_ptr() else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "attrgetter expected at least 1 argument",
            );
        };
        unsafe {
            if object_type_id(attrs_ptr) != TYPE_ID_TUPLE {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "attrgetter expected at least 1 argument",
                );
            }
            let attrs = seq_vec_ref(attrs_ptr);
            if attrs.is_empty() {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "attrgetter expected at least 1 argument",
                );
            }
            for &attr_bits in attrs.iter() {
                let Some(attr_ptr) = obj_from_bits(attr_bits).as_ptr() else {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "attrgetter expects string attributes",
                    );
                };
                if object_type_id(attr_ptr) != TYPE_ID_STRING {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "attrgetter expects string attributes",
                    );
                }
            }
        }
        let Some(self_ptr) = obj_from_bits(self_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            attrgetter_set_attrs_bits(self_ptr, attrs_bits);
        }
        inc_ref_bits(_py, attrs_bits);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_methodcaller_init(
    self_bits: u64,
    name_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "methodcaller() name must be str");
        };
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "methodcaller() name must be str");
            }
        }
        let Some(self_ptr) = obj_from_bits(self_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            methodcaller_set_name_bits(self_ptr, name_bits);
            methodcaller_set_args_bits(self_ptr, args_bits);
            methodcaller_set_kwargs_bits(self_ptr, kwargs_bits);
        }
        inc_ref_bits(_py, name_bits);
        inc_ref_bits(_py, args_bits);
        if kwargs_bits != 0 && !obj_from_bits(kwargs_bits).is_none() {
            inc_ref_bits(_py, kwargs_bits);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_itemgetter(items_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let items_obj = obj_from_bits(items_bits);
        let Some(items_ptr) = items_obj.as_ptr() else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "itemgetter expected at least 1 argument",
            );
        };
        unsafe {
            if object_type_id(items_ptr) != TYPE_ID_TUPLE {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "itemgetter expected at least 1 argument",
                );
            }
            let items = seq_vec_ref(items_ptr);
            if items.is_empty() {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "itemgetter expected at least 1 argument",
                );
            }
        }
        let class_bits = itemgetter_class(_py);
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        let inst_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        if obj_from_bits(inst_bits).is_none() {
            return MoltObject::none().bits();
        }
        let inst_ptr = obj_from_bits(inst_bits).as_ptr().unwrap();
        unsafe {
            itemgetter_set_items_bits(inst_ptr, items_bits);
        }
        inc_ref_bits(_py, items_bits);
        inst_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_itemgetter_type() -> u64 {
    crate::with_gil_entry!(_py, { itemgetter_class(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_attrgetter(attrs_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let attrs_obj = obj_from_bits(attrs_bits);
        let Some(attrs_ptr) = attrs_obj.as_ptr() else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "attrgetter expected at least 1 argument",
            );
        };
        unsafe {
            if object_type_id(attrs_ptr) != TYPE_ID_TUPLE {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "attrgetter expected at least 1 argument",
                );
            }
            let attrs = seq_vec_ref(attrs_ptr);
            if attrs.is_empty() {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "attrgetter expected at least 1 argument",
                );
            }
            for &attr_bits in attrs.iter() {
                let Some(attr_ptr) = obj_from_bits(attr_bits).as_ptr() else {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "attrgetter expects string attributes",
                    );
                };
                if object_type_id(attr_ptr) != TYPE_ID_STRING {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "attrgetter expects string attributes",
                    );
                }
            }
        }
        let class_bits = attrgetter_class(_py);
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        let inst_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        if obj_from_bits(inst_bits).is_none() {
            return MoltObject::none().bits();
        }
        let inst_ptr = obj_from_bits(inst_bits).as_ptr().unwrap();
        unsafe {
            attrgetter_set_attrs_bits(inst_ptr, attrs_bits);
        }
        inc_ref_bits(_py, attrs_bits);
        inst_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_attrgetter_type() -> u64 {
    crate::with_gil_entry!(_py, { attrgetter_class(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_methodcaller(
    name_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "methodcaller() name must be str");
        };
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "methodcaller() name must be str");
            }
        }
        let class_bits = methodcaller_class(_py);
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        let inst_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        if obj_from_bits(inst_bits).is_none() {
            return MoltObject::none().bits();
        }
        let inst_ptr = obj_from_bits(inst_bits).as_ptr().unwrap();
        unsafe {
            methodcaller_set_name_bits(inst_ptr, name_bits);
            methodcaller_set_args_bits(inst_ptr, args_bits);
            methodcaller_set_kwargs_bits(inst_ptr, kwargs_bits);
        }
        inc_ref_bits(_py, name_bits);
        inc_ref_bits(_py, args_bits);
        if kwargs_bits != 0 && !obj_from_bits(kwargs_bits).is_none() {
            inc_ref_bits(_py, kwargs_bits);
        }
        inst_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_methodcaller_type() -> u64 {
    crate::with_gil_entry!(_py, { methodcaller_class(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_itemgetter_call(self_bits: u64, obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let items_bits = unsafe { itemgetter_items_bits(self_ptr) };
        let items_ptr = obj_from_bits(items_bits).as_ptr();
        let Some(items_ptr) = items_ptr else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(items_ptr) != TYPE_ID_TUPLE {
                return MoltObject::none().bits();
            }
            let items = seq_vec_ref(items_ptr);
            if items.len() == 1 {
                let res_bits = molt_index(obj_bits, items[0]);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                return res_bits;
            }
            let mut out: Vec<u64> = Vec::with_capacity(items.len());
            for &item_bits in items.iter() {
                let res_bits = molt_index(obj_bits, item_bits);
                if exception_pending(_py) {
                    for bits in out.iter() {
                        dec_ref_bits(_py, *bits);
                    }
                    return MoltObject::none().bits();
                }
                out.push(res_bits);
            }
            let tuple_ptr = alloc_tuple(_py, out.as_slice());
            if tuple_ptr.is_null() {
                for bits in out.iter() {
                    dec_ref_bits(_py, *bits);
                }
                return MoltObject::none().bits();
            }
            for bits in out.iter() {
                dec_ref_bits(_py, *bits);
            }
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

fn resolve_attr_path(_py: &PyToken<'_>, obj_bits: u64, name: &str) -> u64 {
    let missing = crate::missing_bits(_py);
    let mut current_bits = obj_bits;
    let mut current_owned = false;
    for part in name.split('.') {
        let part_ptr = alloc_string(_py, part.as_bytes());
        if part_ptr.is_null() {
            if current_owned {
                dec_ref_bits(_py, current_bits);
            }
            return MoltObject::none().bits();
        }
        let part_bits = MoltObject::from_ptr(part_ptr).bits();
        let next_bits = molt_getattr_builtin(current_bits, part_bits, missing);
        dec_ref_bits(_py, part_bits);
        if current_owned {
            dec_ref_bits(_py, current_bits);
        }
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if next_bits == missing {
            return raise_exception::<u64>(_py, "AttributeError", name);
        }
        current_bits = next_bits;
        current_owned = true;
    }
    if current_owned {
        current_bits
    } else {
        MoltObject::none().bits()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_attrgetter_call(self_bits: u64, obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let attrs_bits = unsafe { attrgetter_attrs_bits(self_ptr) };
        let attrs_ptr = obj_from_bits(attrs_bits).as_ptr();
        let Some(attrs_ptr) = attrs_ptr else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(attrs_ptr) != TYPE_ID_TUPLE {
                return MoltObject::none().bits();
            }
            let attrs = seq_vec_ref(attrs_ptr);
            if attrs.len() == 1 {
                let name = string_obj_to_owned(obj_from_bits(attrs[0])).unwrap_or_default();
                let res_bits = resolve_attr_path(_py, obj_bits, &name);
                return res_bits;
            }
            let mut out: Vec<u64> = Vec::with_capacity(attrs.len());
            for &attr_bits in attrs.iter() {
                let name = string_obj_to_owned(obj_from_bits(attr_bits)).unwrap_or_default();
                let res_bits = resolve_attr_path(_py, obj_bits, &name);
                if exception_pending(_py) {
                    for bits in out.iter() {
                        dec_ref_bits(_py, *bits);
                    }
                    return MoltObject::none().bits();
                }
                out.push(res_bits);
            }
            let tuple_ptr = alloc_tuple(_py, out.as_slice());
            if tuple_ptr.is_null() {
                for bits in out.iter() {
                    dec_ref_bits(_py, *bits);
                }
                return MoltObject::none().bits();
            }
            for bits in out.iter() {
                dec_ref_bits(_py, *bits);
            }
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_methodcaller_call(self_bits: u64, obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let name_bits = unsafe { methodcaller_name_bits(self_ptr) };
        let args_bits = unsafe { methodcaller_args_bits(self_ptr) };
        let kwargs_bits = unsafe { methodcaller_kwargs_bits(self_ptr) };
        let missing = crate::missing_bits(_py);
        let method_bits = molt_getattr_builtin(obj_bits, name_bits, missing);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if method_bits == missing {
            let name = string_obj_to_owned(obj_from_bits(name_bits)).unwrap_or_default();
            return raise_exception::<u64>(_py, "AttributeError", &name);
        }
        let args_ptr = obj_from_bits(args_bits).as_ptr();
        let mut arg_list: Vec<u64> = Vec::new();
        if let Some(args_ptr) = args_ptr {
            unsafe {
                if object_type_id(args_ptr) == TYPE_ID_TUPLE {
                    arg_list = seq_vec_ref(args_ptr).clone();
                }
            }
        }
        let kw_ptr = obj_from_bits(kwargs_bits).as_ptr();
        let mut kw_pairs: Vec<(u64, u64)> = Vec::new();
        if let Some(kw_ptr) = kw_ptr {
            unsafe {
                if object_type_id(kw_ptr) == TYPE_ID_DICT {
                    let order = crate::dict_order(kw_ptr);
                    let mut idx = 0;
                    while idx + 1 < order.len() {
                        kw_pairs.push((order[idx], order[idx + 1]));
                        idx += 2;
                    }
                }
            }
        }
        let builder_bits = crate::molt_callargs_new(arg_list.len() as u64, kw_pairs.len() as u64);
        if builder_bits == 0 {
            return MoltObject::none().bits();
        }
        for &arg_bits in arg_list.iter() {
            unsafe {
                let _ = crate::molt_callargs_push_pos(builder_bits, arg_bits);
            }
        }
        for (name_bits, val_bits) in kw_pairs.iter() {
            unsafe {
                let _ = crate::molt_callargs_push_kw(builder_bits, *name_bits, *val_bits);
            }
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
        }
        crate::molt_call_bind(method_bits, builder_bits)
    })
}

pub(crate) fn operator_drop_instance(_py: &PyToken<'_>, ptr: *mut u8) -> bool {
    let class_bits = unsafe { object_class_bits(ptr) };
    if class_bits == 0 {
        return false;
    }
    let item_class = ITEMGETTER_CLASS.load(std::sync::atomic::Ordering::Acquire);
    if class_bits == item_class {
        let items_bits = unsafe { itemgetter_items_bits(ptr) };
        if items_bits != 0 && !obj_from_bits(items_bits).is_none() {
            dec_ref_bits(_py, items_bits);
        }
        return true;
    }
    let attr_class = ATTRGETTER_CLASS.load(std::sync::atomic::Ordering::Acquire);
    if class_bits == attr_class {
        let attrs_bits = unsafe { attrgetter_attrs_bits(ptr) };
        if attrs_bits != 0 && !obj_from_bits(attrs_bits).is_none() {
            dec_ref_bits(_py, attrs_bits);
        }
        return true;
    }
    let method_class = METHODCALLER_CLASS.load(std::sync::atomic::Ordering::Acquire);
    if class_bits == method_class {
        let name_bits = unsafe { methodcaller_name_bits(ptr) };
        let args_bits = unsafe { methodcaller_args_bits(ptr) };
        let kwargs_bits = unsafe { methodcaller_kwargs_bits(ptr) };
        if name_bits != 0 && !obj_from_bits(name_bits).is_none() {
            dec_ref_bits(_py, name_bits);
        }
        if args_bits != 0 && !obj_from_bits(args_bits).is_none() {
            dec_ref_bits(_py, args_bits);
        }
        if kwargs_bits != 0 && !obj_from_bits(kwargs_bits).is_none() {
            dec_ref_bits(_py, kwargs_bits);
        }
        return true;
    }
    false
}

// Re-export operator intrinsics for stdlib operator/_operator modules.
#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_abs(val: u64) -> u64 {
    molt_abs_builtin(val)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_add(a: u64, b: u64) -> u64 {
    molt_add(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_sub(a: u64, b: u64) -> u64 {
    molt_sub(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_mul(a: u64, b: u64) -> u64 {
    molt_mul(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_matmul(a: u64, b: u64) -> u64 {
    molt_matmul(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_truediv(a: u64, b: u64) -> u64 {
    molt_div(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_floordiv(a: u64, b: u64) -> u64 {
    molt_floordiv(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_mod(a: u64, b: u64) -> u64 {
    molt_mod(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_pow(a: u64, b: u64) -> u64 {
    molt_pow(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_lshift(a: u64, b: u64) -> u64 {
    molt_lshift(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_rshift(a: u64, b: u64) -> u64 {
    molt_rshift(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_and(a: u64, b: u64) -> u64 {
    molt_bit_and(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_or(a: u64, b: u64) -> u64 {
    molt_bit_or(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_xor(a: u64, b: u64) -> u64 {
    molt_bit_xor(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_neg(val: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(val);
        if let Some(i) = to_i64(obj) {
            return int_bits_from_i128(_py, -(i as i128));
        }
        if let Some(big) = to_bigint(obj) {
            let res = -big;
            if let Some(i) = bigint_to_inline(&res) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, res);
        }
        if let Some(f) = obj.as_float() {
            return MoltObject::from_float(-f).bits();
        }
        if complex_ptr_from_bits(val).is_some() {
            match complex_from_obj_strict(_py, obj) {
                Ok(Some(c)) => return complex_bits(_py, -c.re, -c.im),
                Err(_) => {
                    return raise_exception::<_>(
                        _py,
                        "OverflowError",
                        "int too large to convert to float",
                    )
                }
                _ => {}
            }
        }
        if let Some(ptr) = obj.as_ptr() {
            let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__neg__") else {
                return MoltObject::none().bits();
            };
            let call_bits = unsafe { attr_lookup_ptr_allow_missing(_py, ptr, name_bits) };
            dec_ref_bits(_py, name_bits);
            if let Some(call_bits) = call_bits {
                let res_bits = unsafe { call_callable0(_py, call_bits) };
                dec_ref_bits(_py, call_bits);
                return res_bits;
            }
        }
        let msg = format!("bad operand type for unary -: '{}'", type_name(_py, obj));
        raise_exception::<_>(_py, "TypeError", &msg)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_pos(val: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(val);
        if let Some(i) = to_i64(obj) {
            return MoltObject::from_int(i).bits();
        }
        if let Some(big) = to_bigint(obj) {
            if let Some(i) = bigint_to_inline(&big) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, big);
        }
        if let Some(f) = obj.as_float() {
            return MoltObject::from_float(f).bits();
        }
        if complex_ptr_from_bits(val).is_some() {
            match complex_from_obj_strict(_py, obj) {
                Ok(Some(c)) => return complex_bits(_py, c.re, c.im),
                Err(_) => {
                    return raise_exception::<_>(
                        _py,
                        "OverflowError",
                        "int too large to convert to float",
                    )
                }
                _ => {}
            }
        }
        if let Some(ptr) = obj.as_ptr() {
            let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__pos__") else {
                return MoltObject::none().bits();
            };
            let call_bits = unsafe { attr_lookup_ptr_allow_missing(_py, ptr, name_bits) };
            dec_ref_bits(_py, name_bits);
            if let Some(call_bits) = call_bits {
                let res_bits = unsafe { call_callable0(_py, call_bits) };
                dec_ref_bits(_py, call_bits);
                return res_bits;
            }
        }
        let msg = format!("bad operand type for unary +: '{}'", type_name(_py, obj));
        raise_exception::<_>(_py, "TypeError", &msg)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_invert(val: u64) -> u64 {
    molt_invert(val)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_not(val: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let truthy = molt_is_truthy(val) != 0;
        MoltObject::from_bool(!truthy).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_truth(val: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let truthy = molt_is_truthy(val) != 0;
        MoltObject::from_bool(truthy).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_eq(a: u64, b: u64) -> u64 {
    molt_eq(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_ne(a: u64, b: u64) -> u64 {
    molt_ne(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_lt(a: u64, b: u64) -> u64 {
    molt_lt(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_le(a: u64, b: u64) -> u64 {
    molt_le(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_gt(a: u64, b: u64) -> u64 {
    molt_gt(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_ge(a: u64, b: u64) -> u64 {
    molt_ge(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_is(a: u64, b: u64) -> u64 {
    MoltObject::from_bool(a == b).bits()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_is_not(a: u64, b: u64) -> u64 {
    MoltObject::from_bool(a != b).bits()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_contains(container_bits: u64, item_bits: u64) -> u64 {
    molt_contains(container_bits, item_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_getitem(obj_bits: u64, key_bits: u64) -> u64 {
    molt_getitem_method(obj_bits, key_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_setitem(obj_bits: u64, key_bits: u64, val_bits: u64) -> u64 {
    molt_setitem_method(obj_bits, key_bits, val_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_delitem(obj_bits: u64, key_bits: u64) -> u64 {
    molt_delitem_method(obj_bits, key_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_countof(container_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let iter_bits = molt_iter_checked(container_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let mut count: i64 = 0;
        loop {
            let pair_bits = molt_iter_next(iter_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let Some(pair_ptr) = obj_from_bits(pair_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            unsafe {
                if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                    return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
                }
                let elems = seq_vec_ref(pair_ptr);
                if elems.len() < 2 {
                    return MoltObject::none().bits();
                }
                let val_bits = elems[0];
                let done_bits = elems[1];
                if is_truthy(_py, obj_from_bits(done_bits)) {
                    break;
                }
                let eq_bits = molt_eq(val_bits, value_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                if is_truthy(_py, obj_from_bits(eq_bits)) {
                    count += 1;
                }
            }
        }
        MoltObject::from_int(count).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_length_hint(obj_bits: u64, default_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(obj_bits);
        if let Some(ptr) = obj.as_ptr() {
            let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__length_hint__") else {
                return MoltObject::none().bits();
            };
            let call_bits = unsafe { attr_lookup_ptr_allow_missing(_py, ptr, name_bits) };
            dec_ref_bits(_py, name_bits);
            if let Some(call_bits) = call_bits {
                let res_bits = unsafe { call_callable0(_py, call_bits) };
                dec_ref_bits(_py, call_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                let res_obj = obj_from_bits(res_bits);
                if res_obj.is_none() {
                    return default_bits;
                }
                if let Some(i) = to_i64(res_obj) {
                    if i < 0 {
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "__length_hint__() should return >= 0",
                        );
                    }
                    return MoltObject::from_int(i).bits();
                }
                if let Some(ptr) = bigint_ptr_from_bits(res_bits) {
                    let big = unsafe { bigint_ref(ptr) };
                    if big.is_negative() {
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "__length_hint__() should return >= 0",
                        );
                    }
                    let Some(len) = big.to_usize() else {
                        return raise_exception::<_>(
                            _py,
                            "OverflowError",
                            "cannot fit 'int' into an index-sized integer",
                        );
                    };
                    if len > i64::MAX as usize {
                        return raise_exception::<_>(
                            _py,
                            "OverflowError",
                            "cannot fit 'int' into an index-sized integer",
                        );
                    }
                    return MoltObject::from_int(len as i64).bits();
                }
                let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                let msg = format!("__length_hint__ returned non-int (type {res_type})");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
        let len_bits = molt_len(obj_bits);
        if exception_pending(_py) {
            crate::molt_exception_clear();
            return default_bits;
        }
        len_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_concat(a: u64, b: u64) -> u64 {
    molt_concat(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_iconcat(a: u64, b: u64) -> u64 {
    molt_inplace_concat(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_iadd(a: u64, b: u64) -> u64 {
    molt_inplace_add(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_isub(a: u64, b: u64) -> u64 {
    molt_inplace_sub(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_imul(a: u64, b: u64) -> u64 {
    molt_inplace_mul(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_imatmul(a: u64, b: u64) -> u64 {
    molt_inplace_matmul(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_itruediv(a: u64, b: u64) -> u64 {
    molt_inplace_div(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_ifloordiv(a: u64, b: u64) -> u64 {
    molt_inplace_floordiv(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_imod(a: u64, b: u64) -> u64 {
    molt_inplace_mod(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_ipow(a: u64, b: u64) -> u64 {
    molt_inplace_pow(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_ilshift(a: u64, b: u64) -> u64 {
    molt_inplace_lshift(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_irshift(a: u64, b: u64) -> u64 {
    molt_inplace_rshift(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_iand(a: u64, b: u64) -> u64 {
    molt_inplace_bit_and(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_ior(a: u64, b: u64) -> u64 {
    molt_inplace_bit_or(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_ixor(a: u64, b: u64) -> u64 {
    molt_inplace_bit_xor(a, b)
}
