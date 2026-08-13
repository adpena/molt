use std::sync::OnceLock;

use super::*;
use crate::object::seq_access::{snapshot, with_immutable_tuple_slice};
use crate::object::{
    ClassEdgeOwnership, object_init_class_edge_unpublished, object_replace_class_edge,
};

mod hierarchy;

pub use self::hierarchy::*;

/// Consume structural metadata from a freshly copied class namespace.
///
/// Namespace entries are installed directly into the unpublished class
/// dictionary. Only metadata that owns dedicated type storage is consumed;
/// replaying the namespace through `type.__setattr__` changes descriptor
/// precedence and can observe a partially published type.
pub(crate) unsafe fn class_finalize_namespace_metadata(
    _py: &PyToken<'_>,
    class_ptr: *mut u8,
    default_qualname_bits: u64,
) -> bool {
    let dict_bits = unsafe { class_dict_bits(class_ptr) };
    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
        return false;
    };
    if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
        return false;
    }
    let qualname_name_bits = intern_static_name(
        _py,
        &runtime_state(_py).interned.qualname_name,
        b"__qualname__",
    );
    let mut qualname_bits = default_qualname_bits;
    let mut qualname_owned = false;
    if let Some(bits) = unsafe { dict_get_in_place(_py, dict_ptr, qualname_name_bits) } {
        qualname_bits = bits;
        inc_ref_bits(_py, qualname_bits);
        qualname_owned = true;
        unsafe { dict_del_in_place(_py, dict_ptr, qualname_name_bits) };
        if exception_pending(_py) {
            dec_ref_bits(_py, qualname_bits);
            return false;
        }
    }
    if let Some(classdictcell_bits) = attr_name_bits_from_bytes(_py, b"__classdictcell__") {
        unsafe { dict_del_in_place(_py, dict_ptr, classdictcell_bits) };
        dec_ref_bits(_py, classdictcell_bits);
        if exception_pending(_py) {
            if qualname_owned {
                dec_ref_bits(_py, qualname_bits);
            }
            return false;
        }
    }
    let qualname_obj = obj_from_bits(qualname_bits);
    if !qualname_obj
        .as_ptr()
        .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_STRING })
    {
        let type_label = type_name(_py, qualname_obj);
        if qualname_owned {
            dec_ref_bits(_py, qualname_bits);
        }
        let _ = raise_exception::<u64>(
            _py,
            "TypeError",
            &format!("type __qualname__ must be a str, not {type_label}"),
        );
        return false;
    }
    unsafe { class_set_qualname_bits(_py, class_ptr, qualname_bits) };
    if qualname_owned {
        dec_ref_bits(_py, qualname_bits);
    }
    true
}

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
            if !object_init_class_edge_unpublished(
                _py,
                ptr,
                builtins.type_obj,
                ClassEdgeOwnership::Owned,
            ) {
                dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
                return MoltObject::none().bits();
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
                        let Some(copied) =
                            with_immutable_tuple_slice(bases_ptr, |bases| bases.to_vec())
                        else {
                            return MoltObject::none().bits();
                        };
                        bases_vec = copied;
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
            if !object_init_class_edge_unpublished(
                _py,
                class_ptr,
                cls_bits,
                ClassEdgeOwnership::Owned,
            ) {
                dec_ref_bits(_py, class_bits);
                if bases_owned {
                    dec_ref_bits(_py, bases_tuple_bits);
                }
                return MoltObject::none().bits();
            }
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
        if unsafe { !class_finalize_namespace_metadata(_py, class_ptr, name_bits) } {
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
        let class_bits = builtin_classes(_py).object;
        let obj_bits = crate::object::builders::alloc_class_instance(
            _py,
            std::mem::size_of::<u64>() as u64,
            class_bits,
        );
        let Some(obj_ptr) = obj_from_bits(obj_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            // `OBJECT_NEW` is also the generic default-`object.__new__` seed:
            // class construction may replace this edge before publishing the
            // instance to user code. Select CLASS_INLINE at allocation instead
            // of trying to mutate the immutable aux representation later.
            (*crate::object::header_from_obj_ptr(obj_ptr))
                .fetch_or_flags(crate::object::HEADER_FLAG_RAW_ALLOC);
            crate::object::gc::gc_publish_initialized(_py, obj_ptr);
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
            let Some(elems) =
                (unsafe { snapshot(_py, tuple_ptr, "tuple subclass allocation failed") })
            else {
                dec_ref_bits(_py, tuple_bits);
                return MoltObject::none().bits();
            };
            let new_ptr = alloc_tuple(_py, &elems);
            dec_ref_bits(_py, tuple_bits);
            if new_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let new_bits = MoltObject::from_ptr(new_ptr).bits();
            unsafe {
                if !object_init_class_edge_unpublished(
                    _py,
                    new_ptr,
                    cls_bits,
                    ClassEdgeOwnership::Owned,
                ) {
                    dec_ref_bits(_py, new_bits);
                    return MoltObject::none().bits();
                }
            }
            return new_bits;
        }
        if let Some(tuple_ptr) = obj_from_bits(tuple_bits).as_ptr() {
            unsafe {
                if !object_init_class_edge_unpublished(
                    _py,
                    tuple_ptr,
                    cls_bits,
                    ClassEdgeOwnership::Owned,
                ) {
                    dec_ref_bits(_py, tuple_bits);
                    return MoltObject::none().bits();
                }
            }
        }
        tuple_bits
    })
}

/// # Safety
/// `obj_ptr_bits` must encode a valid Molt object header that can be mutated,
/// and `class_bits` must be either zero or a valid Molt type object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_object_set_class(obj_ptr_bits: u64, class_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let Some(obj_ptr) = crate::provenance::abi::mut_ptr::<u8>(obj_ptr_bits) else {
                return MoltObject::none().bits();
            };
            if obj_ptr.is_null() {
                return MoltObject::none().bits();
            }
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
            if !object_replace_class_edge(_py, obj_ptr, class_bits, ClassEdgeOwnership::Owned) {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "__class__ assignment only supported for mutable types or ModuleType subclasses",
                );
            }
            MoltObject::none().bits()
        })
    }
}
