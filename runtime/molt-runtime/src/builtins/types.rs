use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};

use molt_obj_model::MoltObject;

use crate::{
    alloc_class_obj, alloc_classmethod_obj, alloc_dict_with_pairs, alloc_function_obj,
    alloc_generic_alias, alloc_instance_for_class, alloc_list, alloc_property_obj,
    alloc_staticmethod_obj, alloc_string, alloc_super_obj, alloc_tuple, apply_class_slots_layout,
    attr_lookup_ptr_allow_missing, attr_name_bits_from_bytes, builtin_classes, builtin_type_bits,
    call_callable0, call_callable1, call_callable2, class_bases_bits, class_bases_vec,
    class_bump_layout_version, class_dict_bits, class_layout_version_bits, class_mro_bits,
    class_mro_vec, class_name_for_error, class_set_bases_bits, class_set_layout_version_bits,
    class_set_mro_bits, class_set_qualname_bits, clear_exception, dataclass_set_class_raw,
    dec_ref_bits, dict_del_in_place, dict_get_in_place, dict_order, dict_set_in_place,
    dict_update_apply, dict_update_set_in_place, exception_pending, header_from_obj_ptr,
    inc_ref_bits, init_atomic_bits, instance_dict_bits, intern_static_name, is_builtin_class_bits,
    is_truthy, isinstance_runtime, issubclass_bits, issubclass_runtime, maybe_ptr_from_bits,
    missing_bits, molt_alloc, molt_call_bind, molt_callargs_new, molt_callargs_push_kw,
    molt_callargs_push_pos, molt_contains, molt_dict_from_obj, molt_dict_get, molt_eq,
    molt_getattr_builtin, molt_index, molt_iter, molt_iter_next, molt_len, molt_object_setattr,
    molt_repr_from_obj, molt_setitem_method, molt_str_from_obj, molt_string_isidentifier,
    obj_from_bits, object_class_bits, object_set_class_bits, object_type_id, property_del_bits,
    property_get_bits, property_set_bits, raise_exception, raise_not_iterable, runtime_state,
    seq_vec_ref, string_obj_to_owned, to_i64, tuple_from_iter_bits, type_name, type_of_bits,
    PyToken, HEADER_FLAG_SKIP_CLASS_DECREF, TYPE_ID_BYTES, TYPE_ID_COMPLEX, TYPE_ID_DATACLASS,
    TYPE_ID_DICT, TYPE_ID_ELLIPSIS, TYPE_ID_LIST, TYPE_ID_NOT_IMPLEMENTED, TYPE_ID_PROPERTY,
    TYPE_ID_RANGE, TYPE_ID_STRING, TYPE_ID_TUPLE, TYPE_ID_TYPE,
};

#[no_mangle]
pub extern "C" fn molt_class_new(name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "class name must be str");
        };
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "class name must be str");
            }
        }
        let ptr = alloc_class_obj(_py, name_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_builtin_type(tag_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let tag = match to_i64(obj_from_bits(tag_bits)) {
            Some(val) => val,
            None => return raise_exception::<_>(_py, "TypeError", "builtin type tag must be int"),
        };
        let Some(bits) = builtin_type_bits(_py, tag) else {
            return raise_exception::<_>(_py, "TypeError", "unknown builtin type tag");
        };
        inc_ref_bits(_py, bits);
        bits
    })
}

#[no_mangle]
pub extern "C" fn molt_type_of(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let bits = type_of_bits(_py, val_bits);
        inc_ref_bits(_py, bits);
        bits
    })
}

#[no_mangle]
pub extern "C" fn molt_type_new(
    cls_bits: u64,
    name_bits: u64,
    bases_bits: u64,
    namespace_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let cls_obj = obj_from_bits(cls_bits);
        let Some(cls_ptr) = cls_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "type.__new__ expects type");
        };
        unsafe {
            if object_type_id(cls_ptr) != TYPE_ID_TYPE {
                return raise_exception::<_>(_py, "TypeError", "type.__new__ expects type");
            }
        }
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "class name must be str");
        };
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "class name must be str");
            }
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
            unsafe {
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
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "bases must be a tuple of types",
                        )
                    }
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
        unsafe {
            object_set_class_bits(_py, class_ptr, cls_bits);
            inc_ref_bits(_py, cls_bits);
        }

        let dict_bits = unsafe { class_dict_bits(class_ptr) };
        unsafe {
            let _ = dict_update_apply(_py, dict_bits, dict_update_set_in_place, namespace_bits);
        }
        if exception_pending(_py) {
            if bases_owned {
                dec_ref_bits(_py, bases_tuple_bits);
            }
            return MoltObject::none().bits();
        }
        let mut qualname_bits = 0u64;
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
            if unsafe { object_type_id(dict_ptr) } == TYPE_ID_DICT {
                let qualname_name_bits = intern_static_name(
                    _py,
                    &runtime_state(_py).interned.qualname_name,
                    b"__qualname__",
                );
                if let Some(val_bits) =
                    unsafe { dict_get_in_place(_py, dict_ptr, qualname_name_bits) }
                {
                    qualname_bits = val_bits;
                    unsafe {
                        dict_del_in_place(_py, dict_ptr, qualname_name_bits);
                    }
                    if exception_pending(_py) {
                        if bases_owned {
                            dec_ref_bits(_py, bases_tuple_bits);
                        }
                        return MoltObject::none().bits();
                    }
                }
                if let Some(classcell_bits) = attr_name_bits_from_bytes(_py, b"__classcell__") {
                    unsafe {
                        dict_del_in_place(_py, dict_ptr, classcell_bits);
                    }
                    dec_ref_bits(_py, classcell_bits);
                    if exception_pending(_py) {
                        if bases_owned {
                            dec_ref_bits(_py, bases_tuple_bits);
                        }
                        return MoltObject::none().bits();
                    }
                }
            }
        }
        if qualname_bits == 0 {
            qualname_bits = name_bits;
        }
        let qualname_obj = obj_from_bits(qualname_bits);
        let qualname_is_str = if let Some(ptr) = qualname_obj.as_ptr() {
            unsafe { object_type_id(ptr) == TYPE_ID_STRING }
        } else {
            false
        };
        if !qualname_is_str {
            let type_label = type_name(_py, qualname_obj);
            let msg = format!("type __qualname__ must be a str, not {}", type_label);
            if bases_owned {
                dec_ref_bits(_py, bases_tuple_bits);
            }
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        unsafe {
            class_set_qualname_bits(_py, class_ptr, qualname_bits);
        }

        let _ = molt_class_set_base(class_bits, bases_tuple_bits);
        if exception_pending(_py) {
            if bases_owned {
                dec_ref_bits(_py, bases_tuple_bits);
            }
            return MoltObject::none().bits();
        }
        if unsafe { !apply_class_slots_layout(_py, class_ptr) } {
            if bases_owned {
                dec_ref_bits(_py, bases_tuple_bits);
            }
            return MoltObject::none().bits();
        }

        let mut kw_pairs: Vec<(u64, u64)> = Vec::new();
        let kwargs_obj = obj_from_bits(kwargs_bits);
        if !kwargs_obj.is_none() {
            if let Some(kwargs_ptr) = kwargs_obj.as_ptr() {
                unsafe {
                    if object_type_id(kwargs_ptr) == TYPE_ID_DICT {
                        let entries = dict_order(kwargs_ptr).clone();
                        for pair in entries.chunks(2) {
                            if pair.len() == 2 {
                                kw_pairs.push((pair[0], pair[1]));
                            }
                        }
                    }
                }
            }
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
            let Some(init_bits) =
                (unsafe { attr_lookup_ptr_allow_missing(_py, base_ptr, init_name_bits) })
            else {
                continue;
            };
            let builder_bits =
                molt_callargs_new((1 + kw_pairs.len()) as u64, kw_pairs.len() as u64);
            if builder_bits == 0 {
                dec_ref_bits(_py, init_bits);
                if bases_owned {
                    dec_ref_bits(_py, bases_tuple_bits);
                }
                return MoltObject::none().bits();
            }
            unsafe {
                let _ = molt_callargs_push_pos(builder_bits, class_bits);
            }
            for (name_bits, val_bits) in kw_pairs.iter().copied() {
                unsafe {
                    let _ = molt_callargs_push_kw(builder_bits, name_bits, val_bits);
                }
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
        if !kwargs_obj.is_none() {
            dec_ref_bits(_py, kwargs_bits);
        }
        class_bits
    })
}

#[no_mangle]
pub extern "C" fn molt_type_init(
    _cls_bits: u64,
    _name_bits: u64,
    _bases_bits: u64,
    _namespace_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !obj_from_bits(kwargs_bits).is_none() {
            dec_ref_bits(_py, kwargs_bits);
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_type_mro(cls_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let cls_obj = obj_from_bits(cls_bits);
        let Some(cls_ptr) = cls_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "mro expects type");
        };
        unsafe {
            if object_type_id(cls_ptr) != TYPE_ID_TYPE {
                return raise_exception::<_>(_py, "TypeError", "mro expects type");
            }
        }
        let mro = class_mro_vec(cls_bits);
        let list_ptr = alloc_list(_py, &mro);
        if list_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(list_ptr).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_type_instancecheck(cls_bits: u64, inst_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let inst_type = type_of_bits(_py, inst_bits);
        MoltObject::from_bool(issubclass_bits(inst_type, cls_bits)).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_type_subclasscheck(cls_bits: u64, sub_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        MoltObject::from_bool(issubclass_bits(sub_bits, cls_bits)).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_isinstance(val_bits: u64, class_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        MoltObject::from_bool(isinstance_runtime(_py, val_bits, class_bits)).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_issubclass(sub_bits: u64, class_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(sub_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "issubclass() arg 1 must be a class");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_TYPE {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "issubclass() arg 1 must be a class",
                );
            }
        }
        let mut classes = Vec::new();
        collect_classinfo_issubclass(_py, class_bits, &mut classes);
        for class_bits in classes {
            if issubclass_runtime(_py, sub_bits, class_bits) {
                return MoltObject::from_bool(true).bits();
            }
        }
        MoltObject::from_bool(false).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_object_new() -> u64 {
    crate::with_gil_entry!(_py, {
        let obj_bits = molt_alloc(std::mem::size_of::<u64>() as u64);
        let Some(obj_ptr) = obj_from_bits(obj_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        let class_bits = builtin_classes(_py).object;
        unsafe {
            let _ = molt_object_set_class(obj_ptr, class_bits);
        }
        obj_bits
    })
}

#[no_mangle]
pub extern "C" fn molt_object_new_bound(cls_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let cls_obj = obj_from_bits(cls_bits);
        let Some(cls_ptr) = cls_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "object.__new__ expects type");
        };
        unsafe {
            if object_type_id(cls_ptr) != TYPE_ID_TYPE {
                return raise_exception::<_>(_py, "TypeError", "object.__new__ expects type");
            }
        }
        let builtins = builtin_classes(_py);
        if is_builtin_class_bits(_py, cls_bits) && cls_bits != builtins.object {
            let class_name = class_name_for_error(cls_bits);
            let msg =
                format!("object.__new__({class_name}) is not safe, use {class_name}.__new__()");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        unsafe { alloc_instance_for_class(_py, cls_ptr) }
    })
}

#[no_mangle]
pub extern "C" fn molt_tuple_new_bound(cls_bits: u64, iterable_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let cls_obj = obj_from_bits(cls_bits);
        let Some(cls_ptr) = cls_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "tuple.__new__ expects type");
        };
        unsafe {
            if object_type_id(cls_ptr) != TYPE_ID_TYPE {
                return raise_exception::<_>(_py, "TypeError", "tuple.__new__ expects type");
            }
        }
        let builtins = builtin_classes(_py);
        let tuple_bits = if iterable_bits == missing_bits(_py) {
            let ptr = alloc_tuple(_py, &[]);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        } else {
            let bits = unsafe { tuple_from_iter_bits(_py, iterable_bits) };
            let Some(bits) = bits else {
                return MoltObject::none().bits();
            };
            bits
        };
        if cls_bits == builtins.tuple {
            return tuple_bits;
        }
        let iter_is_tuple = maybe_ptr_from_bits(iterable_bits)
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_TUPLE });
        if iter_is_tuple && tuple_bits == iterable_bits {
            let Some(tuple_ptr) = obj_from_bits(tuple_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            let elems = unsafe { seq_vec_ref(tuple_ptr) }.clone();
            let new_ptr = alloc_tuple(_py, &elems);
            dec_ref_bits(_py, tuple_bits);
            if new_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let new_bits = MoltObject::from_ptr(new_ptr).bits();
            unsafe {
                object_set_class_bits(_py, new_ptr, cls_bits);
            }
            inc_ref_bits(_py, cls_bits);
            return new_bits;
        }
        if let Some(tuple_ptr) = obj_from_bits(tuple_bits).as_ptr() {
            unsafe {
                object_set_class_bits(_py, tuple_ptr, cls_bits);
            }
            inc_ref_bits(_py, cls_bits);
        }
        tuple_bits
    })
}

fn c3_merge(mut seqs: Vec<Vec<u64>>) -> Option<Vec<u64>> {
    let mut result = Vec::new();
    loop {
        seqs.retain(|seq| !seq.is_empty());
        if seqs.is_empty() {
            return Some(result);
        }
        let mut candidate = None;
        'outer: for seq in &seqs {
            let head = seq[0];
            let mut in_tail = false;
            for other in &seqs {
                if other.iter().skip(1).any(|val| *val == head) {
                    in_tail = true;
                    break;
                }
            }
            if !in_tail {
                candidate = Some(head);
                break 'outer;
            }
        }
        let cand = candidate?;
        result.push(cand);
        for seq in &mut seqs {
            if !seq.is_empty() && seq[0] == cand {
                seq.remove(0);
            }
        }
    }
}

fn compute_mro(class_bits: u64, bases: &[u64]) -> Option<Vec<u64>> {
    let mut seqs = Vec::with_capacity(bases.len() + 1);
    for base in bases {
        seqs.push(class_mro_vec(*base));
    }
    seqs.push(bases.to_vec());
    let mut out = vec![class_bits];
    let merged = c3_merge(seqs)?;
    out.extend(merged);
    Some(out)
}

