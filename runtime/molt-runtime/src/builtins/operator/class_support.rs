use std::sync::atomic::AtomicU64;

use molt_obj_model::MoltObject;

use crate::{
    ClassEdgeOwnership, PyToken, TYPE_ID_DICT, alloc_class_obj, alloc_dict_with_pairs,
    alloc_string, alloc_tuple, attr_name_bits_from_bytes, builtin_classes, class_dict_bits,
    dec_ref_bits, dict_set_in_place, init_atomic_bits, intern_static_name, molt_class_set_base,
    obj_from_bits, object_class_bits, object_type_id,
};

fn operator_class(
    _py: &PyToken<'_>,
    slot: &AtomicU64,
    name: &str,
    layout_size: i64,
    call_slot: &AtomicU64,
    call_fn: u64,
    shape: crate::object::ObjectShapeId,
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
        if !unsafe { crate::object::class_set_instance_shape_id(class_ptr, shape) } {
            dec_ref_bits(_py, class_bits);
            return MoltObject::none().bits();
        }
        let builtins = builtin_classes(_py);
        unsafe {
            if let Some(ptr) = obj_from_bits(class_bits).as_ptr()
                && !crate::object::object_init_class_edge_unpublished(
                    _py,
                    ptr,
                    builtins.type_obj,
                    ClassEdgeOwnership::Owned,
                )
            {
                dec_ref_bits(_py, class_bits);
                return MoltObject::none().bits();
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
        let ptr = crate::builtins::functions::alloc_runtime_function_obj(_py, fn_ptr, arity);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            unsafe {
                let builtin_bits = builtin_classes(_py).builtin_function_or_method;
                let old_bits = object_class_bits(ptr);
                if old_bits != builtin_bits
                    && !crate::object::object_init_class_edge_unpublished(
                        _py,
                        ptr,
                        builtin_bits,
                        ClassEdgeOwnership::Owned,
                    )
                {
                    dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
                    return MoltObject::none().bits();
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
            dict_set_in_place(
                _py,
                dict_ptr,
                vararg_name,
                MoltObject::from_bool(true).bits(),
            );
        }
    }
    if has_varkw {
        let varkw_name = intern_static_name(
            _py,
            &crate::runtime_state(_py).interned.molt_varkw,
            b"__molt_varkw__",
        );
        unsafe {
            dict_set_in_place(
                _py,
                dict_ptr,
                varkw_name,
                MoltObject::from_bool(true).bits(),
            );
        }
    }
}

pub(super) fn itemgetter_class(_py: &PyToken<'_>) -> u64 {
    let operator = &crate::runtime_state(_py).operator;
    let class_bits = operator_class(
        _py,
        &operator.itemgetter_class,
        "itemgetter",
        16,
        &operator.itemgetter_call,
        crate::molt_operator_itemgetter_call as *const () as usize as u64,
        crate::object::ObjectShapeId::OperatorItemGetter,
    );
    let init_bits = builtin_func_bits(
        _py,
        &operator.itemgetter_init,
        crate::molt_operator_itemgetter_init as *const () as usize as u64,
        2,
    );
    mark_vararg(_py, init_bits, &[b"self"], true, false);
    set_class_method(_py, class_bits, b"__init__", init_bits);
    class_bits
}

pub(super) fn attrgetter_class(_py: &PyToken<'_>) -> u64 {
    let operator = &crate::runtime_state(_py).operator;
    let class_bits = operator_class(
        _py,
        &operator.attrgetter_class,
        "attrgetter",
        16,
        &operator.attrgetter_call,
        crate::molt_operator_attrgetter_call as *const () as usize as u64,
        crate::object::ObjectShapeId::OperatorAttrGetter,
    );
    let init_bits = builtin_func_bits(
        _py,
        &operator.attrgetter_init,
        crate::molt_operator_attrgetter_init as *const () as usize as u64,
        2,
    );
    mark_vararg(_py, init_bits, &[b"self"], true, false);
    set_class_method(_py, class_bits, b"__init__", init_bits);
    class_bits
}

pub(super) fn methodcaller_class(_py: &PyToken<'_>) -> u64 {
    let operator = &crate::runtime_state(_py).operator;
    let class_bits = operator_class(
        _py,
        &operator.methodcaller_class,
        "methodcaller",
        32,
        &operator.methodcaller_call,
        crate::molt_operator_methodcaller_call as *const () as usize as u64,
        crate::object::ObjectShapeId::OperatorMethodCaller,
    );
    let init_bits = builtin_func_bits(
        _py,
        &operator.methodcaller_init,
        crate::molt_operator_methodcaller_init as *const () as usize as u64,
        4,
    );
    mark_vararg(_py, init_bits, &[b"self", b"name"], true, true);
    set_class_method(_py, class_bits, b"__init__", init_bits);
    class_bits
}
