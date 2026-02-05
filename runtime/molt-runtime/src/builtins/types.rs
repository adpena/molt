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
    class_set_mro_bits, class_set_qualname_bits, dataclass_set_class_raw, dec_ref_bits,
    dict_del_in_place, dict_get_in_place, dict_order, dict_set_in_place, dict_update_apply,
    dict_update_set_in_place, exception_pending, header_from_obj_ptr, inc_ref_bits,
    init_atomic_bits, intern_static_name, instance_dict_bits, is_builtin_class_bits, is_truthy,
    isinstance_runtime, issubclass_bits, issubclass_runtime, maybe_ptr_from_bits, missing_bits,
    molt_alloc, molt_call_bind, molt_callargs_new, molt_callargs_push_kw, molt_callargs_push_pos,
    molt_contains, molt_eq, molt_getattr_builtin, molt_index, molt_iter,
    molt_iter_next, molt_len, molt_object_setattr, molt_repr_from_obj, obj_from_bits,
    object_class_bits, object_set_class_bits, object_type_id, property_del_bits,
    property_get_bits, property_set_bits, raise_exception, raise_not_iterable, runtime_state,
    seq_vec_ref, string_obj_to_owned, to_i64, tuple_from_iter_bits, type_name, type_of_bits,
    PyToken, HEADER_FLAG_SKIP_CLASS_DECREF, TYPE_ID_DATACLASS, TYPE_ID_DICT, TYPE_ID_PROPERTY,
    TYPE_ID_STRING, TYPE_ID_TUPLE, TYPE_ID_TYPE,
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
                    let _ = unsafe { call_callable2(_py, set_name, class_bits, name_bits) };
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
        dict_set_in_place(_py, dict_ptr, vararg_name, MoltObject::from_bool(true).bits());
        dict_set_in_place(_py, dict_ptr, varkw_name, MoltObject::from_bool(true).bits());
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
        crate::molt_types_mappingproxy_new as u64,
        2,
    );
    let init_bits = builtin_func_bits(
        _py,
        &MAPPINGPROXY_INIT_FN,
        crate::molt_types_mappingproxy_init as u64,
        2,
    );
    let getitem_bits = builtin_func_bits(
        _py,
        &MAPPINGPROXY_GETITEM_FN,
        crate::molt_types_mappingproxy_getitem as u64,
        2,
    );
    let iter_bits = builtin_func_bits(
        _py,
        &MAPPINGPROXY_ITER_FN,
        crate::molt_types_mappingproxy_iter as u64,
        1,
    );
    let len_bits = builtin_func_bits(
        _py,
        &MAPPINGPROXY_LEN_FN,
        crate::molt_types_mappingproxy_len as u64,
        1,
    );
    let contains_bits = builtin_func_bits(
        _py,
        &MAPPINGPROXY_CONTAINS_FN,
        crate::molt_types_mappingproxy_contains as u64,
        2,
    );
    let get_bits = builtin_func_bits(
        _py,
        &MAPPINGPROXY_GET_FN,
        crate::molt_types_mappingproxy_get as u64,
        3,
    );
    let keys_bits = builtin_func_bits(
        _py,
        &MAPPINGPROXY_KEYS_FN,
        crate::molt_types_mappingproxy_keys as u64,
        1,
    );
    let items_bits = builtin_func_bits(
        _py,
        &MAPPINGPROXY_ITEMS_FN,
        crate::molt_types_mappingproxy_items as u64,
        1,
    );
    let values_bits = builtin_func_bits(
        _py,
        &MAPPINGPROXY_VALUES_FN,
        crate::molt_types_mappingproxy_values as u64,
        1,
    );
    let repr_bits = builtin_func_bits(
        _py,
        &MAPPINGPROXY_REPR_FN,
        crate::molt_types_mappingproxy_repr as u64,
        1,
    );
    let setitem_bits = builtin_func_bits(
        _py,
        &MAPPINGPROXY_SETITEM_FN,
        crate::molt_types_mappingproxy_setitem as u64,
        3,
    );
    let delitem_bits = builtin_func_bits(
        _py,
        &MAPPINGPROXY_DELITEM_FN,
        crate::molt_types_mappingproxy_delitem as u64,
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
        crate::molt_types_method_new as u64,
        3,
    );
    let init_bits = builtin_func_bits(
        _py,
        &METHOD_INIT_FN,
        crate::molt_types_method_init as u64,
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
        crate::molt_types_simplenamespace_init as u64,
        3,
    );
    let repr_bits = builtin_func_bits(
        _py,
        &SIMPLENAMESPACE_REPR_FN,
        crate::molt_types_simplenamespace_repr as u64,
        1,
    );
    let eq_bits = builtin_func_bits(
        _py,
        &SIMPLENAMESPACE_EQ_FN,
        crate::molt_types_simplenamespace_eq as u64,
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
        crate::molt_types_capsule_new as u64,
        1,
    );
    set_class_method(_py, class_bits, "__new__", new_bits);
    class_bits
}