#[no_mangle]
pub extern "C" fn molt_class_set_base(class_bits: u64, base_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let class_obj = obj_from_bits(class_bits);
        let Some(class_ptr) = class_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "class must be a type object");
        };
        unsafe {
            if object_type_id(class_ptr) != TYPE_ID_TYPE {
                return raise_exception::<_>(_py, "TypeError", "class must be a type object");
            }
        }
        let mut bases_vec = Vec::new();
        let mut bases_owned = false;
        let bases_bits = if obj_from_bits(base_bits).is_none() || base_bits == 0 {
            let tuple_ptr = alloc_tuple(_py, &[]);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            bases_owned = true;
            MoltObject::from_ptr(tuple_ptr).bits()
        } else {
            let base_obj = obj_from_bits(base_bits);
            let Some(base_ptr) = base_obj.as_ptr() else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "base must be a type object or tuple of types",
                );
            };
            unsafe {
                match object_type_id(base_ptr) {
                    TYPE_ID_TYPE => {
                        bases_vec.push(base_bits);
                        let tuple_ptr = alloc_tuple(_py, &[base_bits]);
                        if tuple_ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        bases_owned = true;
                        MoltObject::from_ptr(tuple_ptr).bits()
                    }
                    TYPE_ID_TUPLE => {
                        for item in seq_vec_ref(base_ptr).iter() {
                            bases_vec.push(*item);
                        }
                        base_bits
                    }
                    _ => {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "base must be a type object or tuple of types",
                        )
                    }
                }
            }
        };

        if bases_vec.is_empty() {
            bases_vec = class_bases_vec(bases_bits);
        }
        let mut seen = HashSet::new();
        for base in &bases_vec {
            if !seen.insert(*base) {
                let name = class_name_for_error(*base);
                let msg = format!("duplicate base class {name}");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
        for base in bases_vec.iter() {
            let base_obj = obj_from_bits(*base);
            let Some(base_ptr) = base_obj.as_ptr() else {
                return raise_exception::<_>(_py, "TypeError", "base must be a type object");
            };
            unsafe {
                if object_type_id(base_ptr) != TYPE_ID_TYPE {
                    return raise_exception::<_>(_py, "TypeError", "base must be a type object");
                }
                if base_ptr == class_ptr {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "class cannot inherit from itself",
                    );
                }
            }
        }

        let mro = match compute_mro(class_bits, &bases_vec) {
            Some(val) => val,
            None => {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "Cannot create a consistent method resolution order (MRO) for bases",
                );
            }
        };
        let mro_ptr = alloc_tuple(_py, &mro);
        if mro_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let mro_bits = MoltObject::from_ptr(mro_ptr).bits();

        unsafe {
            let old_bases = class_bases_bits(class_ptr);
            let old_mro = class_mro_bits(class_ptr);
            let mut updated = false;
            if old_bases != bases_bits {
                dec_ref_bits(_py, old_bases);
                if !bases_owned {
                    inc_ref_bits(_py, bases_bits);
                }
                class_set_bases_bits(class_ptr, bases_bits);
                updated = true;
            }
            if old_mro != mro_bits {
                dec_ref_bits(_py, old_mro);
                class_set_mro_bits(class_ptr, mro_bits);
                updated = true;
            }
            let dict_bits = class_dict_bits(class_ptr);
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                if object_type_id(dict_ptr) == TYPE_ID_DICT {
                    let bases_name = intern_static_name(
                        _py,
                        &runtime_state(_py).interned.bases_name,
                        b"__bases__",
                    );
                    let mro_name =
                        intern_static_name(_py, &runtime_state(_py).interned.mro_name, b"__mro__");
                    dict_set_in_place(_py, dict_ptr, bases_name, bases_bits);
                    dict_set_in_place(_py, dict_ptr, mro_name, mro_bits);
                }
            }
            if updated {
                class_bump_layout_version(class_ptr);
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_class_apply_set_name(class_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let class_obj = obj_from_bits(class_bits);
        let Some(class_ptr) = class_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "class must be a type object");
        };
        unsafe {
            if object_type_id(class_ptr) != TYPE_ID_TYPE {
                return raise_exception::<_>(_py, "TypeError", "class must be a type object");
            }
            if !apply_class_slots_layout(_py, class_ptr) {
                return MoltObject::none().bits();
            }
            let dict_bits = class_dict_bits(class_ptr);
            let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return MoltObject::none().bits();
            }
            let entries = dict_order(dict_ptr).clone();
            let set_name_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.set_name_method,
                b"__set_name__",
            );
            for pair in entries.chunks(2) {
                if pair.len() != 2 {
                    continue;
                }
                let name_bits = pair[0];
                let val_bits = pair[1];
                let Some(val_ptr) = maybe_ptr_from_bits(val_bits) else {
                    continue;
                };
                if let Some(set_name) = attr_lookup_ptr_allow_missing(_py, val_ptr, set_name_bits) {
                    let _ = call_callable2(_py, set_name, class_bits, name_bits);
                    dec_ref_bits(_py, set_name);
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_class_layout_version(class_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let class_obj = obj_from_bits(class_bits);
        let Some(class_ptr) = class_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "class must be a type object");
        };
        unsafe {
            if object_type_id(class_ptr) != TYPE_ID_TYPE {
                return raise_exception::<_>(_py, "TypeError", "class must be a type object");
            }
            MoltObject::from_int(class_layout_version_bits(class_ptr) as i64).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_class_set_layout_version(class_bits: u64, version_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let class_obj = obj_from_bits(class_bits);
        let Some(class_ptr) = class_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "class must be a type object");
        };
        unsafe {
            if object_type_id(class_ptr) != TYPE_ID_TYPE {
                return raise_exception::<_>(_py, "TypeError", "class must be a type object");
            }
            let version = match to_i64(obj_from_bits(version_bits)) {
                Some(val) if val >= 0 => val as u64,
                _ => return raise_exception::<_>(_py, "TypeError", "layout version must be int"),
            };
            class_set_layout_version_bits(class_ptr, version);
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_super_new(type_bits: u64, obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let type_obj = obj_from_bits(type_bits);
        let Some(type_ptr) = type_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "super() arg 1 must be a type");
        };
        unsafe {
            if object_type_id(type_ptr) != TYPE_ID_TYPE {
                return raise_exception::<_>(_py, "TypeError", "super() arg 1 must be a type");
            }
        }
        let obj = obj_from_bits(obj_bits);
        if obj.is_none() || obj_bits == 0 {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "super() arg 2 must be an instance or subtype of type",
            );
        }
        let obj_is_type = if let Some(obj_ptr) = obj.as_ptr() {
            unsafe { object_type_id(obj_ptr) == TYPE_ID_TYPE }
        } else {
            false
        };
        let is_instance = isinstance_runtime(_py, obj_bits, type_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let is_subtype = obj_is_type && issubclass_bits(obj_bits, type_bits);
        if !(is_instance || is_subtype) {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "super() arg 2 must be an instance or subtype of type",
            );
        }
        let ptr = alloc_super_obj(_py, type_bits, obj_bits);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_classmethod_new(func_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = alloc_classmethod_obj(_py, func_bits);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_generic_alias_new(origin_bits: u64, args_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let args_obj = obj_from_bits(args_bits);
        let args_tuple_bits = if let Some(args_ptr) = args_obj.as_ptr() {
            unsafe {
                if object_type_id(args_ptr) == TYPE_ID_TUPLE {
                    args_bits
                } else {
                    let tuple_ptr = alloc_tuple(_py, &[args_bits]);
                    if tuple_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    MoltObject::from_ptr(tuple_ptr).bits()
                }
            }
        } else {
            let tuple_ptr = alloc_tuple(_py, &[args_bits]);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(tuple_ptr).bits()
        };
        let owned_args = args_tuple_bits != args_bits;
        let ptr = alloc_generic_alias(_py, origin_bits, args_tuple_bits);
        if owned_args {
            dec_ref_bits(_py, args_tuple_bits);
        }
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_typing_type_param(typevar_ctor_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "type parameter name must be str");
        };
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "type parameter name must be str");
            }
        }
        let builder_bits = molt_callargs_new(1, 0);
        if builder_bits == 0 {
            return MoltObject::none().bits();
        }
        unsafe {
            let _ = molt_callargs_push_pos(builder_bits, name_bits);
        }
        let typevar_bits = molt_call_bind(typevar_ctor_bits, builder_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let Some(flag_name_bits) = attr_name_bits_from_bytes(_py, b"_pep695") else {
            return MoltObject::none().bits();
        };
        let _ = molt_object_setattr(
            typevar_bits,
            flag_name_bits,
            MoltObject::from_bool(true).bits(),
        );
        dec_ref_bits(_py, flag_name_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        typevar_bits
    })
}

#[no_mangle]
pub extern "C" fn molt_staticmethod_new(func_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = alloc_staticmethod_obj(_py, func_bits);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_property_new(get_bits: u64, set_bits: u64, del_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = alloc_property_obj(_py, get_bits, set_bits, del_bits);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_property_getter(prop_bits: u64, get_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let prop_obj = obj_from_bits(prop_bits);
        let Some(prop_ptr) = prop_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "property.getter expects property");
        };
        unsafe {
            if object_type_id(prop_ptr) != TYPE_ID_PROPERTY {
                return raise_exception::<_>(_py, "TypeError", "property.getter expects property");
            }
            let set_bits = property_set_bits(prop_ptr);
            let del_bits = property_del_bits(prop_ptr);
            let ptr = alloc_property_obj(_py, get_bits, set_bits, del_bits);
            if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            }
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_property_setter(prop_bits: u64, set_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let prop_obj = obj_from_bits(prop_bits);
        let Some(prop_ptr) = prop_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "property.setter expects property");
        };
        unsafe {
            if object_type_id(prop_ptr) != TYPE_ID_PROPERTY {
                return raise_exception::<_>(_py, "TypeError", "property.setter expects property");
            }
            let get_bits = property_get_bits(prop_ptr);
            let del_bits = property_del_bits(prop_ptr);
            let ptr = alloc_property_obj(_py, get_bits, set_bits, del_bits);
            if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            }
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_property_deleter(prop_bits: u64, del_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let prop_obj = obj_from_bits(prop_bits);
        let Some(prop_ptr) = prop_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "property.deleter expects property");
        };
        unsafe {
            if object_type_id(prop_ptr) != TYPE_ID_PROPERTY {
                return raise_exception::<_>(_py, "TypeError", "property.deleter expects property");
            }
            let get_bits = property_get_bits(prop_ptr);
            let set_bits = property_set_bits(prop_ptr);
            let ptr = alloc_property_obj(_py, get_bits, set_bits, del_bits);
            if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            }
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_types_dynamic_class_attr_init(
    self_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(positional) = call_vararg_args(_py, "DynamicClassAttribute.__init__", args_bits)
        else {
            return MoltObject::none().bits();
        };
        let Some((_, keywords)) =
            call_vararg_kwargs(_py, "DynamicClassAttribute.__init__", kwargs_bits)
        else {
            return MoltObject::none().bits();
        };
        if positional.len() > 4 {
            let msg = format!(
                "DynamicClassAttribute.__init__() takes at most 4 positional arguments ({} given)",
                positional.len()
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }

        let none = MoltObject::none().bits();
        let mut fget_bits = positional.first().copied().unwrap_or(none);
        let mut fset_bits = positional.get(1).copied().unwrap_or(none);
        let mut fdel_bits = positional.get(2).copied().unwrap_or(none);
        let mut doc_bits = positional.get(3).copied().unwrap_or(none);
        let mut has_fget = !positional.is_empty();
        let mut has_fset = positional.len() >= 2;
        let mut has_fdel = positional.len() >= 3;
        let mut has_doc = positional.len() >= 4;
        for (key, val_bits) in keywords.iter() {
            match key.as_str() {
                "fget" => {
                    if has_fget {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "DynamicClassAttribute.__init__() got multiple values for argument 'fget'",
                        );
                    }
                    fget_bits = *val_bits;
                    has_fget = true;
                }
                "fset" => {
                    if has_fset {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "DynamicClassAttribute.__init__() got multiple values for argument 'fset'",
                        );
                    }
                    fset_bits = *val_bits;
                    has_fset = true;
                }
                "fdel" => {
                    if has_fdel {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "DynamicClassAttribute.__init__() got multiple values for argument 'fdel'",
                        );
                    }
                    fdel_bits = *val_bits;
                    has_fdel = true;
                }
                "doc" => {
                    if has_doc {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "DynamicClassAttribute.__init__() got multiple values for argument 'doc'",
                        );
                    }
                    doc_bits = *val_bits;
                    has_doc = true;
                }
                _ => {
                    let msg = format!(
                        "DynamicClassAttribute.__init__() got an unexpected keyword argument '{}'",
                        key
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            }
        }

        let mut effective_doc = doc_bits;
        let overwrite_doc = obj_from_bits(doc_bits).is_none();
        if overwrite_doc {
            let Some(fget_doc) = dynamic_class_attr_get(_py, fget_bits, "__doc__", none) else {
                return MoltObject::none().bits();
            };
            effective_doc = fget_doc.bits;
            if fget_doc.owned {
                dec_ref_bits(_py, fget_doc.bits);
            }
        }
        let Some(is_abstract) = dynamic_class_attr_get(
            _py,
            fget_bits,
            "__isabstractmethod__",
            MoltObject::from_bool(false).bits(),
        ) else {
            return MoltObject::none().bits();
        };
        let is_abstract_flag = is_truthy(_py, obj_from_bits(is_abstract.bits));
        if is_abstract.owned {
            dec_ref_bits(_py, is_abstract.bits);
        }

        if !dynamic_class_attr_set(_py, self_bits, "fget", fget_bits)
            || !dynamic_class_attr_set(_py, self_bits, "fset", fset_bits)
            || !dynamic_class_attr_set(_py, self_bits, "fdel", fdel_bits)
            || !dynamic_class_attr_set(_py, self_bits, "__doc__", effective_doc)
            || !dynamic_class_attr_set(
                _py,
                self_bits,
                "overwrite_doc",
                MoltObject::from_bool(overwrite_doc).bits(),
            )
            || !dynamic_class_attr_set(
                _py,
                self_bits,
                "__isabstractmethod__",
                MoltObject::from_bool(is_abstract_flag).bits(),
            )
        {
            return MoltObject::none().bits();
        }

        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_types_dynamic_class_attr_get(
    self_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let kwargs_is_dict = obj_from_bits(kwargs_bits)
            .as_ptr()
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_DICT });
        let (positional, keywords) = if kwargs_is_dict {
            let Some(positional) =
                call_vararg_args(_py, "DynamicClassAttribute.__get__", args_bits)
            else {
                return MoltObject::none().bits();
            };
            let Some((_, keywords)) =
                call_vararg_kwargs(_py, "DynamicClassAttribute.__get__", kwargs_bits)
            else {
                return MoltObject::none().bits();
            };
            (positional, keywords)
        } else {
            // Descriptor protocol dispatch may call __get__ directly with
            // `(instance, ownerclass)` instead of vararg tuple/dict packing.
            (vec![args_bits, kwargs_bits], Vec::new())
        };
        if positional.len() > 2 {
            let msg = format!(
                "DynamicClassAttribute.__get__() takes at most 2 positional arguments ({} given)",
                positional.len()
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let none = MoltObject::none().bits();
        let mut instance_bits = positional.first().copied().unwrap_or(0);
        let mut has_instance = !positional.is_empty();
        for (key, val_bits) in keywords.iter() {
            match key.as_str() {
                "instance" => {
                    if has_instance {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "DynamicClassAttribute.__get__() got multiple values for argument 'instance'",
                        );
                    }
                    instance_bits = *val_bits;
                    has_instance = true;
                }
                "ownerclass" => {}
                _ => {
                    let msg = format!(
                        "DynamicClassAttribute.__get__() got an unexpected keyword argument '{}'",
                        key
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            }
        }
        if !has_instance {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "DynamicClassAttribute.__get__() missing 1 required positional argument: 'instance'",
            );
        }

        if obj_from_bits(instance_bits).is_none() {
            let Some(is_abstract) = dynamic_class_attr_get(
                _py,
                self_bits,
                "__isabstractmethod__",
                MoltObject::from_bool(false).bits(),
            ) else {
                return MoltObject::none().bits();
            };
            let is_abstract_flag = is_truthy(_py, obj_from_bits(is_abstract.bits));
            if is_abstract.owned {
                dec_ref_bits(_py, is_abstract.bits);
            }
            if is_abstract_flag {
                inc_ref_bits(_py, self_bits);
                return self_bits;
            }
            return raise_exception::<_>(_py, "AttributeError", "");
        }

        let Some(fget) = dynamic_class_attr_get(_py, self_bits, "fget", none) else {
            return MoltObject::none().bits();
        };
        if obj_from_bits(fget.bits).is_none() {
            if fget.owned {
                dec_ref_bits(_py, fget.bits);
            }
            return raise_exception::<_>(_py, "AttributeError", "unreadable attribute");
        }
        let out = unsafe { call_callable1(_py, fget.bits, instance_bits) };
        if fget.owned {
            dec_ref_bits(_py, fget.bits);
        }
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        out
    })
}

#[no_mangle]
pub extern "C" fn molt_types_dynamic_class_attr_set(
    self_bits: u64,
    instance_bits: u64,
    value_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let none = MoltObject::none().bits();
        let Some(fset) = dynamic_class_attr_get(_py, self_bits, "fset", none) else {
            return MoltObject::none().bits();
        };
        if obj_from_bits(fset.bits).is_none() {
            if fset.owned {
                dec_ref_bits(_py, fset.bits);
            }
            return raise_exception::<_>(_py, "AttributeError", "can't set attribute");
        }
        let _ = unsafe { call_callable2(_py, fset.bits, instance_bits, value_bits) };
        if fset.owned {
            dec_ref_bits(_py, fset.bits);
        }
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_types_dynamic_class_attr_delete(self_bits: u64, instance_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let none = MoltObject::none().bits();
        let Some(fdel) = dynamic_class_attr_get(_py, self_bits, "fdel", none) else {
            return MoltObject::none().bits();
        };
        if obj_from_bits(fdel.bits).is_none() {
            if fdel.owned {
                dec_ref_bits(_py, fdel.bits);
            }
            return raise_exception::<_>(_py, "AttributeError", "can't delete attribute");
        }
        let _ = unsafe { call_callable1(_py, fdel.bits, instance_bits) };
        if fdel.owned {
            dec_ref_bits(_py, fdel.bits);
        }
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        MoltObject::none().bits()
    })
}

fn dynamic_class_attr_clone(
    _py: &PyToken<'_>,
    self_bits: u64,
    fget_bits: u64,
    fset_bits: u64,
    fdel_bits: u64,
    doc_bits: u64,
) -> u64 {
    let class_bits = type_of_bits(_py, self_bits);
    call_with_kwargs(
        _py,
        class_bits,
        &[fget_bits, fset_bits, fdel_bits, doc_bits],
        0,
    )
}

#[no_mangle]
pub extern "C" fn molt_types_dynamic_class_attr_getter(self_bits: u64, fget_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let none = MoltObject::none().bits();
        let Some(overwrite_doc) = dynamic_class_attr_get(
            _py,
            self_bits,
            "overwrite_doc",
            MoltObject::from_bool(false).bits(),
        ) else {
            return MoltObject::none().bits();
        };
        let overwrite_doc_flag = is_truthy(_py, obj_from_bits(overwrite_doc.bits));
        let Some(fset) = dynamic_class_attr_get(_py, self_bits, "fset", none) else {
            if overwrite_doc.owned {
                dec_ref_bits(_py, overwrite_doc.bits);
            }
            return MoltObject::none().bits();
        };
        let Some(fdel) = dynamic_class_attr_get(_py, self_bits, "fdel", none) else {
            if overwrite_doc.owned {
                dec_ref_bits(_py, overwrite_doc.bits);
            }
            if fset.owned {
                dec_ref_bits(_py, fset.bits);
            }
            return MoltObject::none().bits();
        };
        let mut doc_bits = none;
        let mut doc_owned = false;
        if overwrite_doc_flag {
            if let Some(fdoc) = dynamic_class_attr_get(_py, fget_bits, "__doc__", none) {
                doc_bits = fdoc.bits;
                doc_owned = fdoc.owned;
            } else {
                if overwrite_doc.owned {
                    dec_ref_bits(_py, overwrite_doc.bits);
                }
                if fset.owned {
                    dec_ref_bits(_py, fset.bits);
                }
                if fdel.owned {
                    dec_ref_bits(_py, fdel.bits);
                }
                return MoltObject::none().bits();
            }
        }
        if obj_from_bits(doc_bits).is_none() {
            if let Some(self_doc) = dynamic_class_attr_get(_py, self_bits, "__doc__", none) {
                if doc_owned {
                    dec_ref_bits(_py, doc_bits);
                }
                doc_bits = self_doc.bits;
                doc_owned = self_doc.owned;
            } else {
                if overwrite_doc.owned {
                    dec_ref_bits(_py, overwrite_doc.bits);
                }
                if fset.owned {
                    dec_ref_bits(_py, fset.bits);
                }
                if fdel.owned {
                    dec_ref_bits(_py, fdel.bits);
                }
                return MoltObject::none().bits();
            }
        }

        let out =
            dynamic_class_attr_clone(_py, self_bits, fget_bits, fset.bits, fdel.bits, doc_bits);
        if !obj_from_bits(out).is_none()
            && !dynamic_class_attr_set(
                _py,
                out,
                "overwrite_doc",
                MoltObject::from_bool(overwrite_doc_flag).bits(),
            )
        {
            if overwrite_doc.owned {
                dec_ref_bits(_py, overwrite_doc.bits);
            }
            if fset.owned {
                dec_ref_bits(_py, fset.bits);
            }
            if fdel.owned {
                dec_ref_bits(_py, fdel.bits);
            }
            if doc_owned {
                dec_ref_bits(_py, doc_bits);
            }
            return MoltObject::none().bits();
        }

        if overwrite_doc.owned {
            dec_ref_bits(_py, overwrite_doc.bits);
        }
        if fset.owned {
            dec_ref_bits(_py, fset.bits);
        }
        if fdel.owned {
            dec_ref_bits(_py, fdel.bits);
        }
        if doc_owned {
            dec_ref_bits(_py, doc_bits);
        }
        out
    })
}

