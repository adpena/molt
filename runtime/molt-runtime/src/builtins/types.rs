use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use molt_obj_model::MoltObject;

pub(crate) mod dataclasses;

/// Cached `MOLT_TRACE_BUILTIN_TYPE` flag. `molt_builtin_type` resolves builtin
/// type objects (`int`, `str`, ...) and is on a very hot dispatch path; read
/// the env var once rather than per call (per-call `std::env::var` takes the
/// libc environ lock and heap-allocates).
#[inline]
fn trace_builtin_type_enabled() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| std::env::var("MOLT_TRACE_BUILTIN_TYPE").as_deref() == Ok("1"))
}

/// Cached `MOLT_TRACE_ISINSTANCE` flag. `molt_isinstance` runs on every
/// `isinstance()` call; read the env var once rather than per call.
#[inline]
fn trace_isinstance_enabled() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| std::env::var("MOLT_TRACE_ISINSTANCE").as_deref() == Ok("1"))
}

use crate::state::{RuntimeState, cache::clear_atomic_slots};
use crate::{
    ClassInfoProtocol, HEADER_FLAG_SKIP_CLASS_DECREF, PyToken, RuntimeClassInfo, TYPE_ID_BYTES,
    TYPE_ID_COMPLEX, TYPE_ID_DATACLASS, TYPE_ID_DICT, TYPE_ID_ELLIPSIS, TYPE_ID_GENERIC_ALIAS,
    TYPE_ID_LIST, TYPE_ID_NOT_IMPLEMENTED, TYPE_ID_PROPERTY, TYPE_ID_RANGE, TYPE_ID_STRING,
    TYPE_ID_TUPLE, TYPE_ID_TYPE, alloc_class_obj, alloc_classmethod_obj, alloc_dict_with_pairs,
    alloc_generic_alias, alloc_instance_for_class, alloc_list, alloc_property_obj,
    alloc_staticmethod_obj, alloc_string, alloc_super_obj, alloc_tuple, apply_class_slots_layout,
    attr_lookup_ptr_allow_missing, attr_name_bits_from_bytes, builtin_classes, builtin_type_bits,
    call_callable0, call_callable1, call_callable2, class_bases_bits, class_bases_vec,
    class_bump_layout_version, class_dict_bits, class_layout_version_bits, class_mro_bits,
    class_mro_vec, class_name_for_error, class_set_bases_bits, class_set_layout_version_bits,
    class_set_mro_bits, class_set_qualname_bits, clear_exception, collect_runtime_classinfo,
    dataclass_set_class_raw, dec_ref_bits, dict_del_in_place, dict_get_in_place, dict_order,
    dict_set_in_place, dict_update_apply, dict_update_set_in_place, exception_pending,
    function_dict_bits, generic_alias_origin_bits, header_from_obj_ptr, inc_ref_bits,
    init_atomic_bits, instance_dict_bits, intern_static_name, is_builtin_class_bits, is_truthy,
    isinstance_runtime, issubclass_bits, issubclass_runtime, maybe_ptr_from_bits, missing_bits,
    molt_alloc, molt_call_bind, molt_callargs_new, molt_callargs_push_kw, molt_callargs_push_pos,
    molt_contains, molt_dict_from_obj, molt_dict_get, molt_eq, molt_getattr_builtin,
    molt_hash_builtin, molt_index, molt_iter, molt_iter_next, molt_len, molt_object_setattr,
    molt_repr_from_obj, molt_setitem_method, molt_str_from_obj, molt_string_isidentifier, obj_eq,
    obj_from_bits, object_class_bits, object_set_class_bits, object_type_id, property_del_bits,
    property_get_bits, property_set_bits, raise_exception, raise_not_iterable,
    runtime_classinfo_protocol_match, runtime_state, seq_vec_ref, string_obj_to_owned, to_i64,
    tuple_from_iter_bits, type_name, type_of_bits,
};

#[unsafe(no_mangle)]
pub extern "C" fn molt_is_string_obj(val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(val_bits);
        let is_string = obj
            .as_ptr()
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_STRING });
        MoltObject::from_bool(is_string).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_class_new(name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
        // `class` statements lowered via `molt_class_new` are only used on the
        // static fast-path where the metaclass is known to be `type`. Ensure the
        // new class object is an instance of `type` (CPython parity).
        unsafe {
            let builtins = builtin_classes(_py);
            let old_bits = object_class_bits(ptr);
            if old_bits != builtins.type_obj {
                if old_bits != 0 {
                    dec_ref_bits(_py, old_bits);
                }
                object_set_class_bits(_py, ptr, builtins.type_obj);
                inc_ref_bits(_py, builtins.type_obj);
            }
        }
        // Set __doc__ = None on the class dict (CPython parity).
        // Every class has a __doc__ attribute; without this, `cls.__doc__`
        // raises AttributeError which breaks libraries like six.
        unsafe {
            let dict_bits = class_dict_bits(ptr);
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                let doc_key = alloc_string(_py, b"__doc__");
                if !doc_key.is_null() {
                    dict_set_in_place(
                        _py,
                        dict_ptr,
                        MoltObject::from_ptr(doc_key).bits(),
                        MoltObject::none().bits(),
                    );
                    dec_ref_bits(_py, MoltObject::from_ptr(doc_key).bits());
                }
            }
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_builtin_type(tag_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let tag = match to_i64(obj_from_bits(tag_bits)) {
            Some(val) => val,
            None => return raise_exception::<_>(_py, "TypeError", "builtin type tag must be int"),
        };
        let Some(bits) = builtin_type_bits(_py, tag) else {
            return raise_exception::<_>(_py, "TypeError", "unknown builtin type tag");
        };
        if trace_builtin_type_enabled() {
            eprintln!("molt builtin_type tag={} bits=0x{:x}", tag, bits);
        }
        inc_ref_bits(_py, bits);
        bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_type_of(val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let bits = type_of_bits(_py, val_bits);
        inc_ref_bits(_py, bits);
        bits
    })
}