fn cell_class(_py: &PyToken<'_>) -> u64 {
    // TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): replace
    // placeholder cell type once closure cell objects are implemented.
    let class_bits = types_class(_py, &CELL_CLASS, "cell", 8);
    let new_bits = builtin_func_bits(
        _py,
        &CELL_NEW_FN,
        crate::molt_types_cell_new as u64,
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
            return raise_exception::<_>(_py, "TypeError", "mappingproxy() argument cannot be None");
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
pub extern "C" fn molt_types_mappingproxy_get(self_bits: u64, args_bits: u64, kwargs_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let args_ptr = obj_from_bits(args_bits).as_ptr();
        let Some(args_ptr) = args_ptr else {
            return raise_exception::<_>(_py, "TypeError", "mappingproxy.get() expects arguments");
        };
        unsafe {
            if object_type_id(args_ptr) != TYPE_ID_TUPLE {
                return raise_exception::<_>(_py, "TypeError", "mappingproxy.get() expects arguments");
            }
        }
        let args = unsafe { seq_vec_ref(args_ptr) };
        if args.len() == 0 || args.len() > 2 {
            return raise_exception::<_>(_py, "TypeError", "mappingproxy.get() takes 1 or 2 arguments");
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
        let missing = missing_bits(_py);
        let name_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.get_name,
            b"get",
        );
        let method_bits = molt_getattr_builtin(mapping_bits, name_bits, missing);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if method_bits == missing {
            return raise_exception::<_>(_py, "AttributeError", "get");
        }
        let res_bits = if args.len() == 2 {
            unsafe { call_callable2(_py, method_bits, key_bits, default_bits) }
        } else {
            unsafe { call_callable1(_py, method_bits, key_bits) }
        };
        dec_ref_bits(_py, method_bits);
        res_bits
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
        let mapping_repr = string_obj_to_owned(obj_from_bits(mapping_repr_bits)).unwrap_or_default();
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
        raise_exception::<_>(_py, "TypeError", "'mappingproxy' object does not support item assignment")
    })
}

#[no_mangle]
pub extern "C" fn molt_types_mappingproxy_delitem(_self_bits: u64, _key_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_exception::<_>(_py, "TypeError", "'mappingproxy' object does not support item deletion")
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
                    return raise_exception::<_>(_py, "TypeError", "SimpleNamespace expects arguments");
                }
                seq_vec_ref(args_ptr).clone()
            }
        } else {
            Vec::new()
        };
        if args.len() > 1 {
            let msg = format!("SimpleNamespace expected at most 1 argument, got {}", args.len());
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let dict_ptr = alloc_dict_with_pairs(_py, &[]);
        if dict_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
        if args.len() == 1 {
            unsafe {
                let _ = dict_update_apply(_py, dict_bits, dict_update_set_in_place, args[0]);
            }
            if exception_pending(_py) {
                dec_ref_bits(_py, dict_bits);
                return MoltObject::none().bits();
            }
        }
        if let Some(kwargs_ptr) = obj_from_bits(kwargs_bits).as_ptr() {
            unsafe {
                if object_type_id(kwargs_ptr) == TYPE_ID_DICT {
                    let _ = dict_update_apply(
                        _py,
                        dict_bits,
                        dict_update_set_in_place,
                        kwargs_bits,
                    );
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
                            let val_repr =
                                string_obj_to_owned(obj_from_bits(val_repr_bits)).unwrap_or_default();
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

#[no_mangle]
// TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:missing): implement
// types.get_original_bases with full CPython semantics.
pub extern "C" fn molt_types_get_original_bases(_cls_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_exception::<_>(_py, "NotImplementedError", "types.get_original_bases is not implemented")
    })
}

#[no_mangle]
// TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:missing): implement
// types.prepare_class with full CPython semantics.
pub extern "C" fn molt_types_prepare_class(_args_bits: u64, _kwargs_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_exception::<_>(_py, "NotImplementedError", "types.prepare_class is not implemented")
    })
}