#[no_mangle]
pub extern "C" fn molt_types_dynamic_class_attr_setter(self_bits: u64, fset_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let none = MoltObject::none().bits();
        let Some(overwrite_doc) = dynamic_class_attr_get(
            _py,
            self_bits,
            "overwrite_doc",
            MoltObject::from_bool(false).bits(),
        ) else {
            return MoltObject::none().bits();
        };
        let overwrite_doc_flag = is_truthy(_py, obj_from_bits(overwrite_doc.bits));
        let Some(fget) = dynamic_class_attr_get(_py, self_bits, "fget", none) else {
            if overwrite_doc.owned {
                dec_ref_bits(_py, overwrite_doc.bits);
            }
            return MoltObject::none().bits();
        };
        let Some(fdel) = dynamic_class_attr_get(_py, self_bits, "fdel", none) else {
            if overwrite_doc.owned {
                dec_ref_bits(_py, overwrite_doc.bits);
            }
            if fget.owned {
                dec_ref_bits(_py, fget.bits);
            }
            return MoltObject::none().bits();
        };
        let Some(doc) = dynamic_class_attr_get(_py, self_bits, "__doc__", none) else {
            if overwrite_doc.owned {
                dec_ref_bits(_py, overwrite_doc.bits);
            }
            if fget.owned {
                dec_ref_bits(_py, fget.bits);
            }
            if fdel.owned {
                dec_ref_bits(_py, fdel.bits);
            }
            return MoltObject::none().bits();
        };
        let out =
            dynamic_class_attr_clone(_py, self_bits, fget.bits, fset_bits, fdel.bits, doc.bits);
        if !obj_from_bits(out).is_none()
            && !dynamic_class_attr_set(
                _py,
                out,
                "overwrite_doc",
                MoltObject::from_bool(overwrite_doc_flag).bits(),
            )
        {
            if overwrite_doc.owned {
                dec_ref_bits(_py, overwrite_doc.bits);
            }
            if fget.owned {
                dec_ref_bits(_py, fget.bits);
            }
            if fdel.owned {
                dec_ref_bits(_py, fdel.bits);
            }
            if doc.owned {
                dec_ref_bits(_py, doc.bits);
            }
            return MoltObject::none().bits();
        }
        if overwrite_doc.owned {
            dec_ref_bits(_py, overwrite_doc.bits);
        }
        if fget.owned {
            dec_ref_bits(_py, fget.bits);
        }
        if fdel.owned {
            dec_ref_bits(_py, fdel.bits);
        }
        if doc.owned {
            dec_ref_bits(_py, doc.bits);
        }
        out
    })
}

#[no_mangle]
pub extern "C" fn molt_types_dynamic_class_attr_deleter(self_bits: u64, fdel_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let none = MoltObject::none().bits();
        let Some(overwrite_doc) = dynamic_class_attr_get(
            _py,
            self_bits,
            "overwrite_doc",
            MoltObject::from_bool(false).bits(),
        ) else {
            return MoltObject::none().bits();
        };
        let overwrite_doc_flag = is_truthy(_py, obj_from_bits(overwrite_doc.bits));
        let Some(fget) = dynamic_class_attr_get(_py, self_bits, "fget", none) else {
            if overwrite_doc.owned {
                dec_ref_bits(_py, overwrite_doc.bits);
            }
            return MoltObject::none().bits();
        };
        let Some(fset) = dynamic_class_attr_get(_py, self_bits, "fset", none) else {
            if overwrite_doc.owned {
                dec_ref_bits(_py, overwrite_doc.bits);
            }
            if fget.owned {
                dec_ref_bits(_py, fget.bits);
            }
            return MoltObject::none().bits();
        };
        let Some(doc) = dynamic_class_attr_get(_py, self_bits, "__doc__", none) else {
            if overwrite_doc.owned {
                dec_ref_bits(_py, overwrite_doc.bits);
            }
            if fget.owned {
                dec_ref_bits(_py, fget.bits);
            }
            if fset.owned {
                dec_ref_bits(_py, fset.bits);
            }
            return MoltObject::none().bits();
        };
        let out =
            dynamic_class_attr_clone(_py, self_bits, fget.bits, fset.bits, fdel_bits, doc.bits);
        if !obj_from_bits(out).is_none()
            && !dynamic_class_attr_set(
                _py,
                out,
                "overwrite_doc",
                MoltObject::from_bool(overwrite_doc_flag).bits(),
            )
        {
            if overwrite_doc.owned {
                dec_ref_bits(_py, overwrite_doc.bits);
            }
            if fget.owned {
                dec_ref_bits(_py, fget.bits);
            }
            if fset.owned {
                dec_ref_bits(_py, fset.bits);
            }
            if doc.owned {
                dec_ref_bits(_py, doc.bits);
            }
            return MoltObject::none().bits();
        }
        if overwrite_doc.owned {
            dec_ref_bits(_py, overwrite_doc.bits);
        }
        if fget.owned {
            dec_ref_bits(_py, fget.bits);
        }
        if fset.owned {
            dec_ref_bits(_py, fset.bits);
        }
        if doc.owned {
            dec_ref_bits(_py, doc.bits);
        }
        out
    })
}

/// # Safety
/// `obj_ptr` must point to a valid Molt object header that can be mutated, and
/// `class_bits` must be either zero or a valid Molt type object.
#[no_mangle]
pub unsafe extern "C" fn molt_object_set_class(obj_ptr: *mut u8, class_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if obj_ptr.is_null() {
            return raise_exception::<_>(_py, "AttributeError", "object has no class");
        }
        let header = header_from_obj_ptr(obj_ptr);
        if (*header).poll_fn != 0 {
            return raise_exception::<_>(_py, "TypeError", "cannot set class on async object");
        }
        if object_type_id(obj_ptr) == TYPE_ID_DATACLASS {
            return dataclass_set_class_raw(_py, obj_ptr, class_bits);
        }
        if class_bits != 0 {
            let class_obj = obj_from_bits(class_bits);
            let Some(class_ptr) = class_obj.as_ptr() else {
                return raise_exception::<_>(_py, "TypeError", "class must be a type object");
            };
            if object_type_id(class_ptr) != TYPE_ID_TYPE {
                return raise_exception::<_>(_py, "TypeError", "class must be a type object");
            }
        }
        let skip_class_ref = ((*header).flags & HEADER_FLAG_SKIP_CLASS_DECREF) != 0;
        let old_bits = object_class_bits(obj_ptr);
        if old_bits != 0 && !skip_class_ref {
            dec_ref_bits(_py, old_bits);
        }
        object_set_class_bits(_py, obj_ptr, class_bits);
        if class_bits != 0 && !skip_class_ref {
            inc_ref_bits(_py, class_bits);
        }
        MoltObject::none().bits()
    })
}

fn collect_classinfo_issubclass(_py: &PyToken<'_>, class_bits: u64, out: &mut Vec<u64>) {
    let obj = obj_from_bits(class_bits);
    let Some(ptr) = obj.as_ptr() else {
        return raise_exception::<_>(
            _py,
            "TypeError",
            "issubclass() arg 2 must be a class or tuple of classes",
        );
    };
    unsafe {
        match object_type_id(ptr) {
            TYPE_ID_TYPE => out.push(class_bits),
            TYPE_ID_TUPLE => {
                let items = seq_vec_ref(ptr);
                for item in items.iter() {
                    collect_classinfo_issubclass(_py, *item, out);
                }
            }
            _ => raise_exception::<_>(
                _py,
                "TypeError",
                "issubclass() arg 2 must be a class or tuple of classes",
            ),
        }
    }
}

static MAPPINGPROXY_CLASS: AtomicU64 = AtomicU64::new(0);
static SIMPLENAMESPACE_CLASS: AtomicU64 = AtomicU64::new(0);
static CAPSULE_CLASS: AtomicU64 = AtomicU64::new(0);
static CELL_CLASS: AtomicU64 = AtomicU64::new(0);
static DYNAMICCLASSATTRIBUTE_CLASS: AtomicU64 = AtomicU64::new(0);
static METHOD_CLASS: AtomicU64 = AtomicU64::new(0);

static MAPPINGPROXY_NEW_FN: AtomicU64 = AtomicU64::new(0);
static MAPPINGPROXY_INIT_FN: AtomicU64 = AtomicU64::new(0);
static MAPPINGPROXY_GETITEM_FN: AtomicU64 = AtomicU64::new(0);
static MAPPINGPROXY_ITER_FN: AtomicU64 = AtomicU64::new(0);
static MAPPINGPROXY_LEN_FN: AtomicU64 = AtomicU64::new(0);
static MAPPINGPROXY_CONTAINS_FN: AtomicU64 = AtomicU64::new(0);
static MAPPINGPROXY_GET_FN: AtomicU64 = AtomicU64::new(0);
static MAPPINGPROXY_KEYS_FN: AtomicU64 = AtomicU64::new(0);
static MAPPINGPROXY_ITEMS_FN: AtomicU64 = AtomicU64::new(0);
static MAPPINGPROXY_VALUES_FN: AtomicU64 = AtomicU64::new(0);
static MAPPINGPROXY_REPR_FN: AtomicU64 = AtomicU64::new(0);
static MAPPINGPROXY_SETITEM_FN: AtomicU64 = AtomicU64::new(0);
static MAPPINGPROXY_DELITEM_FN: AtomicU64 = AtomicU64::new(0);

static SIMPLENAMESPACE_INIT_FN: AtomicU64 = AtomicU64::new(0);
static SIMPLENAMESPACE_REPR_FN: AtomicU64 = AtomicU64::new(0);
static SIMPLENAMESPACE_EQ_FN: AtomicU64 = AtomicU64::new(0);
static DYNAMICCLASSATTRIBUTE_INIT_FN: AtomicU64 = AtomicU64::new(0);
static DYNAMICCLASSATTRIBUTE_GET_FN: AtomicU64 = AtomicU64::new(0);
static DYNAMICCLASSATTRIBUTE_SET_FN: AtomicU64 = AtomicU64::new(0);
static DYNAMICCLASSATTRIBUTE_DELETE_FN: AtomicU64 = AtomicU64::new(0);
static DYNAMICCLASSATTRIBUTE_GETTER_FN: AtomicU64 = AtomicU64::new(0);
static DYNAMICCLASSATTRIBUTE_SETTER_FN: AtomicU64 = AtomicU64::new(0);
static DYNAMICCLASSATTRIBUTE_DELETER_FN: AtomicU64 = AtomicU64::new(0);
static CAPSULE_NEW_FN: AtomicU64 = AtomicU64::new(0);
static CELL_NEW_FN: AtomicU64 = AtomicU64::new(0);
static METHOD_NEW_FN: AtomicU64 = AtomicU64::new(0);
static METHOD_INIT_FN: AtomicU64 = AtomicU64::new(0);

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

fn types_class(_py: &PyToken<'_>, slot: &AtomicU64, name: &str, layout_size: i64) -> u64 {
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
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
            if unsafe { object_type_id(dict_ptr) } == TYPE_ID_DICT {
                let layout_name = intern_static_name(
                    _py,
                    &runtime_state(_py).interned.molt_layout_size,
                    b"__molt_layout_size__",
                );
                let layout_bits = MoltObject::from_int(layout_size).bits();
                unsafe { dict_set_in_place(_py, dict_ptr, layout_name, layout_bits) };
            }
        }
        class_bits
    })
}

fn set_class_method(_py: &PyToken<'_>, class_bits: u64, name: &str, fn_bits: u64) {
    let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
        return;
    };
    let dict_bits = unsafe { class_dict_bits(class_ptr) };
    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
        return;
    };
    unsafe {
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            return;
        }
    }
    let name_ptr = alloc_string(_py, name.as_bytes());
    if name_ptr.is_null() {
        return;
    }
    let name_bits = MoltObject::from_ptr(name_ptr).bits();
    unsafe { dict_set_in_place(_py, dict_ptr, name_bits, fn_bits) };
    dec_ref_bits(_py, name_bits);
}

fn mark_vararg_method(_py: &PyToken<'_>, func_bits: u64, include_self: bool) {
    let Some(func_ptr) = obj_from_bits(func_bits).as_ptr() else {
        return;
    };
    let dict_ptr = alloc_dict_with_pairs(_py, &[]);
    if dict_ptr.is_null() {
        return;
    }
    let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
    unsafe { crate::function_set_dict_bits(func_ptr, dict_bits) };
    let mut names: Vec<u64> = Vec::new();
    if include_self {
        let name_ptr = alloc_string(_py, b"self");
        if !name_ptr.is_null() {
            names.push(MoltObject::from_ptr(name_ptr).bits());
        }
    }
    let names_ptr = alloc_tuple(_py, names.as_slice());
    for bits in names.iter() {
        dec_ref_bits(_py, *bits);
    }
    if !names_ptr.is_null() {
        let names_bits = MoltObject::from_ptr(names_ptr).bits();
        let arg_names = intern_static_name(
            _py,
            &runtime_state(_py).interned.molt_arg_names,
            b"__molt_arg_names__",
        );
        unsafe { dict_set_in_place(_py, dict_ptr, arg_names, names_bits) };
        dec_ref_bits(_py, names_bits);
    }
    let vararg_name = intern_static_name(
        _py,
        &runtime_state(_py).interned.molt_vararg,
        b"__molt_vararg__",
    );
    let varkw_name = intern_static_name(
        _py,
        &runtime_state(_py).interned.molt_varkw,
        b"__molt_varkw__",
    );
    unsafe {
        dict_set_in_place(
            _py,
            dict_ptr,
            vararg_name,
            MoltObject::from_bool(true).bits(),
        );
        dict_set_in_place(
            _py,
            dict_ptr,
            varkw_name,
            MoltObject::from_bool(true).bits(),
        );
    }
}

unsafe fn mappingproxy_mapping_bits(ptr: *mut u8) -> u64 {
    *(ptr as *const u64)
}

unsafe fn mappingproxy_set_mapping_bits(ptr: *mut u8, bits: u64) {
    *(ptr as *mut u64) = bits;
}

fn mappingproxy_class(_py: &PyToken<'_>) -> u64 {
    let class_bits = types_class(_py, &MAPPINGPROXY_CLASS, "mappingproxy", 16);
    let new_bits = builtin_func_bits(
        _py,
        &MAPPINGPROXY_NEW_FN,
        crate::molt_types_mappingproxy_new as usize as u64,
        2,
    );
    let init_bits = builtin_func_bits(
        _py,
        &MAPPINGPROXY_INIT_FN,
        crate::molt_types_mappingproxy_init as usize as u64,
        2,
    );
    let getitem_bits = builtin_func_bits(
        _py,
        &MAPPINGPROXY_GETITEM_FN,
        crate::molt_types_mappingproxy_getitem as usize as u64,
        2,
    );
    let iter_bits = builtin_func_bits(
        _py,
        &MAPPINGPROXY_ITER_FN,
        crate::molt_types_mappingproxy_iter as usize as u64,
        1,
    );
    let len_bits = builtin_func_bits(
        _py,
        &MAPPINGPROXY_LEN_FN,
        crate::molt_types_mappingproxy_len as usize as u64,
        1,
    );
    let contains_bits = builtin_func_bits(
        _py,
        &MAPPINGPROXY_CONTAINS_FN,
        crate::molt_types_mappingproxy_contains as usize as u64,
        2,
    );
    let get_bits = builtin_func_bits(
        _py,
        &MAPPINGPROXY_GET_FN,
        crate::molt_types_mappingproxy_get as usize as u64,
        3,
    );
    let keys_bits = builtin_func_bits(
        _py,
        &MAPPINGPROXY_KEYS_FN,
        crate::molt_types_mappingproxy_keys as usize as u64,
        1,
    );
    let items_bits = builtin_func_bits(
        _py,
        &MAPPINGPROXY_ITEMS_FN,
        crate::molt_types_mappingproxy_items as usize as u64,
        1,
    );
    let values_bits = builtin_func_bits(
        _py,
        &MAPPINGPROXY_VALUES_FN,
        crate::molt_types_mappingproxy_values as usize as u64,
        1,
    );
    let repr_bits = builtin_func_bits(
        _py,
        &MAPPINGPROXY_REPR_FN,
        crate::molt_types_mappingproxy_repr as usize as u64,
        1,
    );
    let setitem_bits = builtin_func_bits(
        _py,
        &MAPPINGPROXY_SETITEM_FN,
        crate::molt_types_mappingproxy_setitem as usize as u64,
        3,
    );
    let delitem_bits = builtin_func_bits(
        _py,
        &MAPPINGPROXY_DELITEM_FN,
        crate::molt_types_mappingproxy_delitem as usize as u64,
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

pub(crate) fn method_class(_py: &PyToken<'_>) -> u64 {
    let class_bits = types_class(_py, &METHOD_CLASS, "method", 16);
    let new_bits = builtin_func_bits(
        _py,
        &METHOD_NEW_FN,
        crate::molt_types_method_new as usize as u64,
        3,
    );
    let init_bits = builtin_func_bits(
        _py,
        &METHOD_INIT_FN,
        crate::molt_types_method_init as usize as u64,
        3,
    );
    set_class_method(_py, class_bits, "__new__", new_bits);
    set_class_method(_py, class_bits, "__init__", init_bits);
    class_bits
}

fn simplenamespace_class(_py: &PyToken<'_>) -> u64 {
    let class_bits = types_class(_py, &SIMPLENAMESPACE_CLASS, "SimpleNamespace", 8);
    let init_bits = builtin_func_bits(
        _py,
        &SIMPLENAMESPACE_INIT_FN,
        crate::molt_types_simplenamespace_init as usize as u64,
        3,
    );
    let repr_bits = builtin_func_bits(
        _py,
        &SIMPLENAMESPACE_REPR_FN,
        crate::molt_types_simplenamespace_repr as usize as u64,
        1,
    );
    let eq_bits = builtin_func_bits(
        _py,
        &SIMPLENAMESPACE_EQ_FN,
        crate::molt_types_simplenamespace_eq as usize as u64,
        2,
    );
    set_class_method(_py, class_bits, "__init__", init_bits);
    set_class_method(_py, class_bits, "__repr__", repr_bits);
    set_class_method(_py, class_bits, "__eq__", eq_bits);
    mark_vararg_method(_py, init_bits, true);
    class_bits
}

fn capsule_class(_py: &PyToken<'_>) -> u64 {
    let class_bits = types_class(_py, &CAPSULE_CLASS, "capsule", 8);
    let new_bits = builtin_func_bits(
        _py,
        &CAPSULE_NEW_FN,
        crate::molt_types_capsule_new as usize as u64,
        1,
    );
    set_class_method(_py, class_bits, "__new__", new_bits);
    class_bits
}

pub(crate) fn cell_class(_py: &PyToken<'_>) -> u64 {
    let class_bits = types_class(_py, &CELL_CLASS, "cell", 8);
    let new_bits = builtin_func_bits(
        _py,
        &CELL_NEW_FN,
        crate::molt_types_cell_new as usize as u64,
        1,
    );
    set_class_method(_py, class_bits, "__new__", new_bits);
    class_bits
}

fn iter_next_pair(_py: &PyToken<'_>, iter_bits: u64) -> Option<(u64, bool)> {
    let pair_bits = molt_iter_next(iter_bits);
    let pair_obj = obj_from_bits(pair_bits);
    let Some(pair_ptr) = pair_obj.as_ptr() else {
        return None;
    };
    unsafe {
        if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
            let _ = raise_exception::<u64>(_py, "TypeError", "object is not an iterator");
            return None;
        }
        let elems = seq_vec_ref(pair_ptr);
        if elems.len() < 2 {
            let _ = raise_exception::<u64>(_py, "TypeError", "object is not an iterator");
            return None;
        }
        let val_bits = elems[0];
        let done_bits = elems[1];
        let done = is_truthy(_py, obj_from_bits(done_bits));
        Some((val_bits, done))
    }
}

#[no_mangle]
pub extern "C" fn molt_types_method_new(_cls_bits: u64, func_bits: u64, self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if obj_from_bits(self_bits).is_none() {
            inc_ref_bits(_py, func_bits);
            return func_bits;
        }
        crate::molt_bound_method_new(func_bits, self_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_types_method_init(_self_bits: u64, _func_bits: u64, _self_arg: u64) -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::none().bits() })
}