/// Returns the type of an object WITHOUT incrementing the refcount.
/// The type is guaranteed alive because the object holds a strong reference
/// to its type internally. This is the borrowed-reference equivalent of
/// `molt_type_of` and mirrors CPython's `Py_TYPE()` semantics.
#[unsafe(no_mangle)]
pub extern "C" fn molt_type_of_borrowed(val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { type_of_bits(_py, val_bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_type_new(
    cls_bits: u64,
    name_bits: u64,
    bases_bits: u64,
    namespace_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
                        );
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
        let mut qualname_owned = false;
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
            && unsafe { object_type_id(dict_ptr) } == TYPE_ID_DICT
        {
            let qualname_name_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.qualname_name,
                b"__qualname__",
            );
            if let Some(val_bits) = unsafe { dict_get_in_place(_py, dict_ptr, qualname_name_bits) }
            {
                qualname_bits = val_bits;
                // We're about to delete __qualname__ from the class dict; hold a strong
                // reference so we can safely move it into the class qualname slot.
                inc_ref_bits(_py, qualname_bits);
                qualname_owned = true;
                unsafe {
                    dict_del_in_place(_py, dict_ptr, qualname_name_bits);
                }
                if exception_pending(_py) {
                    if qualname_owned {
                        dec_ref_bits(_py, qualname_bits);
                    }
                    if bases_owned {
                        dec_ref_bits(_py, bases_tuple_bits);
                    }
                    return MoltObject::none().bits();
                }
            }
            if let Some(classdictcell_bits) = attr_name_bits_from_bytes(_py, b"__classdictcell__") {
                unsafe {
                    dict_del_in_place(_py, dict_ptr, classdictcell_bits);
                }
                dec_ref_bits(_py, classdictcell_bits);
                if exception_pending(_py) {
                    if qualname_owned {
                        dec_ref_bits(_py, qualname_bits);
                    }
                    if bases_owned {
                        dec_ref_bits(_py, bases_tuple_bits);
                    }
                    return MoltObject::none().bits();
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
            if qualname_owned {
                dec_ref_bits(_py, qualname_bits);
            }
            if bases_owned {
                dec_ref_bits(_py, bases_tuple_bits);
            }
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        unsafe {
            class_set_qualname_bits(_py, class_ptr, qualname_bits);
        }
        if qualname_owned {
            // Balance the strong ref we took before deleting __qualname__ from the dict.
            dec_ref_bits(_py, qualname_bits);
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
        unsafe {
            crate::object::class_finish_definition(_py, class_ptr);
        }

        let mut kw_pairs: Vec<(u64, u64)> = Vec::new();
        let kwargs_obj = obj_from_bits(kwargs_bits);
        if !kwargs_obj.is_none()
            && let Some(kwargs_ptr) = kwargs_obj.as_ptr()
        {
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
        class_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_type_init(
    _cls_bits: u64,
    _name_bits: u64,
    _bases_bits: u64,
    _namespace_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let _ = kwargs_bits;
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_type_prepare(_cls_bits: u64, _name_bits: u64, _bases_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let dict_ptr = alloc_dict_with_pairs(_py, &[]);
        if dict_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(dict_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_type_mro(cls_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_type_instancecheck(cls_bits: u64, inst_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let inst_type = type_of_bits(_py, inst_bits);
        MoltObject::from_bool(issubclass_bits(inst_type, cls_bits)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_type_subclasscheck(cls_bits: u64, sub_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        MoltObject::from_bool(issubclass_bits(sub_bits, cls_bits)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_isinstance(val_bits: u64, class_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let result = isinstance_runtime(_py, val_bits, class_bits);
        if trace_isinstance_enabled() {
            eprintln!(
                "molt isinstance val_type={} class_type={} result={}",
                crate::type_name(_py, obj_from_bits(val_bits)),
                crate::type_name(_py, obj_from_bits(class_bits)),
                result
            );
        }
        MoltObject::from_bool(result).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_issubclass(sub_bits: u64, class_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
        collect_runtime_classinfo(_py, class_bits, ClassInfoProtocol::Subclass, &mut classes);
        for class_info in classes {
            match class_info {
                RuntimeClassInfo::Type(class_bits) => {
                    if issubclass_runtime(_py, sub_bits, class_bits) {
                        return MoltObject::from_bool(true).bits();
                    }
                }
                RuntimeClassInfo::Protocol(class_bits) => {
                    match runtime_classinfo_protocol_match(
                        _py,
                        class_bits,
                        sub_bits,
                        ClassInfoProtocol::Subclass,
                    ) {
                        Some(true) => return MoltObject::from_bool(true).bits(),
                        Some(false) => {}
                        None => break,
                    }
                }
            }
        }
        MoltObject::from_bool(false).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_new() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj_bits = molt_alloc(std::mem::size_of::<u64>() as u64);
        let Some(obj_ptr) = obj_from_bits(obj_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        let class_bits = builtin_classes(_py).object;
        unsafe {
            let _ = molt_object_set_class(obj_ptr as usize as u64, class_bits);
        }
        obj_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_new_bound(cls_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

/// Sized variant of [`molt_object_new_bound`] — the codegen passes
/// the static instance payload size (in bytes, header-exclusive)
/// when the frontend carries it on the `OBJECT_NEW_BOUND` op's
/// `value` field (set from `class_info["size"]`).  The runtime
/// then skips the `class_layout_size` MRO walk + dict probe + name
/// interning entirely.
///
/// All other guards (cls_bits validity, type_id check, builtin
/// safety check) match the unsized entry point exactly.
#[unsafe(no_mangle)]
pub extern "C" fn molt_object_new_bound_sized(cls_bits: u64, payload_size_bytes: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
        unsafe {
            crate::call::class_init::alloc_instance_for_class_sized(
                _py,
                cls_ptr,
                payload_size_bytes as usize,
            )
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tuple_new_bound(cls_bits: u64, iterable_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

fn c3_merge(seqs: Vec<Vec<u64>>) -> Option<Vec<u64>> {
    let mut result = Vec::new();
    let mut heads = vec![0usize; seqs.len()];
    let mut tail_counts: HashMap<u64, usize> = HashMap::new();
    for seq in &seqs {
        for &value in seq.iter().skip(1) {
            *tail_counts.entry(value).or_insert(0) += 1;
        }
    }
    loop {
        let mut remaining = 0usize;
        for (idx, seq) in seqs.iter().enumerate() {
            if heads[idx] < seq.len() {
                remaining += 1;
            }
        }
        if remaining == 0 {
            return Some(result);
        }
        let mut candidate = None;
        'outer: for (seq_idx, seq) in seqs.iter().enumerate() {
            let head_idx = heads[seq_idx];
            if head_idx >= seq.len() {
                continue;
            }
            let head = seq[head_idx];
            if tail_counts.get(&head).copied().unwrap_or(0) == 0 {
                candidate = Some(head);
                break 'outer;
            }
        }
        let cand = candidate?;
        result.push(cand);
        for (idx, seq) in seqs.iter().enumerate() {
            let head_idx = heads[idx];
            if head_idx < seq.len() && seq[head_idx] == cand {
                heads[idx] += 1;
                let next_head_idx = heads[idx];
                if next_head_idx < seq.len() {
                    let next_head = seq[next_head_idx];
                    if let Some(count) = tail_counts.get_mut(&next_head) {
                        if *count <= 1 {
                            tail_counts.remove(&next_head);
                        } else {
                            *count -= 1;
                        }
                    }
                }
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_class_set_base(class_bits: u64, base_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let class_obj = obj_from_bits(class_bits);
        let Some(class_ptr) = class_obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(class_ptr) != TYPE_ID_TYPE {
                return MoltObject::none().bits();
            }
        }
        let mut bases_vec = Vec::new();
        let bases_owned;
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
                        let tuple_ptr = alloc_tuple(_py, &bases_vec);
                        if tuple_ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        bases_owned = true;
                        MoltObject::from_ptr(tuple_ptr).bits()
                    }
                    _ => {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "base must be a type object or tuple of types",
                        );
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
            let mut bases_updated = false;
            let mut mro_updated = false;
            if old_bases != bases_bits {
                dec_ref_bits(_py, old_bases);
                if !bases_owned {
                    inc_ref_bits(_py, bases_bits);
                }
                class_set_bases_bits(class_ptr, bases_bits);
                bases_updated = true;
            }
            if old_mro != mro_bits {
                dec_ref_bits(_py, old_mro);
                class_set_mro_bits(class_ptr, mro_bits);
                mro_updated = true;
            }
            let dict_bits = class_dict_bits(class_ptr);
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                && object_type_id(dict_ptr) == TYPE_ID_DICT
            {
                let bases_name =
                    intern_static_name(_py, &runtime_state(_py).interned.bases_name, b"__bases__");
                let mro_name =
                    intern_static_name(_py, &runtime_state(_py).interned.mro_name, b"__mro__");
                dict_set_in_place(_py, dict_ptr, bases_name, bases_bits);
                dict_set_in_place(_py, dict_ptr, mro_name, mro_bits);
            }
            if bases_owned && !bases_updated {
                dec_ref_bits(_py, bases_bits);
            }
            if !mro_updated {
                dec_ref_bits(_py, mro_bits);
            }
            if bases_updated || mro_updated {
                crate::object::class_refresh_finalizer_flag(_py, class_ptr);
                class_bump_layout_version(class_ptr);
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_class_apply_set_name(class_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let trace_set_name = matches!(
            std::env::var("MOLT_TRACE_SET_NAME").ok().as_deref(),
            Some("1")
        );
        let class_obj = obj_from_bits(class_bits);
        let Some(class_ptr) = class_obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(class_ptr) != TYPE_ID_TYPE {
                return MoltObject::none().bits();
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
                // `entries` is a borrowed snapshot of the class dict.  A user
                // `__set_name__` hook can mutate that dict, including deleting
                // the descriptor currently being initialized, so the apply loop
                // must own the key/value pair across arbitrary hook execution.
                inc_ref_bits(_py, name_bits);
                inc_ref_bits(_py, val_bits);
                let Some(val_ptr) = maybe_ptr_from_bits(val_bits) else {
                    dec_ref_bits(_py, val_bits);
                    dec_ref_bits(_py, name_bits);
                    continue;
                };
                if let Some(set_name) = attr_lookup_ptr_allow_missing(_py, val_ptr, set_name_bits) {
                    if trace_set_name {
                        let class_name = class_name_for_error(class_bits);
                        let key = string_obj_to_owned(obj_from_bits(name_bits))
                            .unwrap_or_else(|| "<non-str>".to_string());
                        let val_type_id = object_type_id(val_ptr);
                        let (set_name_type_id, set_name_type) =
                            if let Some(ptr) = obj_from_bits(set_name).as_ptr() {
                                (object_type_id(ptr), type_name(_py, obj_from_bits(set_name)))
                            } else {
                                (0, type_name(_py, obj_from_bits(set_name)))
                            };
                        eprintln!(
                            "molt set_name: class={} key={} val_type_id={} set_name_type_id={} set_name_type={}",
                            class_name, key, val_type_id, set_name_type_id, set_name_type,
                        );
                    }
                    let _ = call_callable2(_py, set_name, class_bits, name_bits);
                    dec_ref_bits(_py, set_name);
                }
                dec_ref_bits(_py, val_bits);
                dec_ref_bits(_py, name_bits);
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_class_layout_version(class_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let class_obj = obj_from_bits(class_bits);
        let Some(class_ptr) = class_obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(class_ptr) != TYPE_ID_TYPE {
                return MoltObject::none().bits();
            }
            MoltObject::from_int(class_layout_version_bits(class_ptr) as i64).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_class_set_layout_version(class_bits: u64, version_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let class_obj = obj_from_bits(class_bits);
        let Some(class_ptr) = class_obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(class_ptr) != TYPE_ID_TYPE {
                return MoltObject::none().bits();
            }
            let version = match to_i64(obj_from_bits(version_bits)) {
                Some(val) if val >= 0 => val as u64,
                _ => return raise_exception::<_>(_py, "TypeError", "layout version must be int"),
            };
            class_set_layout_version_bits(class_ptr, version);
            crate::bump_type_version();
        }
        MoltObject::none().bits()
    })
}

unsafe fn max_slot_end_from_offsets_dict(offsets_ptr: *mut u8) -> usize {
    unsafe {
        if object_type_id(offsets_ptr) != TYPE_ID_DICT {
            return 0;
        }
        let mut max_end = 0usize;
        let entries = dict_order(offsets_ptr).clone();
        for pair in entries.chunks(2) {
            if pair.len() != 2 {
                continue;
            }
            if let Some(offset) = obj_from_bits(pair[1]).as_int()
                && offset >= 0
            {
                let end = (offset as usize).saturating_add(std::mem::size_of::<u64>());
                if end > max_end {
                    max_end = end;
                }
            }
        }
        max_end
    }
}

unsafe fn merge_class_layout_metadata(
    _py: &PyToken<'_>,
    class_ptr: *mut u8,
    offsets_bits: u64,
    size_bits: u64,
) -> Result<(), u64> {
    unsafe {
        let class_bits = MoltObject::from_ptr(class_ptr).bits();
        let dict_bits = class_dict_bits(class_ptr);
        let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
            return Ok(());
        };
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            return Ok(());
        }

        let offsets_name_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.field_offsets_name,
            b"__molt_field_offsets__",
        );
        let layout_name_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.molt_layout_size,
            b"__molt_layout_size__",
        );

        let mut merged_offsets_ptr: *mut u8 = std::ptr::null_mut();
        if !obj_from_bits(offsets_bits).is_none() {
            let Some(source_offsets_ptr) = obj_from_bits(offsets_bits).as_ptr() else {
                return Err(raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "__molt_field_offsets__ must be dict or None",
                ));
            };
            if object_type_id(source_offsets_ptr) != TYPE_ID_DICT {
                return Err(raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "__molt_field_offsets__ must be dict or None",
                ));
            }
            let mut target_offsets_bits =
                dict_get_in_place(_py, dict_ptr, offsets_name_bits).unwrap_or(0);
            if obj_from_bits(target_offsets_bits).is_none() || target_offsets_bits == 0 {
                let new_ptr = alloc_dict_with_pairs(_py, &[]);
                if new_ptr.is_null() {
                    return Err(MoltObject::none().bits());
                }
                target_offsets_bits = MoltObject::from_ptr(new_ptr).bits();
                dict_set_in_place(_py, dict_ptr, offsets_name_bits, target_offsets_bits);
            }
            let Some(target_offsets_ptr) = obj_from_bits(target_offsets_bits).as_ptr() else {
                return Err(raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "__molt_field_offsets__ must be dict",
                ));
            };
            if object_type_id(target_offsets_ptr) != TYPE_ID_DICT {
                return Err(raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "__molt_field_offsets__ must be dict",
                ));
            }
            let entries = dict_order(source_offsets_ptr).clone();
            for pair in entries.chunks(2) {
                if pair.len() != 2 {
                    continue;
                }
                if dict_get_in_place(_py, target_offsets_ptr, pair[0]).is_some() {
                    continue;
                }
                dict_set_in_place(_py, target_offsets_ptr, pair[0], pair[1]);
            }
            merged_offsets_ptr = target_offsets_ptr;
        } else if let Some(existing_offsets_bits) =
            dict_get_in_place(_py, dict_ptr, offsets_name_bits)
            && let Some(existing_offsets_ptr) = obj_from_bits(existing_offsets_bits).as_ptr()
            && object_type_id(existing_offsets_ptr) == TYPE_ID_DICT
        {
            merged_offsets_ptr = existing_offsets_ptr;
        }

        let builtins = builtin_classes(_py);
        let reserved_tail = if issubclass_bits(class_bits, builtins.dict) {
            2 * std::mem::size_of::<u64>()
        } else {
            std::mem::size_of::<u64>()
        };
        let mut layout_size = 0usize;
        if let Some(existing_size_bits) = dict_get_in_place(_py, dict_ptr, layout_name_bits)
            && let Some(existing_size) = obj_from_bits(existing_size_bits).as_int()
            && existing_size > 0
        {
            layout_size = existing_size as usize;
        }
        let hinted_size = match to_i64(obj_from_bits(size_bits)) {
            Some(value) if value >= 0 => value as usize,
            _ => {
                return Err(raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "__molt_layout_size__ must be int",
                ));
            }
        };
        layout_size = layout_size.max(hinted_size);
        if !merged_offsets_ptr.is_null() {
            let required =
                max_slot_end_from_offsets_dict(merged_offsets_ptr).saturating_add(reserved_tail);
            layout_size = layout_size.max(required);
        }
        if layout_size == 0 {
            layout_size = reserved_tail.max(std::mem::size_of::<u64>());
        }
        let layout_bits = MoltObject::from_int(layout_size as i64).bits();
        dict_set_in_place(_py, dict_ptr, layout_name_bits, layout_bits);
        if !apply_class_slots_layout(_py, class_ptr) {
            return Err(MoltObject::none().bits());
        }
        Ok(())
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_class_merge_layout(
    class_bits: u64,
    offsets_bits: u64,
    size_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let class_obj = obj_from_bits(class_bits);
        let Some(class_ptr) = class_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "class layout merge expects type");
        };
        unsafe {
            if object_type_id(class_ptr) != TYPE_ID_TYPE {
                return raise_exception::<_>(_py, "TypeError", "class layout merge expects type");
            }
            match merge_class_layout_metadata(_py, class_ptr, offsets_bits, size_bits) {
                Ok(()) => MoltObject::none().bits(),
                Err(bits) => bits,
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_super_new(type_bits: u64, obj_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let type_obj = obj_from_bits(type_bits);
        let Some(type_ptr) = type_obj.as_ptr() else {
            let got = type_name(_py, type_obj);
            let msg = format!("super() argument 1 must be a type, not {got}");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        unsafe {
            if object_type_id(type_ptr) != TYPE_ID_TYPE {
                let got = type_name(_py, type_obj);
                let msg = format!("super() argument 1 must be a type, not {got}");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
        let obj = obj_from_bits(obj_bits);
        // CPython allows `super(type)` and `super(type, None)` as the "unbound" form.
        if obj.is_none() || obj_bits == 0 {
            let ptr = alloc_super_obj(_py, type_bits, MoltObject::none().bits());
            return if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            };
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
                "super(type, obj): obj must be an instance or subtype of type",
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_classmethod_new(func_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ptr = alloc_classmethod_obj(_py, func_bits);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bootstrap_descriptor_types() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let builtins = builtin_classes(_py);
        let tuple_ptr = alloc_tuple(
            _py,
            &[
                builtins.classmethod,
                builtins.staticmethod,
                builtins.property,
            ],
        );
        if tuple_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_generic_alias_new(origin_bits: u64, args_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let args_obj = obj_from_bits(args_bits);
        // Always create a fresh heap-allocated args tuple.  This is
        // necessary because the incoming tuple may be stack-allocated
        // (from the Cranelift stack-tuple optimisation) and would become
        // a dangling pointer once the caller's stack frame is unwound.
        // Copying the elements into a new heap tuple is cheap and safe.
        let args_tuple_bits = if let Some(args_ptr) = args_obj.as_ptr() {
            unsafe {
                if object_type_id(args_ptr) == TYPE_ID_TUPLE {
                    let elems = seq_vec_ref(args_ptr);
                    let new_ptr = alloc_tuple(_py, elems);
                    if new_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    MoltObject::from_ptr(new_ptr).bits()
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
        let ptr = alloc_generic_alias(_py, origin_bits, args_tuple_bits);
        // The new tuple was created above; dec_ref since alloc_generic_alias
        // inc_refs it.
        dec_ref_bits(_py, args_tuple_bits);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_generic_alias_mro_entries(alias_bits: u64, _bases_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(alias_ptr) = obj_from_bits(alias_bits).as_ptr() else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "GenericAlias.__mro_entries__ expected GenericAlias",
            );
        };
        unsafe {
            if object_type_id(alias_ptr) != TYPE_ID_GENERIC_ALIAS {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "GenericAlias.__mro_entries__ expected GenericAlias",
                );
            }
            let origin_bits = generic_alias_origin_bits(alias_ptr);
            let tuple_ptr = alloc_tuple(_py, &[origin_bits]);
            if tuple_ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(tuple_ptr).bits()
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_generic_alias_type_new(
    cls_bits: u64,
    origin_bits: u64,
    args_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let cls_obj = obj_from_bits(cls_bits);
        let Some(cls_ptr) = cls_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "GenericAlias.__new__ expects type");
        };
        unsafe {
            if object_type_id(cls_ptr) != TYPE_ID_TYPE {
                return raise_exception::<_>(_py, "TypeError", "GenericAlias.__new__ expects type");
            }
        }
        let builtins = builtin_classes(_py);
        let is_generic_alias_subtype =
            cls_bits == builtins.generic_alias || issubclass_bits(cls_bits, builtins.generic_alias);
        if !is_generic_alias_subtype {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "GenericAlias.__new__ expected GenericAlias subtype",
            );
        }

        let out_bits = molt_generic_alias_new(origin_bits, args_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let Some(out_ptr) = obj_from_bits(out_bits).as_ptr() else {
            return out_bits;
        };
        unsafe {
            let old_class_bits = object_class_bits(out_ptr);
            if old_class_bits != cls_bits {
                if old_class_bits != 0 {
                    dec_ref_bits(_py, old_class_bits);
                }
                object_set_class_bits(_py, out_ptr, cls_bits);
                inc_ref_bits(_py, cls_bits);
            }
        }
        out_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_typing_type_param(typevar_ctor_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_staticmethod_new(func_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ptr = alloc_staticmethod_obj(_py, func_bits);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_property_new(get_bits: u64, set_bits: u64, del_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ptr = alloc_property_obj(_py, get_bits, set_bits, del_bits);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_property_getter(prop_bits: u64, get_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_property_setter(prop_bits: u64, set_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_property_deleter(prop_bits: u64, del_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_dynamic_class_attr_init(
    self_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_dynamic_class_attr_get(
    self_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_dynamic_class_attr_set(
    self_bits: u64,
    instance_bits: u64,
    value_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_dynamic_class_attr_delete(self_bits: u64, instance_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_dynamic_class_attr_getter(self_bits: u64, fget_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_dynamic_class_attr_setter(self_bits: u64, fset_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_dynamic_class_attr_deleter(self_bits: u64, fdel_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
/// `obj_ptr_bits` must encode a valid Molt object header that can be mutated,
/// and `class_bits` must be either zero or a valid Molt type object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_object_set_class(obj_ptr_bits: u64, class_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let obj_ptr = obj_ptr_bits as usize as *mut u8;
            if obj_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let header = header_from_obj_ptr(obj_ptr);
            if crate::object::object_poll_fn(obj_ptr) != 0 {
                return raise_exception::<_>(_py, "TypeError", "cannot set class on async object");
            }
            if object_type_id(obj_ptr) == TYPE_ID_DATACLASS {
                return dataclass_set_class_raw(_py, obj_ptr, class_bits);
            }
            if class_bits != 0 {
                let class_obj = obj_from_bits(class_bits);
                let Some(class_ptr) = class_obj.as_ptr() else {
                    return MoltObject::none().bits();
                };
                if object_type_id(class_ptr) != TYPE_ID_TYPE {
                    return MoltObject::none().bits();
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
}

macro_rules! define_types_runtime_state {
    (@unit $field:ident) => {
        ()
    };
    ($($field:ident),+ $(,)?) => {
        const TYPES_RUNTIME_SLOT_COUNT: usize = <[()]>::len(&[
            $(define_types_runtime_state!(@unit $field)),+
        ]);

        pub(crate) struct TypesRuntimeState {
            $(pub(crate) $field: AtomicU64,)+
        }

        impl TypesRuntimeState {
            pub(crate) fn new() -> Self {
                Self {
                    $($field: AtomicU64::new(0),)+
                }
            }

            fn slots(&self) -> Vec<&AtomicU64> {
                let mut slots = Vec::with_capacity(TYPES_RUNTIME_SLOT_COUNT);
                $(slots.push(&self.$field);)+
                slots
            }
        }
    };
}

define_types_runtime_state! {
    mappingproxy_class,
    simplenamespace_class,
    capsule_class,
    cell_class,
    dynamic_class_attribute_class,
    method_class,
    mappingproxy_new_fn,
    mappingproxy_init_fn,
    mappingproxy_getitem_fn,
    mappingproxy_iter_fn,
    mappingproxy_len_fn,
    mappingproxy_contains_fn,
    mappingproxy_get_fn,
    mappingproxy_keys_fn,
    mappingproxy_items_fn,
    mappingproxy_values_fn,
    mappingproxy_repr_fn,
    mappingproxy_setitem_fn,
    mappingproxy_delitem_fn,
    simplenamespace_init_fn,
    simplenamespace_repr_fn,
    simplenamespace_eq_fn,
    dynamic_class_attribute_init_fn,
    dynamic_class_attribute_get_fn,
    dynamic_class_attribute_set_fn,
    dynamic_class_attribute_delete_fn,
    dynamic_class_attribute_getter_fn,
    dynamic_class_attribute_setter_fn,
    dynamic_class_attribute_deleter_fn,
    capsule_new_fn,
    cell_new_fn,
    method_new_fn,
    method_init_fn,
    types_coroutine_fn,
    types_get_original_bases_fn,
    types_prepare_class_fn,
    types_resolve_bases_fn,
    types_new_class_fn,
}

fn types_state(_py: &PyToken<'_>) -> &'static TypesRuntimeState {
    &runtime_state(_py).types
}

pub(crate) fn types_clear_runtime_state(_py: &PyToken<'_>, state: &RuntimeState) {
    crate::gil_assert();
    let slots = state.types.slots();
    clear_atomic_slots(_py, &slots);
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

fn bootstrap_runtime_func_bits(
    _py: &PyToken<'_>,
    slot: &AtomicU64,
    fn_ptr: u64,
    arity: u64,
) -> u64 {
    init_atomic_bits(_py, slot, || {
        let ptr = crate::builtins::functions::alloc_runtime_function_obj(_py, fn_ptr, arity);
        if ptr.is_null() {
            0
        } else {
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
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
            && unsafe { object_type_id(dict_ptr) } == TYPE_ID_DICT
        {
            let layout_name = intern_static_name(
                _py,
                &runtime_state(_py).interned.molt_layout_size,
                b"__molt_layout_size__",
            );
            let layout_bits = MoltObject::from_int(layout_size).bits();
            unsafe { dict_set_in_place(_py, dict_ptr, layout_name, layout_bits) };
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
    let dict_bits = unsafe { function_dict_bits(func_ptr) };
    let dict_ptr = if dict_bits == 0 {
        let dict_ptr = alloc_dict_with_pairs(_py, &[]);
        if dict_ptr.is_null() {
            return;
        }
        unsafe { crate::function_set_dict_bits(func_ptr, MoltObject::from_ptr(dict_ptr).bits()) };
        dict_ptr
    } else {
        let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
            return;
        };
        unsafe {
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return;
            }
        }
        dict_ptr
    };
    let arg_names = intern_static_name(
        _py,
        &runtime_state(_py).interned.molt_arg_names,
        b"__molt_arg_names__",
    );
    if unsafe { dict_get_in_place(_py, dict_ptr, arg_names) }.is_none() {
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
            unsafe { dict_set_in_place(_py, dict_ptr, arg_names, names_bits) };
            dec_ref_bits(_py, names_bits);
        }
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
    unsafe { *(ptr as *const u64) }
}

unsafe fn mappingproxy_set_mapping_bits(ptr: *mut u8, bits: u64) {
    unsafe {
        *(ptr as *mut u64) = bits;
    }
}

fn mappingproxy_class(_py: &PyToken<'_>) -> u64 {
    let class_bits = types_class(
        _py,
        &types_state(_py).mappingproxy_class,
        "mappingproxy",
        16,
    );
    let new_bits = builtin_func_bits(
        _py,
        &types_state(_py).mappingproxy_new_fn,
        crate::molt_types_mappingproxy_new as *const () as usize as u64,
        2,
    );
    let init_bits = builtin_func_bits(
        _py,
        &types_state(_py).mappingproxy_init_fn,
        crate::molt_types_mappingproxy_init as *const () as usize as u64,
        2,
    );
    let getitem_bits = builtin_func_bits(
        _py,
        &types_state(_py).mappingproxy_getitem_fn,
        crate::molt_types_mappingproxy_getitem as *const () as usize as u64,
        2,
    );
    let iter_bits = builtin_func_bits(
        _py,
        &types_state(_py).mappingproxy_iter_fn,
        crate::molt_types_mappingproxy_iter as *const () as usize as u64,
        1,
    );
    let len_bits = builtin_func_bits(
        _py,
        &types_state(_py).mappingproxy_len_fn,
        crate::molt_types_mappingproxy_len as *const () as usize as u64,
        1,
    );
    let contains_bits = builtin_func_bits(
        _py,
        &types_state(_py).mappingproxy_contains_fn,
        crate::molt_types_mappingproxy_contains as *const () as usize as u64,
        2,
    );
    let get_bits = builtin_func_bits(
        _py,
        &types_state(_py).mappingproxy_get_fn,
        crate::molt_types_mappingproxy_get as *const () as usize as u64,
        3,
    );
    let keys_bits = builtin_func_bits(
        _py,
        &types_state(_py).mappingproxy_keys_fn,
        crate::molt_types_mappingproxy_keys as *const () as usize as u64,
        1,
    );
    let items_bits = builtin_func_bits(
        _py,
        &types_state(_py).mappingproxy_items_fn,
        crate::molt_types_mappingproxy_items as *const () as usize as u64,
        1,
    );
    let values_bits = builtin_func_bits(
        _py,
        &types_state(_py).mappingproxy_values_fn,
        crate::molt_types_mappingproxy_values as *const () as usize as u64,
        1,
    );
    let repr_bits = builtin_func_bits(
        _py,
        &types_state(_py).mappingproxy_repr_fn,
        crate::molt_types_mappingproxy_repr as *const () as usize as u64,
        1,
    );
    let setitem_bits = builtin_func_bits(
        _py,
        &types_state(_py).mappingproxy_setitem_fn,
        crate::molt_types_mappingproxy_setitem as *const () as usize as u64,
        3,
    );
    let delitem_bits = builtin_func_bits(
        _py,
        &types_state(_py).mappingproxy_delitem_fn,
        crate::molt_types_mappingproxy_delitem as *const () as usize as u64,
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
        crate::molt_types_method_new as *const () as usize as u64,
        3,
    );
    let init_bits = builtin_func_bits(
        _py,
        &types_state(_py).method_init_fn,
        crate::molt_types_method_init as *const () as usize as u64,
        3,
    );
    set_class_method(_py, class_bits, "__new__", new_bits);
    set_class_method(_py, class_bits, "__init__", init_bits);
    class_bits
}

fn simplenamespace_class(_py: &PyToken<'_>) -> u64 {
    let class_bits = types_class(
        _py,
        &types_state(_py).simplenamespace_class,
        "SimpleNamespace",
        8,
    );
    let init_bits = builtin_func_bits(
        _py,
        &types_state(_py).simplenamespace_init_fn,
        crate::molt_types_simplenamespace_init as *const () as usize as u64,
        3,
    );
    let repr_bits = builtin_func_bits(
        _py,
        &types_state(_py).simplenamespace_repr_fn,
        crate::molt_types_simplenamespace_repr as *const () as usize as u64,
        1,
    );
    let eq_bits = builtin_func_bits(
        _py,
        &types_state(_py).simplenamespace_eq_fn,
        crate::molt_types_simplenamespace_eq as *const () as usize as u64,
        2,
    );
    set_class_method(_py, class_bits, "__init__", init_bits);
    set_class_method(_py, class_bits, "__repr__", repr_bits);
    set_class_method(_py, class_bits, "__eq__", eq_bits);
    mark_vararg_method(_py, init_bits, true);
    class_bits
}

fn capsule_class(_py: &PyToken<'_>) -> u64 {
    let class_bits = types_class(_py, &types_state(_py).capsule_class, "capsule", 8);
    let new_bits = builtin_func_bits(
        _py,
        &types_state(_py).capsule_new_fn,
        crate::molt_types_capsule_new as *const () as usize as u64,
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
        crate::molt_types_cell_new as *const () as usize as u64,
        1,
    );
    set_class_method(_py, class_bits, "__new__", new_bits);
    class_bits
}

fn iter_next_pair(_py: &PyToken<'_>, iter_bits: u64) -> Option<(u64, bool)> {
    let pair_bits = molt_iter_next(iter_bits);
    let pair_obj = obj_from_bits(pair_bits);
    let pair_ptr = pair_obj.as_ptr()?;
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_keyword_lists() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_keyword_iskeyword(value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        MoltObject::from_bool(keyword_contains(value_bits, HARD_KEYWORDS)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_keyword_issoftkeyword(value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        MoltObject::from_bool(keyword_contains(value_bits, SOFT_KEYWORDS)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_future_features() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let mut rows: Vec<u64> = Vec::with_capacity(FUTURE_FEATURES.len());
        for feature in FUTURE_FEATURES {
            let name_ptr = alloc_string(_py, feature.name.as_bytes());
            if name_ptr.is_null() {
                eprintln!(
                    "MOLT_WARN: molt_future_features: alloc_string failed for '{}'",
                    feature.name
                );
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_stdlib_probe() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { MoltObject::from_bool(true).bits() })
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_coroutine(func_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
    // Keep any temporary tuples (including bases_tuple_bits and __mro_entries__ results) alive
    // until we materialize the final resolved bases tuple. Otherwise, elements that are only
    // referenced by those temporary tuples may be freed, leaving dangling bits in `out`.
    let mut keepalive_tuples: Vec<u64> = Vec::new();

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
            for bits in keepalive_tuples.drain(..) {
                dec_ref_bits(_py, bits);
            }
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
            for bits in keepalive_tuples.drain(..) {
                dec_ref_bits(_py, bits);
            }
            dec_ref_bits(_py, mro_entries_name_bits);
            dec_ref_bits(_py, bases_tuple_bits);
            return MoltObject::none().bits();
        }
        let Some(resolved_ptr) = obj_from_bits(resolved_bits).as_ptr() else {
            for bits in keepalive_tuples.drain(..) {
                dec_ref_bits(_py, bits);
            }
            dec_ref_bits(_py, mro_entries_name_bits);
            dec_ref_bits(_py, bases_tuple_bits);
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(resolved_ptr) != TYPE_ID_TUPLE {
                dec_ref_bits(_py, resolved_bits);
                for bits in keepalive_tuples.drain(..) {
                    dec_ref_bits(_py, bits);
                }
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
        // Keep the returned tuple alive until after we allocate the final output tuple.
        keepalive_tuples.push(resolved_bits);
    }

    dec_ref_bits(_py, mro_entries_name_bits);
    if !updated {
        dec_ref_bits(_py, bases_tuple_bits);
        inc_ref_bits(_py, bases_bits);
        return bases_bits;
    }
    let out_ptr = alloc_tuple(_py, out.as_slice());
    // Now that `out_ptr` has taken ownership (via inc-refs) of the elements, we can drop the
    // temporary tuples that were keeping the elements alive.
    for bits in keepalive_tuples.drain(..) {
        dec_ref_bits(_py, bits);
    }
    dec_ref_bits(_py, bases_tuple_bits);
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
    if let Some(bits) = metaclass_bits {
        // `dict_get_in_place` returns a borrowed reference into `kwds_copy`.
        // The following `dict_del_in_place` drops the dict's strong reference to
        // this value, which frees the object outright when the dict held the
        // only reference (the common `prepare_class(name, (), {'metaclass':
        // Factory()})` literal case). Acquire an owned reference *before* the
        // delete so the metaclass survives — without this, `winner_bits` below
        // would dangle and `__prepare__` would be read from freed memory (a
        // use-after-free that release-mode codegen happened to mask while
        // dev-mode codegen surfaced as a missing namespace key).
        inc_ref_bits(_py, bits);
        unsafe {
            dict_del_in_place(_py, kwds_copy_ptr, metaclass_name_bits);
        }
        if exception_pending(_py) {
            dec_ref_bits(_py, bits);
            dec_ref_bits(_py, metaclass_name_bits);
            dec_ref_bits(_py, kwds_copy_bits);
            return None;
        }
    }
    dec_ref_bits(_py, metaclass_name_bits);

    // `winner_bits` is held as an OWNED reference for the whole function and is
    // returned as an owned reference in `PreparedClassState.metaclass_bits`
    // (both callers decref it, symmetric with `namespace_bits`/`kwds_bits`).
    // The dict branch is already owned (incref'd above); the registry-backed
    // branches return borrowed references, so incref them to make ownership
    // uniform.
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
        inc_ref_bits(_py, bits);
        dec_ref_bits(_py, first_base_bits);
        bits
    } else {
        let bits = builtin_classes(_py).type_obj;
        inc_ref_bits(_py, bits);
        bits
    };

    let winner_is_type = obj_from_bits(winner_bits)
        .as_ptr()
        .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_TYPE });
    if winner_is_type {
        let Some(bases_tuple_bits) = (unsafe { tuple_from_iter_bits(_py, bases_bits) }) else {
            dec_ref_bits(_py, winner_bits);
            dec_ref_bits(_py, kwds_copy_bits);
            return None;
        };
        let Some(bases_tuple_ptr) = obj_from_bits(bases_tuple_bits).as_ptr() else {
            dec_ref_bits(_py, bases_tuple_bits);
            dec_ref_bits(_py, winner_bits);
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
                // Promote to the more-derived base metaclass. `winner_bits` is
                // owned, so release the prior winner and acquire the new one to
                // keep ownership balanced (the registry-backed `base_meta_bits`
                // is borrowed).
                inc_ref_bits(_py, base_meta_bits);
                dec_ref_bits(_py, winner_bits);
                winner_bits = base_meta_bits;
                continue;
            }
            dec_ref_bits(_py, bases_tuple_bits);
            dec_ref_bits(_py, winner_bits);
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
            dec_ref_bits(_py, winner_bits);
            dec_ref_bits(_py, kwds_copy_bits);
            return None;
        };
        let mut lookup_bits = crate::builtins::attributes::molt_get_attr_name_default(
            winner_bits,
            prepare_name_bits,
            missing,
        );
        dec_ref_bits(_py, prepare_name_bits);
        if exception_pending(_py) {
            if crate::builtins::attr::clear_attribute_error_if_pending(_py) {
                lookup_bits = missing;
            } else {
                dec_ref_bits(_py, winner_bits);
                dec_ref_bits(_py, kwds_copy_bits);
                return None;
            }
        }
        lookup_bits
    };
    let namespace_bits = if crate::is_missing_bits(_py, prepare_bits) {
        let ptr = alloc_dict_with_pairs(_py, &[]);
        if ptr.is_null() {
            dec_ref_bits(_py, winner_bits);
            dec_ref_bits(_py, kwds_copy_bits);
            return None;
        }
        MoltObject::from_ptr(ptr).bits()
    } else {
        let val_bits =
            call_with_kwargs(_py, prepare_bits, &[name_bits, bases_bits], kwds_copy_bits);
        dec_ref_bits(_py, prepare_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, winner_bits);
            dec_ref_bits(_py, kwds_copy_bits);
            return None;
        }
        // CPython parity (3.12+): `__build_class__` rejects a `__prepare__`
        // result that is not a mapping with
        //   TypeError: <metaclass>.__prepare__() must return a mapping, not <type>
        // The runtime check is `PyMapping_Check` (`tp_as_mapping->mp_subscript`).
        // `value_supports_mp_subscript` is the single source of truth for that
        // contract: an exact dict, a dict subclass / custom mapping (class with
        // `__getitem__`), and even list/tuple/str/bytes/bytearray/range/
        // memoryview all pass — the latter then fail downstream on the first
        // string-keyed store with their own "indices must be integers" message,
        // exactly as CPython does.  int/object/set/slice/dict-views (no
        // `mp_subscript`) are rejected here.
        if !crate::object::ops::value_supports_mp_subscript(_py, val_bits) {
            let meta_name = class_name_for_error(winner_bits);
            let ns_type = type_name(_py, obj_from_bits(val_bits)).into_owned();
            dec_ref_bits(_py, val_bits);
            dec_ref_bits(_py, winner_bits);
            dec_ref_bits(_py, kwds_copy_bits);
            let msg = format!("{meta_name}.__prepare__() must return a mapping, not {ns_type}");
            let _ = raise_exception::<u64>(_py, "TypeError", &msg);
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_get_original_bases(cls_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_prepare_class(args_bits: u64, kwargs_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
        // `prepare_class_impl` returns all three state fields as owned
        // references; `alloc_tuple` took its own reference on each element, so
        // release ours.
        dec_ref_bits(_py, state.metaclass_bits);
        dec_ref_bits(_py, state.namespace_bits);
        dec_ref_bits(_py, state.kwds_bits);
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_resolve_bases(args_bits: u64, kwargs_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_new_class(args_bits: u64, kwargs_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
                dec_ref_bits(_py, state.metaclass_bits);
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
                dec_ref_bits(_py, state.metaclass_bits);
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
                dec_ref_bits(_py, state.metaclass_bits);
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
        // `prepare_class_impl` returns `metaclass_bits` as an owned reference;
        // release it now that the metaclass has been called.
        dec_ref_bits(_py, state.metaclass_bits);
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

fn dynamic_class_attribute_class(_py: &PyToken<'_>) -> u64 {
    let class_bits = types_class(
        _py,
        &types_state(_py).dynamic_class_attribute_class,
        "DynamicClassAttribute",
        8,
    );
    if class_bits == 0 || obj_from_bits(class_bits).is_none() {
        return class_bits;
    }
    let init_bits = builtin_func_bits(
        _py,
        &types_state(_py).dynamic_class_attribute_init_fn,
        crate::molt_types_dynamic_class_attr_init as *const () as usize as u64,
        3,
    );
    let get_bits = builtin_func_bits(
        _py,
        &types_state(_py).dynamic_class_attribute_get_fn,
        crate::molt_types_dynamic_class_attr_get as *const () as usize as u64,
        3,
    );
    let set_bits = builtin_func_bits(
        _py,
        &types_state(_py).dynamic_class_attribute_set_fn,
        crate::molt_types_dynamic_class_attr_set as *const () as usize as u64,
        3,
    );
    let delete_bits = builtin_func_bits(
        _py,
        &types_state(_py).dynamic_class_attribute_delete_fn,
        crate::molt_types_dynamic_class_attr_delete as *const () as usize as u64,
        2,
    );
    let getter_bits = builtin_func_bits(
        _py,
        &types_state(_py).dynamic_class_attribute_getter_fn,
        crate::molt_types_dynamic_class_attr_getter as *const () as usize as u64,
        2,
    );
    let setter_bits = builtin_func_bits(
        _py,
        &types_state(_py).dynamic_class_attribute_setter_fn,
        crate::molt_types_dynamic_class_attr_setter as *const () as usize as u64,
        2,
    );
    let deleter_bits = builtin_func_bits(
        _py,
        &types_state(_py).dynamic_class_attribute_deleter_fn,
        crate::molt_types_dynamic_class_attr_deleter as *const () as usize as u64,
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

fn build_types_bootstrap_dict(_py: &PyToken<'_>) -> u64 {
    let debug_bootstrap = std::env::var("MOLT_DEBUG_TYPES_BOOTSTRAP").as_deref() == Ok("1");
    let trace_stage = |stage: &str| {
        if debug_bootstrap {
            eprintln!("molt types bootstrap stage={stage}");
        }
    };
    trace_stage("start");
    let dict_ptr = alloc_dict_with_pairs(_py, &[]);
    if dict_ptr.is_null() {
        return 0;
    }
    let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
    let builtins = builtin_classes(_py);
    trace_stage("builtins");
    let mappingproxy_bits = mappingproxy_class(_py);
    trace_stage("mappingproxy");
    let simplenamespace_bits = simplenamespace_class(_py);
    trace_stage("simplenamespace");
    let capsule_bits = capsule_class(_py);
    trace_stage("capsule");
    let cell_bits = cell_class(_py);
    trace_stage("cell");
    let dynamic_class_attr_bits = dynamic_class_attribute_class(_py);
    trace_stage("dynamic_class_attribute");

    let method_type_bits = method_class(_py);
    trace_stage("method_type_done");

    // Bootstrap-critical descriptor exports must come from stable runtime
    // type objects, not reflective attribute probing that can recurse back
    // into the still-initializing attribute/type machinery.
    let wrapper_descriptor_bits = builtins.builtin_function_or_method;
    trace_stage("wrapper_descriptor");
    let method_wrapper_bits = builtins.builtin_function_or_method;
    trace_stage("method_wrapper");
    let method_descriptor_bits = builtins.builtin_function_or_method;
    trace_stage("method_descriptor");
    let classmethod_descriptor_bits = builtins.builtin_function_or_method;
    trace_stage("classmethod_descriptor");
    let getset_descriptor_bits = builtins.property;
    trace_stage("getset_descriptor");
    let member_descriptor_bits = builtins.property;
    trace_stage("member_descriptor");

    let coroutine_bits = bootstrap_runtime_func_bits(
        _py,
        &types_state(_py).types_coroutine_fn,
        crate::molt_types_coroutine as *const () as usize as u64,
        1,
    );
    if coroutine_bits == 0 {
        dec_ref_bits(_py, dict_bits);
        return 0;
    }
    trace_stage("coroutine_bits");

    let get_original_bases_bits = bootstrap_runtime_func_bits(
        _py,
        &types_state(_py).types_get_original_bases_fn,
        crate::molt_types_get_original_bases as *const () as usize as u64,
        1,
    );
    if get_original_bases_bits == 0 {
        dec_ref_bits(_py, dict_bits);
        return 0;
    }
    trace_stage("get_original_bases");

    let prepare_bits = bootstrap_runtime_func_bits(
        _py,
        &types_state(_py).types_prepare_class_fn,
        crate::molt_types_prepare_class as *const () as usize as u64,
        2,
    );
    if prepare_bits == 0 {
        dec_ref_bits(_py, dict_bits);
        return 0;
    }
    mark_vararg_method(_py, prepare_bits, false);
    trace_stage("prepare_bits");

    let resolve_bits = bootstrap_runtime_func_bits(
        _py,
        &types_state(_py).types_resolve_bases_fn,
        crate::molt_types_resolve_bases as *const () as usize as u64,
        2,
    );
    if resolve_bits == 0 {
        dec_ref_bits(_py, dict_bits);
        return 0;
    }
    mark_vararg_method(_py, resolve_bits, false);
    trace_stage("resolve_bits");

    let new_bits = bootstrap_runtime_func_bits(
        _py,
        &types_state(_py).types_new_class_fn,
        crate::molt_types_new_class as *const () as usize as u64,
        2,
    );
    if new_bits == 0 {
        dec_ref_bits(_py, dict_bits);
        return 0;
    }
    mark_vararg_method(_py, new_bits, false);
    trace_stage("new_bits");

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
    let release_failed_payload = || {
        dec_ref_bits(_py, dict_bits);
        0
    };
    for (name, value_bits) in names.iter() {
        let key_ptr = alloc_string(_py, name.as_bytes());
        if key_ptr.is_null() {
            return release_failed_payload();
        }
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        unsafe {
            dict_set_in_place(_py, dict_ptr, key_bits, *value_bits);
        }
        dec_ref_bits(_py, key_bits);
        if exception_pending(_py) {
            return release_failed_payload();
        }
    }
    trace_stage("dict_populated");
    trace_stage("done");
    dict_bits
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_types_bootstrap() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let dict_bits = build_types_bootstrap_dict(_py);
        if dict_bits == 0 {
            return MoltObject::none().bits();
        }
        dict_bits
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MoltHeader, maybe_ptr_from_bits};
    use std::sync::Once;
    use std::sync::atomic::Ordering;

    static INIT: Once = Once::new();

    fn init_runtime() {
        INIT.call_once(|| {
            assert_ne!(crate::lifecycle::init(), 0);
        });
        let _ = crate::molt_exception_clear();
    }

    unsafe fn ref_count(bits: u64) -> u32 {
        let ptr = maybe_ptr_from_bits(bits).expect("expected heap object");
        let header = unsafe { ptr.sub(std::mem::size_of::<MoltHeader>()) as *const MoltHeader };
        unsafe { (*header).ref_count.load(Ordering::Acquire) }
    }

    #[test]
    fn type_new_borrows_kwargs_dict() {
        init_runtime();

        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                let builtins = builtin_classes(_py);
                let name_ptr = alloc_string(_py, b"KwargsBorrowedTypeNew");
                assert!(!name_ptr.is_null());
                let name_bits = MoltObject::from_ptr(name_ptr).bits();
                let bases_ptr = alloc_tuple(_py, &[builtins.object]);
                assert!(!bases_ptr.is_null());
                let bases_bits = MoltObject::from_ptr(bases_ptr).bits();
                let ns_ptr = alloc_dict_with_pairs(_py, &[]);
                assert!(!ns_ptr.is_null());
                let ns_bits = MoltObject::from_ptr(ns_ptr).bits();
                let kwargs_ptr = alloc_dict_with_pairs(_py, &[]);
                assert!(!kwargs_ptr.is_null());
                let kwargs_bits = MoltObject::from_ptr(kwargs_ptr).bits();
                inc_ref_bits(_py, kwargs_bits);
                let before = ref_count(kwargs_bits);

                let cls_bits = molt_type_new(
                    builtins.type_obj,
                    name_bits,
                    bases_bits,
                    ns_bits,
                    kwargs_bits,
                );

                assert!(
                    !exception_pending(_py),
                    "type.__new__ with empty kwargs left an exception pending"
                );
                assert_eq!(
                    ref_count(kwargs_bits),
                    before,
                    "type.__new__ must borrow kwargs; caller owns argument cleanup"
                );

                dec_ref_bits(_py, cls_bits);
                dec_ref_bits(_py, kwargs_bits);
                dec_ref_bits(_py, kwargs_bits);
                dec_ref_bits(_py, ns_bits);
                dec_ref_bits(_py, bases_bits);
                dec_ref_bits(_py, name_bits);
            }
        });
    }

    #[test]
    fn type_init_borrows_kwargs_dict() {
        init_runtime();

        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                let kwargs_ptr = alloc_dict_with_pairs(_py, &[]);
                assert!(!kwargs_ptr.is_null());
                let kwargs_bits = MoltObject::from_ptr(kwargs_ptr).bits();
                inc_ref_bits(_py, kwargs_bits);
                let before = ref_count(kwargs_bits);

                let result = molt_type_init(
                    MoltObject::none().bits(),
                    MoltObject::none().bits(),
                    MoltObject::none().bits(),
                    MoltObject::none().bits(),
                    kwargs_bits,
                );

                assert!(obj_from_bits(result).is_none());
                assert_eq!(
                    ref_count(kwargs_bits),
                    before,
                    "type.__init__ must borrow kwargs; caller owns argument cleanup"
                );
                dec_ref_bits(_py, kwargs_bits);
                dec_ref_bits(_py, kwargs_bits);
            }
        });
    }

    #[test]
    fn types_bootstrap_returns_fresh_dicts_with_cached_helpers() {
        init_runtime();

        let first_bits = molt_types_bootstrap();
        let second_bits = molt_types_bootstrap();

        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                assert!(
                    !exception_pending(_py),
                    "types bootstrap must not leave an exception pending"
                );
                assert_ne!(
                    first_bits, second_bits,
                    "types bootstrap must return independent module dicts"
                );

                let first_ptr = maybe_ptr_from_bits(first_bits).expect("first bootstrap dict");
                let second_ptr = maybe_ptr_from_bits(second_bits).expect("second bootstrap dict");
                assert_eq!(object_type_id(first_ptr), TYPE_ID_DICT);
                assert_eq!(object_type_id(second_ptr), TYPE_ID_DICT);

                let key_ptr = alloc_string(_py, b"new_class");
                assert!(!key_ptr.is_null());
                let key_bits = MoltObject::from_ptr(key_ptr).bits();
                let first_new_class =
                    dict_get_in_place(_py, first_ptr, key_bits).expect("first new_class");
                let second_new_class =
                    dict_get_in_place(_py, second_ptr, key_bits).expect("second new_class");
                assert_eq!(
                    first_new_class, second_new_class,
                    "fresh bootstrap dicts should share cached runtime helper objects"
                );

                dec_ref_bits(_py, key_bits);
                dec_ref_bits(_py, first_bits);
                dec_ref_bits(_py, second_bits);
            }
        });
    }

    #[test]
    fn types_runtime_state_is_owned_and_clearable() {
        init_runtime();

        let state = RuntimeState::new();
        for slot in state.types.slots() {
            slot.store(MoltObject::from_int(7).bits(), Ordering::Release);
        }

        crate::with_gil_entry_nopanic!(_py, {
            types_clear_runtime_state(_py, &state);
        });

        for slot in state.types.slots() {
            assert_eq!(slot.load(Ordering::Acquire), 0);
        }
    }

    #[test]
    fn vararg_marker_reuses_function_dict_and_preserves_empty_arg_names() {
        init_runtime();

        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                let func_bits = bootstrap_runtime_func_bits(
                    _py,
                    &types_state(_py).types_prepare_class_fn,
                    crate::molt_types_prepare_class as *const () as usize as u64,
                    2,
                );
                assert_ne!(func_bits, 0);
                let func_ptr = maybe_ptr_from_bits(func_bits).expect("prepare_class function");

                mark_vararg_method(_py, func_bits, false);
                let first_dict_bits = function_dict_bits(func_ptr);
                assert_ne!(
                    first_dict_bits, 0,
                    "vararg marker must install a function dict"
                );

                mark_vararg_method(_py, func_bits, false);
                let second_dict_bits = function_dict_bits(func_ptr);
                assert_eq!(
                    first_dict_bits, second_dict_bits,
                    "repeated vararg marking must not replace cached function metadata"
                );

                let dict_ptr = maybe_ptr_from_bits(second_dict_bits).expect("function dict");
                assert_eq!(object_type_id(dict_ptr), TYPE_ID_DICT);
                let arg_names_key = intern_static_name(
                    _py,
                    &runtime_state(_py).interned.molt_arg_names,
                    b"__molt_arg_names__",
                );
                let arg_names_bits = dict_get_in_place(_py, dict_ptr, arg_names_key)
                    .expect("empty arg-name metadata");
                let arg_names_ptr = maybe_ptr_from_bits(arg_names_bits).expect("arg names tuple");
                assert_eq!(object_type_id(arg_names_ptr), TYPE_ID_TUPLE);
                assert_eq!(
                    seq_vec_ref(arg_names_ptr).len(),
                    0,
                    "non-self vararg helpers still need an explicit empty arg-name tuple"
                );
            }
        });
    }
}