#[no_mangle]
// TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:missing): implement
// types.resolve_bases with full CPython semantics.
pub extern "C" fn molt_types_resolve_bases(_args_bits: u64, _kwargs_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_exception::<_>(_py, "NotImplementedError", "types.resolve_bases is not implemented")
    })
}

#[no_mangle]
// TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:missing): implement
// types.new_class with full CPython semantics.
pub extern "C" fn molt_types_new_class(_args_bits: u64, _kwargs_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_exception::<_>(_py, "NotImplementedError", "types.new_class is not implemented")
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

fn property_type_bits(_py: &PyToken<'_>) -> Option<u64> {
    let none = MoltObject::none().bits();
    let ptr = alloc_property_obj(_py, none, none, none);
    if ptr.is_null() {
        return None;
    }
    let prop_bits = MoltObject::from_ptr(ptr).bits();
    let type_bits = type_of_bits(_py, prop_bits);
    dec_ref_bits(_py, prop_bits);
    Some(type_bits)
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
        // TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): replace
        // DynamicClassAttribute placeholder with a dedicated descriptor type.
        let prop_type_bits = property_type_bits(_py).unwrap_or(builtins.object);

        let method_type_bits = {
            let func_ptr = alloc_function_obj(_py, crate::molt_types_coroutine as u64, 1);
            if func_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let func_bits = MoltObject::from_ptr(func_ptr).bits();
            let inst_bits = unsafe { alloc_instance_for_class(_py, obj_from_bits(builtins.object).as_ptr().unwrap()) };
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

        let coroutine_func_ptr = alloc_function_obj(_py, crate::molt_types_coroutine as u64, 1);
        if coroutine_func_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let coroutine_bits = MoltObject::from_ptr(coroutine_func_ptr).bits();

        let get_original_bases_ptr =
            alloc_function_obj(_py, crate::molt_types_get_original_bases as u64, 1);
        if get_original_bases_ptr.is_null() {
            dec_ref_bits(_py, coroutine_bits);
            return MoltObject::none().bits();
        }
        let get_original_bases_bits = MoltObject::from_ptr(get_original_bases_ptr).bits();

        let prepare_ptr = alloc_function_obj(_py, crate::molt_types_prepare_class as u64, 2);
        if prepare_ptr.is_null() {
            dec_ref_bits(_py, coroutine_bits);
            dec_ref_bits(_py, get_original_bases_bits);
            return MoltObject::none().bits();
        }
        let prepare_bits = MoltObject::from_ptr(prepare_ptr).bits();
        mark_vararg_method(_py, prepare_bits, false);

        let resolve_ptr = alloc_function_obj(_py, crate::molt_types_resolve_bases as u64, 2);
        if resolve_ptr.is_null() {
            dec_ref_bits(_py, coroutine_bits);
            dec_ref_bits(_py, get_original_bases_bits);
            dec_ref_bits(_py, prepare_bits);
            return MoltObject::none().bits();
        }
        let resolve_bits = MoltObject::from_ptr(resolve_ptr).bits();
        mark_vararg_method(_py, resolve_bits, false);

        let new_ptr = alloc_function_obj(_py, crate::molt_types_new_class as u64, 2);
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
            ("DynamicClassAttribute", prop_type_bits),
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
