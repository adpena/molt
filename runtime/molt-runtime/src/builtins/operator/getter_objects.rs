use super::class_support::{attrgetter_class, itemgetter_class, methodcaller_class};
use std::sync::atomic::Ordering;

use molt_obj_model::MoltObject;

use crate::object::ObjectShapeId;
use crate::{
    PyToken, TYPE_ID_DICT, TYPE_ID_STRING, TYPE_ID_TUPLE, alloc_string, alloc_tuple, dec_ref_bits,
    exception_pending, inc_ref_bits, molt_getattr_builtin, molt_index, obj_from_bits,
    object_class_bits, object_type_id, raise_exception, string_obj_to_owned,
};

pub(crate) unsafe fn operator_visit_owned_edges(
    shape: ObjectShapeId,
    ptr: *mut u8,
    mut visit: impl FnMut(u64),
) {
    unsafe {
        match shape {
            ObjectShapeId::OperatorItemGetter => visit(itemgetter_items_bits(ptr)),
            ObjectShapeId::OperatorAttrGetter => visit(attrgetter_attrs_bits(ptr)),
            ObjectShapeId::OperatorMethodCaller => {
                visit(methodcaller_name_bits(ptr));
                visit(methodcaller_args_bits(ptr));
                visit(methodcaller_kwargs_bits(ptr));
            }
            _ => unreachable!("non-operator object shape"),
        }
    }
}

pub(crate) unsafe fn operator_detach_owned_edges(
    shape: ObjectShapeId,
    ptr: *mut u8,
    mut detach: impl FnMut(u64),
) {
    let none = MoltObject::none().bits();
    let detached = unsafe {
        match shape {
            ObjectShapeId::OperatorItemGetter => {
                let old = itemgetter_items_bits(ptr);
                itemgetter_set_items_bits(ptr, none);
                [old, none, none]
            }
            ObjectShapeId::OperatorAttrGetter => {
                let old = attrgetter_attrs_bits(ptr);
                attrgetter_set_attrs_bits(ptr, none);
                [old, none, none]
            }
            ObjectShapeId::OperatorMethodCaller => {
                let old = [
                    methodcaller_name_bits(ptr),
                    methodcaller_args_bits(ptr),
                    methodcaller_kwargs_bits(ptr),
                ];
                methodcaller_set_name_bits(ptr, none);
                methodcaller_set_args_bits(ptr, none);
                methodcaller_set_kwargs_bits(ptr, none);
                old
            }
            _ => unreachable!("non-operator object shape"),
        }
    };
    for bits in detached {
        detach(bits);
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_itemgetter_init(self_bits: u64, items_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
            if crate::object::seq_access::len(items_ptr) == 0 {
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
    crate::with_gil_entry_nopanic!(_py, {
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
            if crate::object::seq_access::len(attrs_ptr) == 0 {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "attrgetter expected at least 1 argument",
                );
            }
            let attrs = crate::object::seq_access::pin_tuple(_py, attrs_ptr)
                .expect("type-checked attrgetter tuple must remain live");
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
            if crate::object::seq_access::len(items_ptr) == 0 {
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
    crate::with_gil_entry_nopanic!(_py, { itemgetter_class(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_attrgetter(attrs_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
            if crate::object::seq_access::len(attrs_ptr) == 0 {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "attrgetter expected at least 1 argument",
                );
            }
            let attrs = crate::object::seq_access::pin_tuple(_py, attrs_ptr)
                .expect("type-checked attrgetter tuple must remain live");
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
    crate::with_gil_entry_nopanic!(_py, { attrgetter_class(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_methodcaller(
    name_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, { methodcaller_class(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_operator_itemgetter_call(self_bits: u64, obj_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
            let Some(items) = crate::object::seq_access::snapshot(
                _py,
                items_ptr,
                "sequence snapshot allocation failed",
            ) else {
                return MoltObject::none().bits();
            };
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
    crate::with_gil_entry_nopanic!(_py, {
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
            let Some(attrs) = crate::object::seq_access::snapshot(
                _py,
                attrs_ptr,
                "sequence snapshot allocation failed",
            ) else {
                return MoltObject::none().bits();
            };
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
    crate::with_gil_entry_nopanic!(_py, {
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
        let arg_snapshot = if let Some(args_ptr) = args_ptr {
            unsafe {
                if object_type_id(args_ptr) == TYPE_ID_TUPLE {
                    let Some(snapshot) = crate::object::seq_access::snapshot(
                        _py,
                        args_ptr,
                        "sequence snapshot allocation failed",
                    ) else {
                        return MoltObject::none().bits();
                    };
                    Some(snapshot)
                } else {
                    None
                }
            }
        } else {
            None
        };
        let arg_list = arg_snapshot.as_deref().unwrap_or(&[]);
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
    let operator = &crate::runtime_state(_py).operator;
    let item_class = operator.itemgetter_class.load(Ordering::Acquire);
    if class_bits == item_class {
        let items_bits = unsafe { itemgetter_items_bits(ptr) };
        if items_bits != 0 && !obj_from_bits(items_bits).is_none() {
            dec_ref_bits(_py, items_bits);
        }
        return true;
    }
    let attr_class = operator.attrgetter_class.load(Ordering::Acquire);
    if class_bits == attr_class {
        let attrs_bits = unsafe { attrgetter_attrs_bits(ptr) };
        if attrs_bits != 0 && !obj_from_bits(attrs_bits).is_none() {
            dec_ref_bits(_py, attrs_bits);
        }
        return true;
    }
    let method_class = operator.methodcaller_class.load(Ordering::Acquire);
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