#[no_mangle]
pub extern "C" fn molt_types_mappingproxy_new(cls_bits: u64, mapping_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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

#[no_mangle]
pub extern "C" fn molt_types_mappingproxy_init(_self_bits: u64, _mapping_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::none().bits() })
}

#[no_mangle]
pub extern "C" fn molt_types_mappingproxy_getitem(self_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let mapping_bits = unsafe { mappingproxy_mapping_bits(self_ptr) };
        molt_index(mapping_bits, key_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_types_mappingproxy_iter(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let mapping_bits = unsafe { mappingproxy_mapping_bits(self_ptr) };
        let iter_bits = molt_iter(mapping_bits);
        if obj_from_bits(iter_bits).is_none() {
            return raise_not_iterable(_py, mapping_bits);
        }
        iter_bits
    })
}

#[no_mangle]
pub extern "C" fn molt_types_mappingproxy_len(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let mapping_bits = unsafe { mappingproxy_mapping_bits(self_ptr) };
        molt_len(mapping_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_types_mappingproxy_contains(self_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_ptr = obj_from_bits(self_bits).as_ptr().unwrap();
        let mapping_bits = unsafe { mappingproxy_mapping_bits(self_ptr) };
        molt_contains(mapping_bits, key_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_types_mappingproxy_get(
    self_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
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

#[no_mangle]
pub extern "C" fn molt_types_mappingproxy_keys(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { mappingproxy_call_noargs(_py, self_bits, "keys") })
}

#[no_mangle]
pub extern "C" fn molt_types_mappingproxy_items(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { mappingproxy_call_noargs(_py, self_bits, "items") })
}

#[no_mangle]
pub extern "C" fn molt_types_mappingproxy_values(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { mappingproxy_call_noargs(_py, self_bits, "values") })
}

#[no_mangle]
pub extern "C" fn molt_types_mappingproxy_repr(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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

#[no_mangle]
pub extern "C" fn molt_types_mappingproxy_setitem(
    _self_bits: u64,
    _key_bits: u64,
    _val_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_exception::<_>(
            _py,
            "TypeError",
            "'mappingproxy' object does not support item assignment",
        )
    })
}

#[no_mangle]
pub extern "C" fn molt_types_mappingproxy_delitem(_self_bits: u64, _key_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_exception::<_>(
            _py,
            "TypeError",
            "'mappingproxy' object does not support item deletion",
        )
    })
}

#[no_mangle]
pub extern "C" fn molt_types_capsule_new(_cls_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_exception::<_>(_py, "TypeError", "cannot create 'capsule' instances")
    })
}

#[no_mangle]
pub extern "C" fn molt_types_cell_new(_cls_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_exception::<_>(_py, "TypeError", "cannot create 'cell' instances")
    })
}

type FutureRelease = (i64, i64, i64, &'static str, i64);

struct FutureFeatureEntry {
    name: &'static str,
    optional: FutureRelease,
    mandatory: Option<FutureRelease>,
    compiler_flag: i64,
}

const FUTURE_FEATURES: &[FutureFeatureEntry] = &[
    FutureFeatureEntry {
        name: "nested_scopes",
        optional: (2, 1, 0, "beta", 1),
        mandatory: Some((2, 2, 0, "alpha", 0)),
        compiler_flag: 0x0010,
    },
    FutureFeatureEntry {
        name: "generators",
        optional: (2, 2, 0, "alpha", 1),
        mandatory: Some((2, 3, 0, "final", 0)),
        compiler_flag: 0,
    },
    FutureFeatureEntry {
        name: "division",
        optional: (2, 2, 0, "alpha", 2),
        mandatory: Some((3, 0, 0, "alpha", 0)),
        compiler_flag: 0x20000,
    },
    FutureFeatureEntry {
        name: "absolute_import",
        optional: (2, 5, 0, "alpha", 1),
        mandatory: Some((3, 0, 0, "alpha", 0)),
        compiler_flag: 0x40000,
    },
    FutureFeatureEntry {
        name: "with_statement",
        optional: (2, 5, 0, "alpha", 1),
        mandatory: Some((2, 6, 0, "alpha", 0)),
        compiler_flag: 0x80000,
    },
    FutureFeatureEntry {
        name: "print_function",
        optional: (2, 6, 0, "alpha", 2),
        mandatory: Some((3, 0, 0, "alpha", 0)),
        compiler_flag: 0x100000,
    },
    FutureFeatureEntry {
        name: "unicode_literals",
        optional: (2, 6, 0, "alpha", 2),
        mandatory: Some((3, 0, 0, "alpha", 0)),
        compiler_flag: 0x200000,
    },
    FutureFeatureEntry {
        name: "barry_as_FLUFL",
        optional: (3, 1, 0, "alpha", 2),
        mandatory: Some((4, 0, 0, "alpha", 0)),
        compiler_flag: 0x400000,
    },
    FutureFeatureEntry {
        name: "generator_stop",
        optional: (3, 5, 0, "beta", 1),
        mandatory: Some((3, 7, 0, "alpha", 0)),
        compiler_flag: 0x800000,
    },
    FutureFeatureEntry {
        name: "annotations",
        optional: (3, 7, 0, "beta", 1),
        mandatory: None,
        compiler_flag: 0x1000000,
    },
];

const HARD_KEYWORDS: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue",
    "def", "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import",
    "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while",
    "with", "yield",
];

const SOFT_KEYWORDS: &[&str] = &["_", "case", "match", "type"];

fn alloc_str_list_bits(_py: &PyToken<'_>, words: &[&str]) -> Option<u64> {
    let mut elems: Vec<u64> = Vec::with_capacity(words.len());
    for word in words {
        let ptr = alloc_string(_py, word.as_bytes());
        if ptr.is_null() {
            for bits in elems {
                dec_ref_bits(_py, bits);
            }
            return None;
        }
        elems.push(MoltObject::from_ptr(ptr).bits());
    }
    let list_ptr = alloc_list(_py, &elems);
    if list_ptr.is_null() {
        for bits in elems {
            dec_ref_bits(_py, bits);
        }
        return None;
    }
    for bits in elems {
        dec_ref_bits(_py, bits);
    }
    Some(MoltObject::from_ptr(list_ptr).bits())
}

fn alloc_release_tuple_bits(_py: &PyToken<'_>, rel: FutureRelease) -> Option<u64> {
    let release_ptr = alloc_string(_py, rel.3.as_bytes());
    if release_ptr.is_null() {
        return None;
    }
    let release_bits = MoltObject::from_ptr(release_ptr).bits();
    let parts = [
        MoltObject::from_int(rel.0).bits(),
        MoltObject::from_int(rel.1).bits(),
        MoltObject::from_int(rel.2).bits(),
        release_bits,
        MoltObject::from_int(rel.4).bits(),
    ];
    let tuple_ptr = alloc_tuple(_py, &parts);
    dec_ref_bits(_py, release_bits);
    if tuple_ptr.is_null() {
        return None;
    }
    Some(MoltObject::from_ptr(tuple_ptr).bits())
}

#[no_mangle]
pub extern "C" fn molt_keyword_lists() -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(kwlist_bits) = alloc_str_list_bits(_py, HARD_KEYWORDS) else {
            return MoltObject::none().bits();
        };
        let Some(softkwlist_bits) = alloc_str_list_bits(_py, SOFT_KEYWORDS) else {
            dec_ref_bits(_py, kwlist_bits);
            return MoltObject::none().bits();
        };
        let pair_ptr = alloc_tuple(_py, &[kwlist_bits, softkwlist_bits]);
        dec_ref_bits(_py, kwlist_bits);
        dec_ref_bits(_py, softkwlist_bits);
        if pair_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(pair_ptr).bits()
    })
}

fn keyword_contains(value_bits: u64, keywords: &[&str]) -> bool {
    let value_obj = obj_from_bits(value_bits);
    let Some(value_ptr) = value_obj.as_ptr() else {
        return false;
    };
    unsafe {
        if object_type_id(value_ptr) != TYPE_ID_STRING {
            return false;
        }
    }
    let Some(value) = string_obj_to_owned(value_obj) else {
        return false;
    };
    keywords.contains(&value.as_str())
}

