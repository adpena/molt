use std::collections::HashSet;

use molt_obj_model::MoltObject;

use crate::builtins::attr::attr_lookup_ptr_allow_missing;
use crate::{
    alloc_class_obj, alloc_classmethod_obj, alloc_generic_alias, alloc_property_obj,
    alloc_staticmethod_obj, alloc_super_obj, alloc_tuple, builtin_classes, builtin_type_bits,
    call_callable2, class_bases_bits, class_bases_vec, class_bump_layout_version, class_dict_bits,
    class_layout_version_bits, class_mro_bits, class_mro_vec, class_name_for_error,
    class_set_bases_bits, class_set_layout_version_bits, class_set_mro_bits, dec_ref_bits,
    dict_order, dict_set_in_place, header_from_obj_ptr, inc_ref_bits, intern_static_name,
    isinstance_bits, issubclass_bits, maybe_ptr_from_bits, molt_alloc, obj_from_bits,
    object_class_bits, object_set_class_bits, object_type_id, raise_exception, runtime_state,
    seq_vec_ref, to_i64, type_of_bits, PyToken, HEADER_FLAG_SKIP_CLASS_DECREF, TYPE_ID_DICT,
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
pub extern "C" fn molt_isinstance(val_bits: u64, class_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        MoltObject::from_bool(isinstance_bits(_py, val_bits, class_bits)).bits()
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
            if issubclass_bits(sub_bits, class_bits) {
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
        let obj_type_bits = if let Some(obj_ptr) = obj.as_ptr() {
            unsafe {
                if object_type_id(obj_ptr) == TYPE_ID_TYPE {
                    obj_bits
                } else {
                    type_of_bits(_py, obj_bits)
                }
            }
        } else {
            type_of_bits(_py, obj_bits)
        };
        if !issubclass_bits(obj_type_bits, type_bits) {
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
            _ => {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "issubclass() arg 2 must be a class or tuple of classes",
                )
            }
        }
    }
}