#[no_mangle]
pub extern "C" fn molt_keyword_iskeyword(value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        MoltObject::from_bool(keyword_contains(value_bits, HARD_KEYWORDS)).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_keyword_issoftkeyword(value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        MoltObject::from_bool(keyword_contains(value_bits, SOFT_KEYWORDS)).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_future_features() -> u64 {
    crate::with_gil_entry!(_py, {
        let mut rows: Vec<u64> = Vec::with_capacity(FUTURE_FEATURES.len());
        for feature in FUTURE_FEATURES {
            let name_ptr = alloc_string(_py, feature.name.as_bytes());
            if name_ptr.is_null() {
                for bits in rows {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let Some(optional_bits) = alloc_release_tuple_bits(_py, feature.optional) else {
                dec_ref_bits(_py, name_bits);
                for bits in rows {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            };
            let mandatory_bits = if let Some(mandatory) = feature.mandatory {
                let Some(bits) = alloc_release_tuple_bits(_py, mandatory) else {
                    dec_ref_bits(_py, name_bits);
                    dec_ref_bits(_py, optional_bits);
                    for bits in rows {
                        dec_ref_bits(_py, bits);
                    }
                    return MoltObject::none().bits();
                };
                bits
            } else {
                MoltObject::none().bits()
            };
            let compiler_flag_bits = MoltObject::from_int(feature.compiler_flag).bits();
            let row_ptr = alloc_tuple(
                _py,
                &[name_bits, optional_bits, mandatory_bits, compiler_flag_bits],
            );
            dec_ref_bits(_py, name_bits);
            dec_ref_bits(_py, optional_bits);
            if !obj_from_bits(mandatory_bits).is_none() {
                dec_ref_bits(_py, mandatory_bits);
            }
            if row_ptr.is_null() {
                for bits in rows {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            rows.push(MoltObject::from_ptr(row_ptr).bits());
        }
        let list_ptr = alloc_list(_py, &rows);
        if list_ptr.is_null() {
            for bits in rows {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        for bits in rows {
            dec_ref_bits(_py, bits);
        }
        MoltObject::from_ptr(list_ptr).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_stdlib_probe() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(true).bits() })
}

#[no_mangle]
pub extern "C" fn molt_types_simplenamespace_init(
    self_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
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

#[no_mangle]
pub extern "C" fn molt_types_simplenamespace_repr(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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

#[no_mangle]
pub extern "C" fn molt_types_simplenamespace_eq(self_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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

#[no_mangle]
pub extern "C" fn molt_types_coroutine(func_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__molt_is_coroutine__") else {
            return MoltObject::none().bits();
        };
        let _ = molt_object_setattr(func_bits, name_bits, MoltObject::from_bool(true).bits());
        dec_ref_bits(_py, name_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        inc_ref_bits(_py, func_bits);
        func_bits
    })
}

struct PreparedClassState {
    metaclass_bits: u64,
    namespace_bits: u64,
    kwds_bits: u64,
}

struct AttrValue {
    bits: u64,
    owned: bool,
}

fn dynamic_class_attr_get(
    _py: &PyToken<'_>,
    obj_bits: u64,
    name: &str,
    default_bits: u64,
) -> Option<AttrValue> {
    let missing = missing_bits(_py);
    let name_bits = attr_name_bits_from_bytes(_py, name.as_bytes())?;
    let val_bits = crate::molt_get_attr_name_default(obj_bits, name_bits, missing);
    dec_ref_bits(_py, name_bits);
    if exception_pending(_py) {
        return None;
    }
    if val_bits == missing {
        return Some(AttrValue {
            bits: default_bits,
            owned: false,
        });
    }
    Some(AttrValue {
        bits: val_bits,
        owned: true,
    })
}

fn dynamic_class_attr_set(_py: &PyToken<'_>, obj_bits: u64, name: &str, value_bits: u64) -> bool {
    let Some(name_bits) = attr_name_bits_from_bytes(_py, name.as_bytes()) else {
        return false;
    };
    let _ = molt_object_setattr(obj_bits, name_bits, value_bits);
    dec_ref_bits(_py, name_bits);
    !exception_pending(_py)
}

fn call_vararg_args(_py: &PyToken<'_>, func_name: &str, args_bits: u64) -> Option<Vec<u64>> {
    let Some(args_ptr) = obj_from_bits(args_bits).as_ptr() else {
        let msg = format!("{func_name}() expects positional arguments tuple");
        let _ = raise_exception::<u64>(_py, "TypeError", &msg);
        return None;
    };
    unsafe {
        if object_type_id(args_ptr) != TYPE_ID_TUPLE {
            let msg = format!("{func_name}() expects positional arguments tuple");
            let _ = raise_exception::<u64>(_py, "TypeError", &msg);
            return None;
        }
    }
    Some(unsafe { seq_vec_ref(args_ptr) }.clone())
}

fn call_vararg_kwargs(
    _py: &PyToken<'_>,
    func_name: &str,
    kwargs_bits: u64,
) -> Option<(*mut u8, Vec<(String, u64)>)> {
    let Some(kwargs_ptr) = obj_from_bits(kwargs_bits).as_ptr() else {
        let msg = format!("{func_name}() expects keyword arguments dict");
        let _ = raise_exception::<u64>(_py, "TypeError", &msg);
        return None;
    };
    unsafe {
        if object_type_id(kwargs_ptr) != TYPE_ID_DICT {
            let msg = format!("{func_name}() expects keyword arguments dict");
            let _ = raise_exception::<u64>(_py, "TypeError", &msg);
            return None;
        }
    }
    let order = unsafe { dict_order(kwargs_ptr) }.clone();
    let mut entries = Vec::with_capacity(order.len() / 2);
    for pair in order.chunks(2) {
        if pair.len() != 2 {
            continue;
        }
        let key_name = match string_obj_to_owned(obj_from_bits(pair[0])) {
            Some(name) => name,
            None => {
                let _ = raise_exception::<u64>(_py, "TypeError", "keywords must be strings");
                return None;
            }
        };
        entries.push((key_name, pair[1]));
    }
    Some((kwargs_ptr, entries))
}

fn copy_kwds_mapping(_py: &PyToken<'_>, kwds_bits: u64) -> Option<u64> {
    let dict_ptr = alloc_dict_with_pairs(_py, &[]);
    if dict_ptr.is_null() {
        return None;
    }
    let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
    if !obj_from_bits(kwds_bits).is_none() {
        unsafe {
            let _ = dict_update_apply(_py, dict_bits, dict_update_set_in_place, kwds_bits);
        }
        if exception_pending(_py) {
            dec_ref_bits(_py, dict_bits);
            return None;
        }
    }
    Some(dict_bits)
}

fn call_with_kwargs(
    _py: &PyToken<'_>,
    callable_bits: u64,
    positional: &[u64],
    kwargs_bits: u64,
) -> u64 {
    let mut kw_pairs: Vec<(u64, u64)> = Vec::new();
    if let Some(kwargs_ptr) = obj_from_bits(kwargs_bits).as_ptr() {
        unsafe {
            if object_type_id(kwargs_ptr) != TYPE_ID_DICT {
                return raise_exception::<_>(_py, "TypeError", "keyword arguments must be a dict");
            }
            let order = dict_order(kwargs_ptr).clone();
            for pair in order.chunks(2) {
                if pair.len() == 2 {
                    kw_pairs.push((pair[0], pair[1]));
                }
            }
        }
    }
    let builder_bits = molt_callargs_new(
        (positional.len() + kw_pairs.len()) as u64,
        kw_pairs.len() as u64,
    );
    if builder_bits == 0 {
        return MoltObject::none().bits();
    }
    for val_bits in positional.iter().copied() {
        unsafe {
            let _ = molt_callargs_push_pos(builder_bits, val_bits);
        }
    }
    for (name_bits, val_bits) in kw_pairs.iter().copied() {
        unsafe {
            let _ = molt_callargs_push_kw(builder_bits, name_bits, val_bits);
        }
    }
    molt_call_bind(callable_bits, builder_bits)
}

fn validate_make_dataclass_field_name(_py: &PyToken<'_>, name_bits: u64) -> Option<String> {
    if !isinstance_runtime(_py, name_bits, builtin_classes(_py).str) {
        let _ = raise_exception::<u64>(_py, "TypeError", "Field names must be strings");
        return None;
    }

    let name_str_bits = molt_str_from_obj(name_bits);
    if obj_from_bits(name_str_bits).is_none() {
        return None;
    }

    let ident_bits = molt_string_isidentifier(name_str_bits);
    let is_ident = is_truthy(_py, obj_from_bits(ident_bits));
    if obj_from_bits(ident_bits).as_ptr().is_some() {
        dec_ref_bits(_py, ident_bits);
    }
    if !is_ident || keyword_contains(name_str_bits, HARD_KEYWORDS) {
        let repr_bits = molt_repr_from_obj(name_str_bits);
        let repr = string_obj_to_owned(obj_from_bits(repr_bits)).unwrap_or_default();
        dec_ref_bits(_py, repr_bits);
        dec_ref_bits(_py, name_str_bits);
        let msg = format!("Field names must be valid identifiers: {repr}");
        let _ = raise_exception::<u64>(_py, "TypeError", &msg);
        return None;
    }
    let out = string_obj_to_owned(obj_from_bits(name_str_bits));
    dec_ref_bits(_py, name_str_bits);
    out
}

#[allow(clippy::too_many_arguments)]
#[no_mangle]
pub extern "C" fn molt_dataclasses_make_dataclass(
    cls_name_bits: u64,
    fields_bits: u64,
    bases_bits: u64,
    namespace_bits: u64,
    module_bits: u64,
    default_field_type_bits: u64,
    _field_class_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let mut result_bits = MoltObject::none().bits();
        let mut bases_tuple_bits = 0u64;
        let mut body_bits = 0u64;
        let mut annotations_bits = 0u64;
        let mut fields_iter_bits = 0u64;

        'compute: {
            let Some(cls_name_ptr) = obj_from_bits(cls_name_bits).as_ptr() else {
                result_bits = raise_exception::<_>(_py, "TypeError", "cls_name must be a string");
                break 'compute;
            };
            unsafe {
                if object_type_id(cls_name_ptr) != TYPE_ID_STRING {
                    result_bits =
                        raise_exception::<_>(_py, "TypeError", "cls_name must be a string");
                    break 'compute;
                }
            }

            bases_tuple_bits = if obj_from_bits(bases_bits)
                .as_ptr()
                .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_TUPLE })
            {
                inc_ref_bits(_py, bases_bits);
                bases_bits
            } else {
                let Some(bits) = (unsafe { tuple_from_iter_bits(_py, bases_bits) }) else {
                    result_bits = MoltObject::none().bits();
                    break 'compute;
                };
                bits
            };

            if obj_from_bits(namespace_bits).is_none() {
                let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                if dict_ptr.is_null() {
                    result_bits = MoltObject::none().bits();
                    break 'compute;
                }
                body_bits = MoltObject::from_ptr(dict_ptr).bits();
            } else {
                body_bits = molt_dict_from_obj(namespace_bits);
                if obj_from_bits(body_bits).is_none() {
                    result_bits = MoltObject::none().bits();
                    break 'compute;
                }
            }
            let Some(body_ptr) = obj_from_bits(body_bits).as_ptr() else {
                result_bits = MoltObject::none().bits();
                break 'compute;
            };
            unsafe {
                if object_type_id(body_ptr) != TYPE_ID_DICT {
                    result_bits = MoltObject::none().bits();
                    break 'compute;
                }
            }
            let Some(init_name_bits) = attr_name_bits_from_bytes(_py, b"__init__") else {
                result_bits = MoltObject::none().bits();
                break 'compute;
            };
            let has_user_init =
                unsafe { dict_get_in_place(_py, body_ptr, init_name_bits) }.is_some();
            dec_ref_bits(_py, init_name_bits);
            let Some(user_init_marker_bits) =
                attr_name_bits_from_bytes(_py, b"__molt_dataclass_user_init__")
            else {
                result_bits = MoltObject::none().bits();
                break 'compute;
            };
            unsafe {
                dict_set_in_place(
                    _py,
                    body_ptr,
                    user_init_marker_bits,
                    MoltObject::from_bool(has_user_init).bits(),
                );
            }
            dec_ref_bits(_py, user_init_marker_bits);
            if exception_pending(_py) {
                result_bits = MoltObject::none().bits();
                break 'compute;
            }
            let Some(make_dataclass_marker_bits) =
                attr_name_bits_from_bytes(_py, b"__molt_make_dataclass__")
            else {
                result_bits = MoltObject::none().bits();
                break 'compute;
            };
            unsafe {
                dict_set_in_place(
                    _py,
                    body_ptr,
                    make_dataclass_marker_bits,
                    MoltObject::from_bool(true).bits(),
                );
            }
            dec_ref_bits(_py, make_dataclass_marker_bits);
            if exception_pending(_py) {
                result_bits = MoltObject::none().bits();
                break 'compute;
            }

            let Some(annotations_name_bits) = attr_name_bits_from_bytes(_py, b"__annotations__")
            else {
                result_bits = MoltObject::none().bits();
                break 'compute;
            };
            let existing_annotations_bits =
                unsafe { dict_get_in_place(_py, body_ptr, annotations_name_bits) };
            dec_ref_bits(_py, annotations_name_bits);
            annotations_bits = if let Some(bits) = existing_annotations_bits {
                let Some(existing_ptr) = obj_from_bits(bits).as_ptr() else {
                    result_bits =
                        raise_exception::<_>(_py, "TypeError", "__annotations__ must be a dict");
                    break 'compute;
                };
                unsafe {
                    if object_type_id(existing_ptr) != TYPE_ID_DICT {
                        result_bits = raise_exception::<_>(
                            _py,
                            "TypeError",
                            "__annotations__ must be a dict",
                        );
                        break 'compute;
                    }
                }
                let copied_bits = molt_dict_from_obj(bits);
                if obj_from_bits(copied_bits).is_none() {
                    result_bits = MoltObject::none().bits();
                    break 'compute;
                }
                copied_bits
            } else {
                let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                if dict_ptr.is_null() {
                    result_bits = MoltObject::none().bits();
                    break 'compute;
                }
                MoltObject::from_ptr(dict_ptr).bits()
            };
            let Some(annotations_ptr) = obj_from_bits(annotations_bits).as_ptr() else {
                result_bits = MoltObject::none().bits();
                break 'compute;
            };
            unsafe {
                if object_type_id(annotations_ptr) != TYPE_ID_DICT {
                    result_bits = MoltObject::none().bits();
                    break 'compute;
                }
            }

            let mut seen: HashSet<String> = HashSet::new();
            let annotation_order = unsafe { dict_order(annotations_ptr) }.clone();
            for pair in annotation_order.chunks(2) {
                if pair.len() != 2 {
                    continue;
                }
                if let Some(name) = string_obj_to_owned(obj_from_bits(pair[0])) {
                    seen.insert(name);
                }
            }

            fields_iter_bits = molt_iter(fields_bits);
            if exception_pending(_py) {
                result_bits = MoltObject::none().bits();
                break 'compute;
            }
            loop {
                let Some((field_spec_bits, done)) = iter_next_pair(_py, fields_iter_bits) else {
                    result_bits = MoltObject::none().bits();
                    break 'compute;
                };
                if done {
                    break;
                }

                let mut raw_name_bits = field_spec_bits;
                let mut field_type_bits = default_field_type_bits;
                let mut default_value_bits = 0u64;
                let mut has_default_value = false;
                let invalid_spec_msg = "Invalid field specification: must be name, (name, type), or (name, type, Field)";

                let Some(field_spec_ptr) = obj_from_bits(field_spec_bits).as_ptr() else {
                    result_bits = raise_exception::<_>(_py, "TypeError", invalid_spec_msg);
                    break 'compute;
                };

                unsafe {
                    match object_type_id(field_spec_ptr) {
                        TYPE_ID_STRING => {}
                        TYPE_ID_TUPLE | TYPE_ID_LIST => {
                            let parts = seq_vec_ref(field_spec_ptr).clone();
                            if parts.len() == 2 {
                                raw_name_bits = parts[0];
                                field_type_bits = parts[1];
                            } else if parts.len() == 3 {
                                raw_name_bits = parts[0];
                                field_type_bits = parts[1];
                                default_value_bits = parts[2];
                                has_default_value = true;
                            } else {
                                result_bits =
                                    raise_exception::<_>(_py, "TypeError", invalid_spec_msg);
                                break 'compute;
                            }
                        }
                        _ => {
                            result_bits = raise_exception::<_>(_py, "TypeError", invalid_spec_msg);
                            break 'compute;
                        }
                    }
                }

                let Some(field_name) = validate_make_dataclass_field_name(_py, raw_name_bits)
                else {
                    result_bits = MoltObject::none().bits();
                    break 'compute;
                };

                if seen.contains(field_name.as_str()) {
                    let field_name_repr_bits = molt_repr_from_obj(raw_name_bits);
                    let field_name_repr = string_obj_to_owned(obj_from_bits(field_name_repr_bits))
                        .unwrap_or_default();
                    dec_ref_bits(_py, field_name_repr_bits);
                    let msg = format!("Field name duplicated: {field_name_repr}");
                    result_bits = raise_exception::<_>(_py, "TypeError", &msg);
                    break 'compute;
                }
                seen.insert(field_name.clone());

                let key_ptr = alloc_string(_py, field_name.as_bytes());
                if key_ptr.is_null() {
                    result_bits = MoltObject::none().bits();
                    break 'compute;
                }
                let key_bits = MoltObject::from_ptr(key_ptr).bits();
                unsafe {
                    dict_set_in_place(_py, annotations_ptr, key_bits, field_type_bits);
                }
                if exception_pending(_py) {
                    dec_ref_bits(_py, key_bits);
                    result_bits = MoltObject::none().bits();
                    break 'compute;
                }
                if has_default_value {
                    unsafe {
                        dict_set_in_place(_py, body_ptr, key_bits, default_value_bits);
                    }
                    if exception_pending(_py) {
                        dec_ref_bits(_py, key_bits);
                        result_bits = MoltObject::none().bits();
                        break 'compute;
                    }
                }
                dec_ref_bits(_py, key_bits);
            }
            if exception_pending(_py) {
                break 'compute;
            }

            let Some(annotations_key_bits) = attr_name_bits_from_bytes(_py, b"__annotations__")
            else {
                result_bits = MoltObject::none().bits();
                break 'compute;
            };
            unsafe {
                dict_set_in_place(_py, body_ptr, annotations_key_bits, annotations_bits);
            }
            dec_ref_bits(_py, annotations_key_bits);
            if exception_pending(_py) {
                result_bits = MoltObject::none().bits();
                break 'compute;
            }

            let Some(molt_dataclass_name_bits) =
                attr_name_bits_from_bytes(_py, b"__molt_dataclass__")
            else {
                result_bits = MoltObject::none().bits();
                break 'compute;
            };
            let has_molt_dataclass =
                unsafe { dict_get_in_place(_py, body_ptr, molt_dataclass_name_bits) }.is_some();
            if !has_molt_dataclass {
                unsafe {
                    dict_set_in_place(
                        _py,
                        body_ptr,
                        molt_dataclass_name_bits,
                        MoltObject::from_bool(true).bits(),
                    );
                }
            }
            dec_ref_bits(_py, molt_dataclass_name_bits);
            if exception_pending(_py) {
                result_bits = MoltObject::none().bits();
                break 'compute;
            }

            let Some(module_name_bits) = attr_name_bits_from_bytes(_py, b"__module__") else {
                result_bits = MoltObject::none().bits();
                break 'compute;
            };
            let has_module =
                unsafe { dict_get_in_place(_py, body_ptr, module_name_bits) }.is_some();
            if !has_module {
                unsafe {
                    dict_set_in_place(_py, body_ptr, module_name_bits, module_bits);
                }
            }
            dec_ref_bits(_py, module_name_bits);
            if exception_pending(_py) {
                result_bits = MoltObject::none().bits();
                break 'compute;
            }

            let out_ptr = alloc_tuple(_py, &[bases_tuple_bits, body_bits]);
            if out_ptr.is_null() {
                result_bits = MoltObject::none().bits();
                break 'compute;
            }
            result_bits = MoltObject::from_ptr(out_ptr).bits();
        }

        if !obj_from_bits(fields_iter_bits).is_none() {
            dec_ref_bits(_py, fields_iter_bits);
        }
        if !obj_from_bits(annotations_bits).is_none() {
            dec_ref_bits(_py, annotations_bits);
        }
        if !obj_from_bits(body_bits).is_none() {
            dec_ref_bits(_py, body_bits);
        }
        if !obj_from_bits(bases_tuple_bits).is_none() {
            dec_ref_bits(_py, bases_tuple_bits);
        }

        result_bits
    })
}

fn dataclasses_class_bits(_py: &PyToken<'_>, obj_bits: u64) -> u64 {
    if obj_from_bits(obj_bits)
        .as_ptr()
        .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_TYPE })
    {
        obj_bits
    } else {
        type_of_bits(_py, obj_bits)
    }
}

fn dataclasses_fields_dict_bits(_py: &PyToken<'_>, cls_bits: u64, missing: u64) -> Option<u64> {
    let fields_name_bits = attr_name_bits_from_bytes(_py, b"__dataclass_fields__")?;
    let fields_bits = molt_getattr_builtin(cls_bits, fields_name_bits, missing);
    dec_ref_bits(_py, fields_name_bits);
    if exception_pending(_py) {
        clear_exception(_py);
        return None;
    }
    if fields_bits == missing {
        return None;
    }
    let fields_ptr = obj_from_bits(fields_bits).as_ptr()?;
    unsafe {
        if object_type_id(fields_ptr) != TYPE_ID_DICT {
            return None;
        }
    }
    Some(fields_bits)
}

fn dataclasses_collect_fields_by_tag(
    _py: &PyToken<'_>,
    fields_dict_bits: u64,
    field_tag_bits: u64,
) -> Option<Vec<u64>> {
    let fields_dict_ptr = obj_from_bits(fields_dict_bits).as_ptr()?;
    let field_type_name_bits = attr_name_bits_from_bytes(_py, b"_field_type")?;
    let missing = missing_bits(_py);
    let mut out: Vec<u64> = Vec::new();
    let order = unsafe { dict_order(fields_dict_ptr) }.clone();
    for pair in order.chunks(2) {
        if pair.len() != 2 {
            continue;
        }
        let field_obj_bits = pair[1];
        let tag_bits = molt_getattr_builtin(field_obj_bits, field_type_name_bits, missing);
        if exception_pending(_py) {
            dec_ref_bits(_py, field_type_name_bits);
            return None;
        }
        if tag_bits == field_tag_bits {
            out.push(field_obj_bits);
        }
    }
    dec_ref_bits(_py, field_type_name_bits);
    Some(out)
}

fn dataclasses_is_dataclass_instance(_py: &PyToken<'_>, obj_bits: u64, missing: u64) -> bool {
    let Some(obj_ptr) = obj_from_bits(obj_bits).as_ptr() else {
        return false;
    };
    unsafe {
        match object_type_id(obj_ptr) {
            TYPE_ID_TYPE | TYPE_ID_LIST | TYPE_ID_TUPLE | TYPE_ID_DICT => return false,
            _ => {}
        }
    }
    let cls_bits = type_of_bits(_py, obj_bits);
    dataclasses_fields_dict_bits(_py, cls_bits, missing).is_some()
}

fn dataclasses_deepcopy(_py: &PyToken<'_>, value_bits: u64) -> u64 {
    // Immediate scalar values are immutable and represented without object pointers.
    if obj_from_bits(value_bits).as_ptr().is_none() {
        inc_ref_bits(_py, value_bits);
        return value_bits;
    }
    let Some(value_ptr) = obj_from_bits(value_bits).as_ptr() else {
        return MoltObject::none().bits();
    };
    unsafe {
        let ty = object_type_id(value_ptr);
        if matches!(
            ty,
            TYPE_ID_STRING
                | TYPE_ID_BYTES
                | TYPE_ID_RANGE
                | TYPE_ID_TYPE
                | TYPE_ID_NOT_IMPLEMENTED
                | TYPE_ID_ELLIPSIS
                | TYPE_ID_COMPLEX
        ) {
            inc_ref_bits(_py, value_bits);
            return value_bits;
        }
    }

    let memo_ptr = alloc_dict_with_pairs(_py, &[]);
    if memo_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let memo_bits = MoltObject::from_ptr(memo_ptr).bits();
    let Some(deepcopy_name_bits) = attr_name_bits_from_bytes(_py, b"__deepcopy__") else {
        dec_ref_bits(_py, memo_bits);
        return MoltObject::none().bits();
    };
    let missing = missing_bits(_py);
    let deepcopy_bits = molt_getattr_builtin(value_bits, deepcopy_name_bits, missing);
    dec_ref_bits(_py, deepcopy_name_bits);
    if exception_pending(_py) {
        if !crate::builtins::attr::clear_attribute_error_if_pending(_py) {
            dec_ref_bits(_py, memo_bits);
            return MoltObject::none().bits();
        }
        dec_ref_bits(_py, memo_bits);
        inc_ref_bits(_py, value_bits);
        return value_bits;
    }
    if deepcopy_bits == missing {
        dec_ref_bits(_py, memo_bits);
        inc_ref_bits(_py, value_bits);
        return value_bits;
    }
    let out_bits = unsafe { call_callable1(_py, deepcopy_bits, memo_bits) };
    dec_ref_bits(_py, deepcopy_bits);
    dec_ref_bits(_py, memo_bits);
    out_bits
}

fn dataclasses_asdict_inner(
    _py: &PyToken<'_>,
    value_bits: u64,
    dict_factory_bits: u64,
    field_tag_bits: u64,
) -> u64 {
    let missing = missing_bits(_py);
    if dataclasses_is_dataclass_instance(_py, value_bits, missing) {
        let cls_bits = type_of_bits(_py, value_bits);
        let Some(fields_dict_bits) = dataclasses_fields_dict_bits(_py, cls_bits, missing) else {
            return MoltObject::none().bits();
        };
        let Some(field_objs) =
            dataclasses_collect_fields_by_tag(_py, fields_dict_bits, field_tag_bits)
        else {
            return MoltObject::none().bits();
        };
        let Some(name_name_bits) = attr_name_bits_from_bytes(_py, b"name") else {
            return MoltObject::none().bits();
        };
        let mut item_bits: Vec<u64> = Vec::with_capacity(field_objs.len());
        for field_obj_bits in field_objs {
            let name_bits = molt_getattr_builtin(field_obj_bits, name_name_bits, missing);
            if exception_pending(_py) || name_bits == missing {
                dec_ref_bits(_py, name_name_bits);
                for bits in item_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let field_val_bits = molt_getattr_builtin(value_bits, name_bits, missing);
            if exception_pending(_py) || field_val_bits == missing {
                dec_ref_bits(_py, name_name_bits);
                for bits in item_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let copied_bits =
                dataclasses_asdict_inner(_py, field_val_bits, dict_factory_bits, field_tag_bits);
            if obj_from_bits(copied_bits).is_none() {
                dec_ref_bits(_py, name_name_bits);
                for bits in item_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let pair_ptr = alloc_tuple(_py, &[name_bits, copied_bits]);
            dec_ref_bits(_py, copied_bits);
            if pair_ptr.is_null() {
                dec_ref_bits(_py, name_name_bits);
                for bits in item_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            item_bits.push(MoltObject::from_ptr(pair_ptr).bits());
        }
        dec_ref_bits(_py, name_name_bits);
        let items_list_ptr = alloc_list(_py, item_bits.as_slice());
        for bits in item_bits {
            dec_ref_bits(_py, bits);
        }
        if items_list_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let items_list_bits = MoltObject::from_ptr(items_list_ptr).bits();
        let out_bits = unsafe { call_callable1(_py, dict_factory_bits, items_list_bits) };
        dec_ref_bits(_py, items_list_bits);
        return out_bits;
    }

    let Some(value_ptr) = obj_from_bits(value_bits).as_ptr() else {
        return dataclasses_deepcopy(_py, value_bits);
    };
    unsafe {
        match object_type_id(value_ptr) {
            TYPE_ID_LIST => {
                let elems = seq_vec_ref(value_ptr).clone();
                let mut copied: Vec<u64> = Vec::with_capacity(elems.len());
                for elem_bits in elems {
                    let inner_bits =
                        dataclasses_asdict_inner(_py, elem_bits, dict_factory_bits, field_tag_bits);
                    if obj_from_bits(inner_bits).is_none() {
                        for bits in copied {
                            dec_ref_bits(_py, bits);
                        }
                        return MoltObject::none().bits();
                    }
                    copied.push(inner_bits);
                }
                let copied_list_ptr = alloc_list(_py, copied.as_slice());
                for bits in copied {
                    dec_ref_bits(_py, bits);
                }
                if copied_list_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let copied_list_bits = MoltObject::from_ptr(copied_list_ptr).bits();
                let cls_bits = type_of_bits(_py, value_bits);
                let out_bits = call_callable1(_py, cls_bits, copied_list_bits);
                dec_ref_bits(_py, copied_list_bits);
                return out_bits;
            }
            TYPE_ID_TUPLE => {
                let elems = seq_vec_ref(value_ptr).clone();
                let mut copied: Vec<u64> = Vec::with_capacity(elems.len());
                for elem_bits in elems {
                    let inner_bits =
                        dataclasses_asdict_inner(_py, elem_bits, dict_factory_bits, field_tag_bits);
                    if obj_from_bits(inner_bits).is_none() {
                        for bits in copied {
                            dec_ref_bits(_py, bits);
                        }
                        return MoltObject::none().bits();
                    }
                    copied.push(inner_bits);
                }
                let copied_list_ptr = alloc_list(_py, copied.as_slice());
                for bits in copied {
                    dec_ref_bits(_py, bits);
                }
                if copied_list_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let copied_list_bits = MoltObject::from_ptr(copied_list_ptr).bits();
                let cls_bits = type_of_bits(_py, value_bits);
                let out_bits = call_callable1(_py, cls_bits, copied_list_bits);
                dec_ref_bits(_py, copied_list_bits);
                return out_bits;
            }
            TYPE_ID_DICT => {
                let mut pair_bits: Vec<u64> = Vec::new();
                let order = dict_order(value_ptr).clone();
                for pair in order.chunks(2) {
                    if pair.len() != 2 {
                        continue;
                    }
                    let key_bits =
                        dataclasses_asdict_inner(_py, pair[0], dict_factory_bits, field_tag_bits);
                    if obj_from_bits(key_bits).is_none() {
                        for bits in pair_bits {
                            dec_ref_bits(_py, bits);
                        }
                        return MoltObject::none().bits();
                    }
                    let val_bits =
                        dataclasses_asdict_inner(_py, pair[1], dict_factory_bits, field_tag_bits);
                    if obj_from_bits(val_bits).is_none() {
                        dec_ref_bits(_py, key_bits);
                        for bits in pair_bits {
                            dec_ref_bits(_py, bits);
                        }
                        return MoltObject::none().bits();
                    }
                    let tuple_ptr = alloc_tuple(_py, &[key_bits, val_bits]);
                    dec_ref_bits(_py, key_bits);
                    dec_ref_bits(_py, val_bits);
                    if tuple_ptr.is_null() {
                        for bits in pair_bits {
                            dec_ref_bits(_py, bits);
                        }
                        return MoltObject::none().bits();
                    }
                    pair_bits.push(MoltObject::from_ptr(tuple_ptr).bits());
                }
                let pairs_list_ptr = alloc_list(_py, pair_bits.as_slice());
                for bits in pair_bits {
                    dec_ref_bits(_py, bits);
                }
                if pairs_list_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let pairs_list_bits = MoltObject::from_ptr(pairs_list_ptr).bits();
                let cls_bits = type_of_bits(_py, value_bits);
                let out_bits = call_callable1(_py, cls_bits, pairs_list_bits);
                dec_ref_bits(_py, pairs_list_bits);
                return out_bits;
            }
            _ => {}
        }
    }
    dataclasses_deepcopy(_py, value_bits)
}

fn dataclasses_astuple_inner(
    _py: &PyToken<'_>,
    value_bits: u64,
    tuple_factory_bits: u64,
    field_tag_bits: u64,
) -> u64 {
    let missing = missing_bits(_py);
    if dataclasses_is_dataclass_instance(_py, value_bits, missing) {
        let cls_bits = type_of_bits(_py, value_bits);
        let Some(fields_dict_bits) = dataclasses_fields_dict_bits(_py, cls_bits, missing) else {
            return MoltObject::none().bits();
        };
        let Some(field_objs) =
            dataclasses_collect_fields_by_tag(_py, fields_dict_bits, field_tag_bits)
        else {
            return MoltObject::none().bits();
        };
        let Some(name_name_bits) = attr_name_bits_from_bytes(_py, b"name") else {
            return MoltObject::none().bits();
        };
        let mut values: Vec<u64> = Vec::with_capacity(field_objs.len());
        for field_obj_bits in field_objs {
            let name_bits = molt_getattr_builtin(field_obj_bits, name_name_bits, missing);
            if exception_pending(_py) || name_bits == missing {
                dec_ref_bits(_py, name_name_bits);
                for bits in values {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let field_val_bits = molt_getattr_builtin(value_bits, name_bits, missing);
            if exception_pending(_py) || field_val_bits == missing {
                dec_ref_bits(_py, name_name_bits);
                for bits in values {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let copied_bits =
                dataclasses_astuple_inner(_py, field_val_bits, tuple_factory_bits, field_tag_bits);
            if obj_from_bits(copied_bits).is_none() {
                dec_ref_bits(_py, name_name_bits);
                for bits in values {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            values.push(copied_bits);
        }
        dec_ref_bits(_py, name_name_bits);
        let values_list_ptr = alloc_list(_py, values.as_slice());
        for bits in values {
            dec_ref_bits(_py, bits);
        }
        if values_list_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let values_list_bits = MoltObject::from_ptr(values_list_ptr).bits();
        let out_bits = unsafe { call_callable1(_py, tuple_factory_bits, values_list_bits) };
        dec_ref_bits(_py, values_list_bits);
        return out_bits;
    }

    let Some(value_ptr) = obj_from_bits(value_bits).as_ptr() else {
        return dataclasses_deepcopy(_py, value_bits);
    };
    unsafe {
        match object_type_id(value_ptr) {
            TYPE_ID_LIST => {
                let elems = seq_vec_ref(value_ptr).clone();
                let mut copied: Vec<u64> = Vec::with_capacity(elems.len());
                for elem_bits in elems {
                    let inner_bits = dataclasses_astuple_inner(
                        _py,
                        elem_bits,
                        tuple_factory_bits,
                        field_tag_bits,
                    );
                    if obj_from_bits(inner_bits).is_none() {
                        for bits in copied {
                            dec_ref_bits(_py, bits);
                        }
                        return MoltObject::none().bits();
                    }
                    copied.push(inner_bits);
                }
                let copied_list_ptr = alloc_list(_py, copied.as_slice());
                for bits in copied {
                    dec_ref_bits(_py, bits);
                }
                if copied_list_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let copied_list_bits = MoltObject::from_ptr(copied_list_ptr).bits();
                let cls_bits = type_of_bits(_py, value_bits);
                let out_bits = call_callable1(_py, cls_bits, copied_list_bits);
                dec_ref_bits(_py, copied_list_bits);
                return out_bits;
            }
            TYPE_ID_TUPLE => {
                let elems = seq_vec_ref(value_ptr).clone();
                let mut copied: Vec<u64> = Vec::with_capacity(elems.len());
                for elem_bits in elems {
                    let inner_bits = dataclasses_astuple_inner(
                        _py,
                        elem_bits,
                        tuple_factory_bits,
                        field_tag_bits,
                    );
                    if obj_from_bits(inner_bits).is_none() {
                        for bits in copied {
                            dec_ref_bits(_py, bits);
                        }
                        return MoltObject::none().bits();
                    }
                    copied.push(inner_bits);
                }
                let copied_list_ptr = alloc_list(_py, copied.as_slice());
                for bits in copied {
                    dec_ref_bits(_py, bits);
                }
                if copied_list_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let copied_list_bits = MoltObject::from_ptr(copied_list_ptr).bits();
                let cls_bits = type_of_bits(_py, value_bits);
                let out_bits = call_callable1(_py, cls_bits, copied_list_bits);
                dec_ref_bits(_py, copied_list_bits);
                return out_bits;
            }
            TYPE_ID_DICT => {
                let mut pair_bits: Vec<u64> = Vec::new();
                let order = dict_order(value_ptr).clone();
                for pair in order.chunks(2) {
                    if pair.len() != 2 {
                        continue;
                    }
                    let key_bits =
                        dataclasses_astuple_inner(_py, pair[0], tuple_factory_bits, field_tag_bits);
                    if obj_from_bits(key_bits).is_none() {
                        for bits in pair_bits {
                            dec_ref_bits(_py, bits);
                        }
                        return MoltObject::none().bits();
                    }
                    let val_bits =
                        dataclasses_astuple_inner(_py, pair[1], tuple_factory_bits, field_tag_bits);
                    if obj_from_bits(val_bits).is_none() {
                        dec_ref_bits(_py, key_bits);
                        for bits in pair_bits {
                            dec_ref_bits(_py, bits);
                        }
                        return MoltObject::none().bits();
                    }
                    let tuple_ptr = alloc_tuple(_py, &[key_bits, val_bits]);
                    dec_ref_bits(_py, key_bits);
                    dec_ref_bits(_py, val_bits);
                    if tuple_ptr.is_null() {
                        for bits in pair_bits {
                            dec_ref_bits(_py, bits);
                        }
                        return MoltObject::none().bits();
                    }
                    pair_bits.push(MoltObject::from_ptr(tuple_ptr).bits());
                }
                let pairs_list_ptr = alloc_list(_py, pair_bits.as_slice());
                for bits in pair_bits {
                    dec_ref_bits(_py, bits);
                }
                if pairs_list_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let pairs_list_bits = MoltObject::from_ptr(pairs_list_ptr).bits();
                let cls_bits = type_of_bits(_py, value_bits);
                let out_bits = call_callable1(_py, cls_bits, pairs_list_bits);
                dec_ref_bits(_py, pairs_list_bits);
                return out_bits;
            }
            _ => {}
        }
    }
    dataclasses_deepcopy(_py, value_bits)
}

#[no_mangle]
pub extern "C" fn molt_dataclasses_is_dataclass(obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let cls_bits = dataclasses_class_bits(_py, obj_bits);
        let Some(cls_ptr) = obj_from_bits(cls_bits).as_ptr() else {
            return MoltObject::from_bool(false).bits();
        };
        unsafe {
            if object_type_id(cls_ptr) != TYPE_ID_TYPE {
                return MoltObject::from_bool(false).bits();
            }
        }
        let Some(fields_name_bits) = attr_name_bits_from_bytes(_py, b"__dataclass_fields__") else {
            return MoltObject::from_bool(false).bits();
        };
        let mut has_fields = false;
        for base_bits in class_mro_vec(cls_bits) {
            let Some(base_ptr) = obj_from_bits(base_bits).as_ptr() else {
                continue;
            };
            unsafe {
                if object_type_id(base_ptr) != TYPE_ID_TYPE {
                    continue;
                }
                let dict_bits = class_dict_bits(base_ptr);
                let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                    continue;
                };
                if object_type_id(dict_ptr) != TYPE_ID_DICT {
                    continue;
                }
                if dict_get_in_place(_py, dict_ptr, fields_name_bits).is_some() {
                    has_fields = true;
                    break;
                }
            }
        }
        dec_ref_bits(_py, fields_name_bits);
        MoltObject::from_bool(has_fields).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_dataclasses_fields(class_or_instance_bits: u64, field_tag_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        let cls_bits = dataclasses_class_bits(_py, class_or_instance_bits);
        let Some(fields_dict_bits) = dataclasses_fields_dict_bits(_py, cls_bits, missing) else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "must be called with a dataclass type or instance",
            );
        };
        let Some(field_objs) =
            dataclasses_collect_fields_by_tag(_py, fields_dict_bits, field_tag_bits)
        else {
            return MoltObject::none().bits();
        };
        let out_ptr = alloc_tuple(_py, field_objs.as_slice());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_dataclasses_asdict(
    obj_bits: u64,
    dict_factory_bits: u64,
    field_tag_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        if !dataclasses_is_dataclass_instance(_py, obj_bits, missing) {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "asdict() should be called on dataclass instances",
            );
        }
        dataclasses_asdict_inner(_py, obj_bits, dict_factory_bits, field_tag_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_dataclasses_astuple(
    obj_bits: u64,
    tuple_factory_bits: u64,
    field_tag_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        if !dataclasses_is_dataclass_instance(_py, obj_bits, missing) {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "astuple() should be called on dataclass instances",
            );
        }
        dataclasses_astuple_inner(_py, obj_bits, tuple_factory_bits, field_tag_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_dataclasses_replace(
    obj_bits: u64,
    changes_bits: u64,
    field_tag_bits: u64,
    initvar_tag_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        if !dataclasses_is_dataclass_instance(_py, obj_bits, missing) {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "replace() should be called on dataclass instances",
            );
        }
        let cls_bits = type_of_bits(_py, obj_bits);
        let changes_copy_bits = molt_dict_from_obj(changes_bits);
        if obj_from_bits(changes_copy_bits).is_none() {
            return MoltObject::none().bits();
        }
        let Some(changes_ptr) = obj_from_bits(changes_copy_bits).as_ptr() else {
            dec_ref_bits(_py, changes_copy_bits);
            return MoltObject::none().bits();
        };
        let values_ptr = alloc_dict_with_pairs(_py, &[]);
        if values_ptr.is_null() {
            dec_ref_bits(_py, changes_copy_bits);
            return MoltObject::none().bits();
        }
        let values_bits = MoltObject::from_ptr(values_ptr).bits();
        let Some(fields_dict_bits) = dataclasses_fields_dict_bits(_py, cls_bits, missing) else {
            dec_ref_bits(_py, changes_copy_bits);
            dec_ref_bits(_py, values_bits);
            return raise_exception::<_>(
                _py,
                "TypeError",
                "replace() should be called on dataclass instances",
            );
        };
        let Some(fields_ptr) = obj_from_bits(fields_dict_bits).as_ptr() else {
            dec_ref_bits(_py, changes_copy_bits);
            dec_ref_bits(_py, values_bits);
            return MoltObject::none().bits();
        };
        let Some(field_type_name_bits) = attr_name_bits_from_bytes(_py, b"_field_type") else {
            dec_ref_bits(_py, changes_copy_bits);
            dec_ref_bits(_py, values_bits);
            return MoltObject::none().bits();
        };
        let Some(name_name_bits) = attr_name_bits_from_bytes(_py, b"name") else {
            dec_ref_bits(_py, field_type_name_bits);
            dec_ref_bits(_py, changes_copy_bits);
            dec_ref_bits(_py, values_bits);
            return MoltObject::none().bits();
        };
        let Some(init_name_bits) = attr_name_bits_from_bytes(_py, b"init") else {
            dec_ref_bits(_py, name_name_bits);
            dec_ref_bits(_py, field_type_name_bits);
            dec_ref_bits(_py, changes_copy_bits);
            dec_ref_bits(_py, values_bits);
            return MoltObject::none().bits();
        };

        let order = unsafe { dict_order(fields_ptr) }.clone();
        for pair in order.chunks(2) {
            if pair.len() != 2 {
                continue;
            }
            let field_obj_bits = pair[1];
            let ftype_bits = molt_getattr_builtin(field_obj_bits, field_type_name_bits, missing);
            if exception_pending(_py) || ftype_bits == missing {
                dec_ref_bits(_py, init_name_bits);
                dec_ref_bits(_py, name_name_bits);
                dec_ref_bits(_py, field_type_name_bits);
                dec_ref_bits(_py, changes_copy_bits);
                dec_ref_bits(_py, values_bits);
                return MoltObject::none().bits();
            }
            let name_bits = molt_getattr_builtin(field_obj_bits, name_name_bits, missing);
            if exception_pending(_py) || name_bits == missing {
                dec_ref_bits(_py, init_name_bits);
                dec_ref_bits(_py, name_name_bits);
                dec_ref_bits(_py, field_type_name_bits);
                dec_ref_bits(_py, changes_copy_bits);
                dec_ref_bits(_py, values_bits);
                return MoltObject::none().bits();
            }

            if ftype_bits == initvar_tag_bits {
                let ch_val = unsafe { dict_get_in_place(_py, changes_ptr, name_bits) };
                if let Some(bits) = ch_val {
                    unsafe {
                        dict_set_in_place(_py, values_ptr, name_bits, bits);
                        dict_del_in_place(_py, changes_ptr, name_bits);
                    }
                    if exception_pending(_py) {
                        dec_ref_bits(_py, init_name_bits);
                        dec_ref_bits(_py, name_name_bits);
                        dec_ref_bits(_py, field_type_name_bits);
                        dec_ref_bits(_py, changes_copy_bits);
                        dec_ref_bits(_py, values_bits);
                        return MoltObject::none().bits();
                    }
                } else {
                    let name_repr_bits = molt_repr_from_obj(name_bits);
                    let name_repr =
                        string_obj_to_owned(obj_from_bits(name_repr_bits)).unwrap_or_default();
                    dec_ref_bits(_py, name_repr_bits);
                    dec_ref_bits(_py, init_name_bits);
                    dec_ref_bits(_py, name_name_bits);
                    dec_ref_bits(_py, field_type_name_bits);
                    dec_ref_bits(_py, changes_copy_bits);
                    dec_ref_bits(_py, values_bits);
                    let msg = format!("InitVar {name_repr} must be specified with replace()");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                continue;
            }
            if ftype_bits != field_tag_bits {
                continue;
            }

            let init_flag_bits = molt_getattr_builtin(field_obj_bits, init_name_bits, missing);
            if exception_pending(_py) || init_flag_bits == missing {
                dec_ref_bits(_py, init_name_bits);
                dec_ref_bits(_py, name_name_bits);
                dec_ref_bits(_py, field_type_name_bits);
                dec_ref_bits(_py, changes_copy_bits);
                dec_ref_bits(_py, values_bits);
                return MoltObject::none().bits();
            }
            let init_enabled = is_truthy(_py, obj_from_bits(init_flag_bits));
            if !init_enabled {
                if unsafe { dict_get_in_place(_py, changes_ptr, name_bits) }.is_some() {
                    let field_name =
                        string_obj_to_owned(obj_from_bits(name_bits)).unwrap_or_default();
                    dec_ref_bits(_py, init_name_bits);
                    dec_ref_bits(_py, name_name_bits);
                    dec_ref_bits(_py, field_type_name_bits);
                    dec_ref_bits(_py, changes_copy_bits);
                    dec_ref_bits(_py, values_bits);
                    let msg = format!(
                        "field {field_name} is declared with init=False, it cannot be specified with replace()"
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                continue;
            }

            if let Some(changed_bits) = unsafe { dict_get_in_place(_py, changes_ptr, name_bits) } {
                unsafe {
                    dict_set_in_place(_py, values_ptr, name_bits, changed_bits);
                    dict_del_in_place(_py, changes_ptr, name_bits);
                }
                if exception_pending(_py) {
                    dec_ref_bits(_py, init_name_bits);
                    dec_ref_bits(_py, name_name_bits);
                    dec_ref_bits(_py, field_type_name_bits);
                    dec_ref_bits(_py, changes_copy_bits);
                    dec_ref_bits(_py, values_bits);
                    return MoltObject::none().bits();
                }
            } else {
                let current_bits = molt_getattr_builtin(obj_bits, name_bits, missing);
                if exception_pending(_py) || current_bits == missing {
                    dec_ref_bits(_py, init_name_bits);
                    dec_ref_bits(_py, name_name_bits);
                    dec_ref_bits(_py, field_type_name_bits);
                    dec_ref_bits(_py, changes_copy_bits);
                    dec_ref_bits(_py, values_bits);
                    return MoltObject::none().bits();
                }
                unsafe {
                    dict_set_in_place(_py, values_ptr, name_bits, current_bits);
                }
                if exception_pending(_py) {
                    dec_ref_bits(_py, init_name_bits);
                    dec_ref_bits(_py, name_name_bits);
                    dec_ref_bits(_py, field_type_name_bits);
                    dec_ref_bits(_py, changes_copy_bits);
                    dec_ref_bits(_py, values_bits);
                    return MoltObject::none().bits();
                }
            }
        }

        dec_ref_bits(_py, init_name_bits);
        dec_ref_bits(_py, name_name_bits);
        dec_ref_bits(_py, field_type_name_bits);

        unsafe {
            let _ = dict_update_apply(
                _py,
                values_bits,
                dict_update_set_in_place,
                changes_copy_bits,
            );
        }
        dec_ref_bits(_py, changes_copy_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, values_bits);
            return MoltObject::none().bits();
        }
        let out_bits = call_with_kwargs(_py, cls_bits, &[], values_bits);
        dec_ref_bits(_py, values_bits);
        out_bits
    })
}

fn resolve_bases_impl(_py: &PyToken<'_>, bases_bits: u64) -> u64 {
    let Some(bases_tuple_bits) = (unsafe { tuple_from_iter_bits(_py, bases_bits) }) else {
        return MoltObject::none().bits();
    };
    let Some(bases_tuple_ptr) = obj_from_bits(bases_tuple_bits).as_ptr() else {
        dec_ref_bits(_py, bases_tuple_bits);
        return MoltObject::none().bits();
    };
    let bases = unsafe { seq_vec_ref(bases_tuple_ptr) }.clone();
    let Some(mro_entries_name_bits) = attr_name_bits_from_bytes(_py, b"__mro_entries__") else {
        dec_ref_bits(_py, bases_tuple_bits);
        return MoltObject::none().bits();
    };
    let missing = missing_bits(_py);
    let mut out: Vec<u64> = Vec::new();
    let mut updated = false;

    for (idx, base_bits) in bases.iter().copied().enumerate() {
        let is_type = obj_from_bits(base_bits)
            .as_ptr()
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_TYPE });
        if is_type {
            if updated {
                out.push(base_bits);
            }
            continue;
        }
        let mro_entries_bits = molt_getattr_builtin(base_bits, mro_entries_name_bits, missing);
        if exception_pending(_py) {
            dec_ref_bits(_py, mro_entries_name_bits);
            dec_ref_bits(_py, bases_tuple_bits);
            return MoltObject::none().bits();
        }
        if mro_entries_bits == missing {
            if updated {
                out.push(base_bits);
            }
            continue;
        }
        if !updated {
            out.extend_from_slice(&bases[..idx]);
            updated = true;
        }
        let resolved_bits = unsafe { call_callable1(_py, mro_entries_bits, bases_bits) };
        dec_ref_bits(_py, mro_entries_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, mro_entries_name_bits);
            dec_ref_bits(_py, bases_tuple_bits);
            return MoltObject::none().bits();
        }
        let Some(resolved_ptr) = obj_from_bits(resolved_bits).as_ptr() else {
            dec_ref_bits(_py, mro_entries_name_bits);
            dec_ref_bits(_py, bases_tuple_bits);
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(resolved_ptr) != TYPE_ID_TUPLE {
                dec_ref_bits(_py, resolved_bits);
                dec_ref_bits(_py, mro_entries_name_bits);
                dec_ref_bits(_py, bases_tuple_bits);
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "__mro_entries__ must return a tuple",
                );
            }
            out.extend_from_slice(seq_vec_ref(resolved_ptr));
        }
        dec_ref_bits(_py, resolved_bits);
    }

    dec_ref_bits(_py, mro_entries_name_bits);
    dec_ref_bits(_py, bases_tuple_bits);
    if !updated {
        inc_ref_bits(_py, bases_bits);
        return bases_bits;
    }
    let out_ptr = alloc_tuple(_py, out.as_slice());
    if out_ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(out_ptr).bits()
}

fn prepare_class_impl(
    _py: &PyToken<'_>,
    name_bits: u64,
    bases_bits: u64,
    kwds_bits: u64,
) -> Option<PreparedClassState> {
    let Some(name_ptr) = obj_from_bits(name_bits).as_ptr() else {
        let _ = raise_exception::<u64>(_py, "TypeError", "class name must be str");
        return None;
    };
    unsafe {
        if object_type_id(name_ptr) != TYPE_ID_STRING {
            let _ = raise_exception::<u64>(_py, "TypeError", "class name must be str");
            return None;
        }
    }

    let kwds_copy_bits = copy_kwds_mapping(_py, kwds_bits)?;
    let Some(kwds_copy_ptr) = obj_from_bits(kwds_copy_bits).as_ptr() else {
        dec_ref_bits(_py, kwds_copy_bits);
        return None;
    };
    unsafe {
        if object_type_id(kwds_copy_ptr) != TYPE_ID_DICT {
            dec_ref_bits(_py, kwds_copy_bits);
            let _ = raise_exception::<u64>(_py, "TypeError", "kwds must be a mapping");
            return None;
        }
    }

    let Some(metaclass_name_bits) = attr_name_bits_from_bytes(_py, b"metaclass") else {
        dec_ref_bits(_py, kwds_copy_bits);
        return None;
    };
    let metaclass_bits = unsafe { dict_get_in_place(_py, kwds_copy_ptr, metaclass_name_bits) };
    if metaclass_bits.is_some() {
        unsafe {
            dict_del_in_place(_py, kwds_copy_ptr, metaclass_name_bits);
        }
        if exception_pending(_py) {
            dec_ref_bits(_py, metaclass_name_bits);
            dec_ref_bits(_py, kwds_copy_bits);
            return None;
        }
    }
    dec_ref_bits(_py, metaclass_name_bits);

    let mut winner_bits = if let Some(bits) = metaclass_bits {
        bits
    } else if is_truthy(_py, obj_from_bits(bases_bits)) {
        let index_zero = MoltObject::from_int(0).bits();
        let first_base_bits = molt_index(bases_bits, index_zero);
        if exception_pending(_py) {
            dec_ref_bits(_py, kwds_copy_bits);
            return None;
        }
        let bits = type_of_bits(_py, first_base_bits);
        dec_ref_bits(_py, first_base_bits);
        bits
    } else {
        builtin_classes(_py).type_obj
    };

    let winner_is_type = obj_from_bits(winner_bits)
        .as_ptr()
        .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_TYPE });
    if winner_is_type {
        let Some(bases_tuple_bits) = (unsafe { tuple_from_iter_bits(_py, bases_bits) }) else {
            dec_ref_bits(_py, kwds_copy_bits);
            return None;
        };
        let Some(bases_tuple_ptr) = obj_from_bits(bases_tuple_bits).as_ptr() else {
            dec_ref_bits(_py, bases_tuple_bits);
            dec_ref_bits(_py, kwds_copy_bits);
            return None;
        };
        let bases = unsafe { seq_vec_ref(bases_tuple_ptr) }.clone();
        for base_bits in bases.iter().copied() {
            let base_meta_bits = type_of_bits(_py, base_bits);
            if issubclass_bits(winner_bits, base_meta_bits) {
                continue;
            }
            if issubclass_bits(base_meta_bits, winner_bits) {
                winner_bits = base_meta_bits;
                continue;
            }
            dec_ref_bits(_py, bases_tuple_bits);
            dec_ref_bits(_py, kwds_copy_bits);
            let _ = raise_exception::<u64>(
                _py,
                "TypeError",
                "metaclass conflict: the metaclass of a derived class must be a (non-strict) subclass of the metaclasses of all its bases",
            );
            return None;
        }
        dec_ref_bits(_py, bases_tuple_bits);
    }

    let missing = missing_bits(_py);
    let prepare_bits = if winner_bits == builtin_classes(_py).type_obj {
        missing
    } else {
        let Some(prepare_name_bits) = attr_name_bits_from_bytes(_py, b"__prepare__") else {
            dec_ref_bits(_py, kwds_copy_bits);
            return None;
        };
        let mut lookup_bits = molt_getattr_builtin(winner_bits, prepare_name_bits, missing);
        dec_ref_bits(_py, prepare_name_bits);
        if exception_pending(_py) {
            if crate::builtins::attr::clear_attribute_error_if_pending(_py) {
                lookup_bits = missing;
            } else {
                dec_ref_bits(_py, kwds_copy_bits);
                return None;
            }
        }
        lookup_bits
    };
    let namespace_bits = if prepare_bits == missing {
        let ptr = alloc_dict_with_pairs(_py, &[]);
        if ptr.is_null() {
            dec_ref_bits(_py, kwds_copy_bits);
            return None;
        }
        MoltObject::from_ptr(ptr).bits()
    } else {
        let val_bits =
            call_with_kwargs(_py, prepare_bits, &[name_bits, bases_bits], kwds_copy_bits);
        dec_ref_bits(_py, prepare_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, kwds_copy_bits);
            return None;
        }
        val_bits
    };

    Some(PreparedClassState {
        metaclass_bits: winner_bits,
        namespace_bits,
        kwds_bits: kwds_copy_bits,
    })
}

#[no_mangle]
pub extern "C" fn molt_types_get_original_bases(cls_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(cls_ptr) = obj_from_bits(cls_bits).as_ptr() else {
            let owner = type_name(_py, obj_from_bits(cls_bits));
            let msg = format!("Expected an instance of type, not '{owner}'");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        unsafe {
            if object_type_id(cls_ptr) != TYPE_ID_TYPE {
                let owner = type_name(_py, obj_from_bits(cls_bits));
                let msg = format!("Expected an instance of type, not '{owner}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
        let missing = missing_bits(_py);
        let Some(orig_name_bits) = attr_name_bits_from_bytes(_py, b"__orig_bases__") else {
            return MoltObject::none().bits();
        };
        let orig_bits = molt_getattr_builtin(cls_bits, orig_name_bits, missing);
        dec_ref_bits(_py, orig_name_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if orig_bits != missing {
            return orig_bits;
        }
        let Some(bases_name_bits) = attr_name_bits_from_bytes(_py, b"__bases__") else {
            return MoltObject::none().bits();
        };
        let bases_bits = molt_getattr_builtin(cls_bits, bases_name_bits, missing);
        dec_ref_bits(_py, bases_name_bits);
        if exception_pending(_py) || bases_bits == missing {
            return MoltObject::none().bits();
        }
        bases_bits
    })
}

#[no_mangle]
pub extern "C" fn molt_types_prepare_class(args_bits: u64, kwargs_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(positional) = call_vararg_args(_py, "prepare_class", args_bits) else {
            return MoltObject::none().bits();
        };
        let Some((_, keywords)) = call_vararg_kwargs(_py, "prepare_class", kwargs_bits) else {
            return MoltObject::none().bits();
        };
        if positional.len() > 3 {
            let msg = format!(
                "prepare_class() takes at most 3 positional arguments ({} given)",
                positional.len()
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let mut name_bits = positional.first().copied().unwrap_or(0);
        let mut bases_bits = positional.get(1).copied().unwrap_or(0);
        let mut kwds_arg_bits = positional
            .get(2)
            .copied()
            .unwrap_or(MoltObject::none().bits());
        let mut has_name = !positional.is_empty();
        let mut has_bases = positional.len() >= 2;
        let mut has_kwds = positional.len() >= 3;
        for (key, val_bits) in keywords.iter() {
            match key.as_str() {
                "name" => {
                    if has_name {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "prepare_class() got multiple values for argument 'name'",
                        );
                    }
                    name_bits = *val_bits;
                    has_name = true;
                }
                "bases" => {
                    if has_bases {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "prepare_class() got multiple values for argument 'bases'",
                        );
                    }
                    bases_bits = *val_bits;
                    has_bases = true;
                }
                "kwds" => {
                    if has_kwds {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "prepare_class() got multiple values for argument 'kwds'",
                        );
                    }
                    kwds_arg_bits = *val_bits;
                    has_kwds = true;
                }
                _ => {
                    let msg = format!(
                        "prepare_class() got an unexpected keyword argument '{}'",
                        key
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            }
        }
        if !has_name {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "prepare_class() missing 1 required positional argument: 'name'",
            );
        }
        let mut owned_bases = false;
        if !has_bases {
            let ptr = alloc_tuple(_py, &[]);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            bases_bits = MoltObject::from_ptr(ptr).bits();
            owned_bases = true;
        }
        let Some(state) = prepare_class_impl(_py, name_bits, bases_bits, kwds_arg_bits) else {
            if owned_bases {
                dec_ref_bits(_py, bases_bits);
            }
            return MoltObject::none().bits();
        };
        let out_ptr = alloc_tuple(
            _py,
            &[state.metaclass_bits, state.namespace_bits, state.kwds_bits],
        );
        if owned_bases {
            dec_ref_bits(_py, bases_bits);
        }
        dec_ref_bits(_py, state.namespace_bits);
        dec_ref_bits(_py, state.kwds_bits);
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_types_resolve_bases(args_bits: u64, kwargs_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(positional) = call_vararg_args(_py, "resolve_bases", args_bits) else {
            return MoltObject::none().bits();
        };
        let Some((_, keywords)) = call_vararg_kwargs(_py, "resolve_bases", kwargs_bits) else {
            return MoltObject::none().bits();
        };
        if positional.len() > 1 {
            let msg = format!(
                "resolve_bases() takes 1 positional argument but {} were given",
                positional.len()
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let mut bases_bits = positional.first().copied().unwrap_or(0);
        let mut has_bases = !positional.is_empty();
        for (key, val_bits) in keywords.iter() {
            match key.as_str() {
                "bases" => {
                    if has_bases {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "resolve_bases() got multiple values for argument 'bases'",
                        );
                    }
                    bases_bits = *val_bits;
                    has_bases = true;
                }
                _ => {
                    let msg = format!(
                        "resolve_bases() got an unexpected keyword argument '{}'",
                        key
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            }
        }
        if !has_bases {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "resolve_bases() missing 1 required positional argument: 'bases'",
            );
        }
        resolve_bases_impl(_py, bases_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_types_new_class(args_bits: u64, kwargs_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(positional) = call_vararg_args(_py, "new_class", args_bits) else {
            return MoltObject::none().bits();
        };
        let Some((_, keywords)) = call_vararg_kwargs(_py, "new_class", kwargs_bits) else {
            return MoltObject::none().bits();
        };
        if positional.len() > 4 {
            let msg = format!(
                "new_class() takes at most 4 positional arguments ({} given)",
                positional.len()
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let mut name_bits = positional.first().copied().unwrap_or(0);
        let mut bases_bits = positional.get(1).copied().unwrap_or(0);
        let mut kwds_arg_bits = positional
            .get(2)
            .copied()
            .unwrap_or(MoltObject::none().bits());
        let mut exec_body_bits = positional
            .get(3)
            .copied()
            .unwrap_or(MoltObject::none().bits());
        let mut has_name = !positional.is_empty();
        let mut has_bases = positional.len() >= 2;
        let mut has_kwds = positional.len() >= 3;
        let mut has_exec = positional.len() >= 4;
        for (key, val_bits) in keywords.iter() {
            match key.as_str() {
                "name" => {
                    if has_name {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "new_class() got multiple values for argument 'name'",
                        );
                    }
                    name_bits = *val_bits;
                    has_name = true;
                }
                "bases" => {
                    if has_bases {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "new_class() got multiple values for argument 'bases'",
                        );
                    }
                    bases_bits = *val_bits;
                    has_bases = true;
                }
                "kwds" => {
                    if has_kwds {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "new_class() got multiple values for argument 'kwds'",
                        );
                    }
                    kwds_arg_bits = *val_bits;
                    has_kwds = true;
                }
                "exec_body" => {
                    if has_exec {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "new_class() got multiple values for argument 'exec_body'",
                        );
                    }
                    exec_body_bits = *val_bits;
                    has_exec = true;
                }
                _ => {
                    let msg = format!("new_class() got an unexpected keyword argument '{}'", key);
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            }
        }
        if !has_name {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "new_class() missing 1 required positional argument: 'name'",
            );
        }
        let mut owned_bases = false;
        if !has_bases {
            let ptr = alloc_tuple(_py, &[]);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            bases_bits = MoltObject::from_ptr(ptr).bits();
            owned_bases = true;
        }
        let resolved_bases_bits = resolve_bases_impl(_py, bases_bits);
        if obj_from_bits(resolved_bases_bits).is_none() {
            if owned_bases {
                dec_ref_bits(_py, bases_bits);
            }
            return MoltObject::none().bits();
        }
        let Some(state) = prepare_class_impl(_py, name_bits, resolved_bases_bits, kwds_arg_bits)
        else {
            dec_ref_bits(_py, resolved_bases_bits);
            if owned_bases {
                dec_ref_bits(_py, bases_bits);
            }
            return MoltObject::none().bits();
        };
        if !obj_from_bits(exec_body_bits).is_none() {
            let _ = unsafe { call_callable1(_py, exec_body_bits, state.namespace_bits) };
            if exception_pending(_py) {
                dec_ref_bits(_py, state.namespace_bits);
                dec_ref_bits(_py, state.kwds_bits);
                dec_ref_bits(_py, resolved_bases_bits);
                if owned_bases {
                    dec_ref_bits(_py, bases_bits);
                }
                return MoltObject::none().bits();
            }
        }
        if resolved_bases_bits != bases_bits {
            let Some(orig_bases_name_bits) = attr_name_bits_from_bytes(_py, b"__orig_bases__")
            else {
                dec_ref_bits(_py, state.namespace_bits);
                dec_ref_bits(_py, state.kwds_bits);
                dec_ref_bits(_py, resolved_bases_bits);
                if owned_bases {
                    dec_ref_bits(_py, bases_bits);
                }
                return MoltObject::none().bits();
            };
            let _ = molt_setitem_method(state.namespace_bits, orig_bases_name_bits, bases_bits);
            dec_ref_bits(_py, orig_bases_name_bits);
            if exception_pending(_py) {
                dec_ref_bits(_py, state.namespace_bits);
                dec_ref_bits(_py, state.kwds_bits);
                dec_ref_bits(_py, resolved_bases_bits);
                if owned_bases {
                    dec_ref_bits(_py, bases_bits);
                }
                return MoltObject::none().bits();
            }
        }
        let class_bits = call_with_kwargs(
            _py,
            state.metaclass_bits,
            &[name_bits, resolved_bases_bits, state.namespace_bits],
            state.kwds_bits,
        );
        dec_ref_bits(_py, state.namespace_bits);
        dec_ref_bits(_py, state.kwds_bits);
        dec_ref_bits(_py, resolved_bases_bits);
        if owned_bases {
            dec_ref_bits(_py, bases_bits);
        }
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        class_bits
    })
}

fn type_from_class_attr(_py: &PyToken<'_>, class_bits: u64, name: &str) -> Option<u64> {
    let class_ptr = obj_from_bits(class_bits).as_ptr()?;
    unsafe {
        if object_type_id(class_ptr) != TYPE_ID_TYPE {
            return None;
        }
    }
    let dict_bits = unsafe { class_dict_bits(class_ptr) };
    let dict_ptr = obj_from_bits(dict_bits).as_ptr()?;
    unsafe {
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            return None;
        }
    }
    let name_bits = attr_name_bits_from_bytes(_py, name.as_bytes())?;
    let val_bits = unsafe { dict_get_in_place(_py, dict_ptr, name_bits) }?;
    dec_ref_bits(_py, name_bits);
    Some(type_of_bits(_py, val_bits))
}

fn type_from_instance_attr(_py: &PyToken<'_>, class_bits: u64, name: &str) -> Option<u64> {
    let class_ptr = obj_from_bits(class_bits).as_ptr()?;
    let inst_bits = unsafe { alloc_instance_for_class(_py, class_ptr) };
    if obj_from_bits(inst_bits).is_none() {
        return None;
    }
    let missing = missing_bits(_py);
    let name_bits = attr_name_bits_from_bytes(_py, name.as_bytes())?;
    let attr_bits = molt_getattr_builtin(inst_bits, name_bits, missing);
    dec_ref_bits(_py, name_bits);
    if exception_pending(_py) || attr_bits == missing {
        if exception_pending(_py) {
            let _ = crate::builtins::attr::clear_attribute_error_if_pending(_py);
        }
        dec_ref_bits(_py, inst_bits);
        return None;
    }
    let type_bits = type_of_bits(_py, attr_bits);
    dec_ref_bits(_py, attr_bits);
    dec_ref_bits(_py, inst_bits);
    Some(type_bits)
}

fn dynamic_class_attribute_class(_py: &PyToken<'_>) -> u64 {
    let class_bits = types_class(
        _py,
        &DYNAMICCLASSATTRIBUTE_CLASS,
        "DynamicClassAttribute",
        8,
    );
    if class_bits == 0 || obj_from_bits(class_bits).is_none() {
        return class_bits;
    }
    let init_bits = builtin_func_bits(
        _py,
        &DYNAMICCLASSATTRIBUTE_INIT_FN,
        crate::molt_types_dynamic_class_attr_init as usize as u64,
        3,
    );
    let get_bits = builtin_func_bits(
        _py,
        &DYNAMICCLASSATTRIBUTE_GET_FN,
        crate::molt_types_dynamic_class_attr_get as usize as u64,
        3,
    );
    let set_bits = builtin_func_bits(
        _py,
        &DYNAMICCLASSATTRIBUTE_SET_FN,
        crate::molt_types_dynamic_class_attr_set as usize as u64,
        3,
    );
    let delete_bits = builtin_func_bits(
        _py,
        &DYNAMICCLASSATTRIBUTE_DELETE_FN,
        crate::molt_types_dynamic_class_attr_delete as usize as u64,
        2,
    );
    let getter_bits = builtin_func_bits(
        _py,
        &DYNAMICCLASSATTRIBUTE_GETTER_FN,
        crate::molt_types_dynamic_class_attr_getter as usize as u64,
        2,
    );
    let setter_bits = builtin_func_bits(
        _py,
        &DYNAMICCLASSATTRIBUTE_SETTER_FN,
        crate::molt_types_dynamic_class_attr_setter as usize as u64,
        2,
    );
    let deleter_bits = builtin_func_bits(
        _py,
        &DYNAMICCLASSATTRIBUTE_DELETER_FN,
        crate::molt_types_dynamic_class_attr_deleter as usize as u64,
        2,
    );
    set_class_method(_py, class_bits, "__init__", init_bits);
    set_class_method(_py, class_bits, "__get__", get_bits);
    set_class_method(_py, class_bits, "__set__", set_bits);
    set_class_method(_py, class_bits, "__delete__", delete_bits);
    set_class_method(_py, class_bits, "getter", getter_bits);
    set_class_method(_py, class_bits, "setter", setter_bits);
    set_class_method(_py, class_bits, "deleter", deleter_bits);
    mark_vararg_method(_py, init_bits, true);
    mark_vararg_method(_py, get_bits, true);
    class_bits
}

#[no_mangle]
pub extern "C" fn molt_types_bootstrap() -> u64 {
    crate::with_gil_entry!(_py, {
        let dict_ptr = alloc_dict_with_pairs(_py, &[]);
        if dict_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
        let builtins = builtin_classes(_py);
        let mappingproxy_bits = mappingproxy_class(_py);
        let simplenamespace_bits = simplenamespace_class(_py);
        let capsule_bits = capsule_class(_py);
        let cell_bits = cell_class(_py);
        let dynamic_class_attr_bits = dynamic_class_attribute_class(_py);

        let method_type_bits = {
            let func_ptr = alloc_function_obj(_py, crate::molt_types_coroutine as usize as u64, 1);
            if func_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let func_bits = MoltObject::from_ptr(func_ptr).bits();
            let inst_bits = unsafe {
                alloc_instance_for_class(_py, obj_from_bits(builtins.object).as_ptr().unwrap())
            };
            if obj_from_bits(inst_bits).is_none() {
                dec_ref_bits(_py, func_bits);
                return MoltObject::none().bits();
            }
            let bound_bits = crate::molt_bound_method_new(func_bits, inst_bits);
            if obj_from_bits(bound_bits).is_none() {
                dec_ref_bits(_py, func_bits);
                dec_ref_bits(_py, inst_bits);
                return MoltObject::none().bits();
            }
            let type_bits = type_of_bits(_py, bound_bits);
            dec_ref_bits(_py, bound_bits);
            dec_ref_bits(_py, func_bits);
            dec_ref_bits(_py, inst_bits);
            type_bits
        };

        let wrapper_descriptor_bits =
            type_from_class_attr(_py, builtins.object, "__init__").unwrap_or(builtins.object);
        let method_wrapper_bits =
            type_from_instance_attr(_py, builtins.object, "__str__").unwrap_or(builtins.object);
        let method_descriptor_bits =
            type_from_class_attr(_py, builtins.str, "join").unwrap_or(builtins.object);
        let classmethod_descriptor_bits =
            type_from_class_attr(_py, builtins.dict, "fromkeys").unwrap_or(builtins.object);
        let getset_descriptor_bits =
            type_from_class_attr(_py, builtins.function, "__code__").unwrap_or(builtins.object);
        let member_descriptor_bits =
            type_from_class_attr(_py, builtins.function, "__globals__").unwrap_or(builtins.object);

        let coroutine_func_ptr =
            alloc_function_obj(_py, crate::molt_types_coroutine as usize as u64, 1);
        if coroutine_func_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let coroutine_bits = MoltObject::from_ptr(coroutine_func_ptr).bits();

        let get_original_bases_ptr =
            alloc_function_obj(_py, crate::molt_types_get_original_bases as usize as u64, 1);
        if get_original_bases_ptr.is_null() {
            dec_ref_bits(_py, coroutine_bits);
            return MoltObject::none().bits();
        }
        let get_original_bases_bits = MoltObject::from_ptr(get_original_bases_ptr).bits();

        let prepare_ptr =
            alloc_function_obj(_py, crate::molt_types_prepare_class as usize as u64, 2);
        if prepare_ptr.is_null() {
            dec_ref_bits(_py, coroutine_bits);
            dec_ref_bits(_py, get_original_bases_bits);
            return MoltObject::none().bits();
        }
        let prepare_bits = MoltObject::from_ptr(prepare_ptr).bits();
        mark_vararg_method(_py, prepare_bits, false);

        let resolve_ptr =
            alloc_function_obj(_py, crate::molt_types_resolve_bases as usize as u64, 2);
        if resolve_ptr.is_null() {
            dec_ref_bits(_py, coroutine_bits);
            dec_ref_bits(_py, get_original_bases_bits);
            dec_ref_bits(_py, prepare_bits);
            return MoltObject::none().bits();
        }
        let resolve_bits = MoltObject::from_ptr(resolve_ptr).bits();
        mark_vararg_method(_py, resolve_bits, false);

        let new_ptr = alloc_function_obj(_py, crate::molt_types_new_class as usize as u64, 2);
        if new_ptr.is_null() {
            dec_ref_bits(_py, coroutine_bits);
            dec_ref_bits(_py, get_original_bases_bits);
            dec_ref_bits(_py, prepare_bits);
            dec_ref_bits(_py, resolve_bits);
            return MoltObject::none().bits();
        }
        let new_bits = MoltObject::from_ptr(new_ptr).bits();
        mark_vararg_method(_py, new_bits, false);

        let names = [
            ("AsyncGeneratorType", builtins.async_generator),
            ("BuiltinFunctionType", builtins.builtin_function_or_method),
            ("BuiltinMethodType", builtins.builtin_function_or_method),
            ("CapsuleType", capsule_bits),
            ("CellType", cell_bits),
            ("ClassMethodDescriptorType", classmethod_descriptor_bits),
            ("CodeType", builtins.code),
            ("CoroutineType", builtins.coroutine),
            ("EllipsisType", builtins.ellipsis_type),
            ("FrameType", builtins.frame),
            ("FunctionType", builtins.function),
            ("GeneratorType", builtins.generator),
            ("MappingProxyType", mappingproxy_bits),
            ("MethodType", method_type_bits),
            ("MethodDescriptorType", method_descriptor_bits),
            ("MethodWrapperType", method_wrapper_bits),
            ("ModuleType", builtins.module),
            ("NoneType", builtins.none_type),
            ("NotImplementedType", builtins.not_implemented_type),
            ("GenericAlias", builtins.generic_alias),
            ("GetSetDescriptorType", getset_descriptor_bits),
            ("LambdaType", builtins.function),
            ("MemberDescriptorType", member_descriptor_bits),
            ("SimpleNamespace", simplenamespace_bits),
            ("TracebackType", builtins.traceback),
            ("UnionType", builtins.union_type),
            ("WrapperDescriptorType", wrapper_descriptor_bits),
            ("DynamicClassAttribute", dynamic_class_attr_bits),
            ("coroutine", coroutine_bits),
            ("get_original_bases", get_original_bases_bits),
            ("new_class", new_bits),
            ("prepare_class", prepare_bits),
            ("resolve_bases", resolve_bits),
        ];
        for (name, value_bits) in names.iter() {
            let key_ptr = alloc_string(_py, name.as_bytes());
            if key_ptr.is_null() {
                continue;
            }
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            unsafe {
                dict_set_in_place(_py, dict_ptr, key_bits, *value_bits);
            }
            dec_ref_bits(_py, key_bits);
        }
        dec_ref_bits(_py, coroutine_bits);
        dec_ref_bits(_py, get_original_bases_bits);
        dec_ref_bits(_py, prepare_bits);
        dec_ref_bits(_py, resolve_bits);
        dec_ref_bits(_py, new_bits);
        dict_bits
    })
}

pub(crate) fn types_drop_instance(_py: &PyToken<'_>, ptr: *mut u8) -> bool {
    let class_bits = unsafe { object_class_bits(ptr) };
    if class_bits == 0 {
        return false;
    }
    let mappingproxy = MAPPINGPROXY_CLASS.load(Ordering::Acquire);
    if class_bits == mappingproxy {
        let mapping_bits = unsafe { mappingproxy_mapping_bits(ptr) };
        if mapping_bits != 0 && !obj_from_bits(mapping_bits).is_none() {
            dec_ref_bits(_py, mapping_bits);
        }
        return true;
    }
    false
}
