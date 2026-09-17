use std::sync::OnceLock;

use super::*;
use crate::TYPE_ID_FUNCTION;
use crate::builtins::exceptions::ExceptionSentinel;
use crate::object::seq_access::{snapshot, with_immutable_tuple_slice};
use crate::object::type_ids::TYPE_ID_OBJECT;
use crate::object::{
    ClassEdgeOwnership, object_init_class_edge_unpublished, object_payload_size,
    object_replace_class_edge,
};

mod hierarchy;

pub use self::hierarchy::*;

#[cfg(test)]
#[path = "class_namespace_tests.rs"]
mod class_namespace_tests;

#[cfg(test)]
#[path = "class_attachment_tests.rs"]
mod class_attachment_tests;

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
    if exception_pending(_py) {
        return false;
    }
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
    if exception_pending(_py) {
        return false;
    }
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
    // Validate identity before publishing either compiler cell: failed type
    // construction must not expose a class through an otherwise valid cell.
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
    // CPython only normalizes plain Python functions, never descriptors or
    // native builtin-function objects supplied by a namespace provider.
    for (name, static_method) in [
        (b"__new__".as_slice(), true),
        (b"__init_subclass__".as_slice(), false),
        (b"__class_getitem__".as_slice(), false),
    ] {
        let Some(key_bits) = attr_name_bits_from_bytes(_py, name) else {
            return false;
        };
        if let Some(function) = unsafe { dict_get_in_place(_py, dict_ptr, key_bits) } {
            let plain_function = obj_from_bits(function).as_ptr().is_some_and(|ptr| unsafe {
                object_type_id(ptr) == TYPE_ID_FUNCTION
                    && crate::object_class_bits(ptr)
                        != builtin_classes(_py).builtin_function_or_method
            });
            if plain_function {
                let descriptor = if static_method {
                    molt_staticmethod_new(function)
                } else {
                    molt_classmethod_new(function)
                };
                if !exception_pending(_py) {
                    unsafe { dict_set_in_place(_py, dict_ptr, key_bits, descriptor) };
                }
                dec_ref_bits(_py, descriptor);
            }
        }
        dec_ref_bits(_py, key_bits);
        if exception_pending(_py) {
            return false;
        }
    }
    // The two compiler cells share validation, ownership, and publication.
    // Populate the class cell first, matching type.__new__, then publish the
    // finished dictionary; neither metadata key survives on the class itself.
    for (name, value) in [
        (
            b"__classcell__".as_slice(),
            MoltObject::from_ptr(class_ptr).bits(),
        ),
        (b"__classdictcell__".as_slice(), dict_bits),
    ] {
        let Some(key_bits) = attr_name_bits_from_bytes(_py, name) else {
            return false;
        };
        if let Some(cell_bits) = unsafe { dict_get_in_place(_py, dict_ptr, key_bits) } {
            let cell_ptr = crate::object::cells::cell_ptr_from_bits(cell_bits);
            let Some(cell_ptr) = cell_ptr else {
                dec_ref_bits(_py, key_bits);
                let type_repr_bits = crate::molt_repr_builtin(type_of_bits(_py, cell_bits));
                if exception_pending(_py) {
                    dec_ref_bits(_py, type_repr_bits);
                    return false;
                }
                let type_repr = string_obj_to_owned(obj_from_bits(type_repr_bits));
                dec_ref_bits(_py, type_repr_bits);
                let Some(type_repr) = type_repr else {
                    return false;
                };
                let message = format!(
                    "{} must be a nonlocal cell, not {}",
                    String::from_utf8_lossy(name),
                    type_repr,
                );
                let _ = raise_exception::<u64>(_py, "TypeError", &message);
                return false;
            };
            unsafe { crate::object::cells::cell_replace_value(_py, cell_ptr, value) };
            unsafe { dict_del_in_place(_py, dict_ptr, key_bits) };
        }
        dec_ref_bits(_py, key_bits);
        if exception_pending(_py) {
            return false;
        }
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
        // Namespace providers, slot iterators, cell replacement, and class
        // hooks may release caller-visible owners. Keep every borrowed input
        // alive throughout preparation, publication, and failure finalization.
        let _input_owners =
            [cls_bits, name_bits, bases_bits, namespace_bits, kwargs_bits].map(|bits| {
                inc_ref_bits(_py, bits);
                obj_from_bits(bits).as_ptr().map(crate::PtrDropGuard::new)
            });
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

        // Own all keywords before namespace copying or cell replacement can
        // invoke destructors, and before any construction callback can mutate
        // a caller-visible kwargs dictionary.
        let keyword_values: Vec<u64> = kw_pairs
            .iter()
            .flat_map(|&(name, value)| [name, value])
            .collect();
        let keyword_snapshot = if keyword_values.is_empty() {
            std::ptr::null_mut()
        } else {
            let snapshot = alloc_tuple(_py, &keyword_values);
            if snapshot.is_null() {
                return MoltObject::none().bits();
            }
            snapshot
        };
        let _keyword_owner = crate::PtrDropGuard::new(keyword_snapshot);
        let (kw_names, kw_values): (Vec<_>, Vec<_>) = kw_pairs.into_iter().unzip();

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

        if prepare_class_base_layout(_py, &bases_vec, None).is_none() {
            if bases_owned {
                dec_ref_bits(_py, bases_tuple_bits);
            }
            return MoltObject::none().bits();
        }

        // Slot iteration/validation belongs before class allocation: unlike
        // later namespace metadata errors, these failures cannot finalize a
        // class that CPython never created. Copy once and reuse the same private
        // dictionary as the eventual class namespace.
        let namespace_ptr = alloc_dict_with_pairs(_py, &[]);
        if namespace_ptr.is_null() {
            if bases_owned {
                dec_ref_bits(_py, bases_tuple_bits);
            }
            return MoltObject::none().bits();
        }
        let copied_namespace = MoltObject::from_ptr(namespace_ptr).bits();
        let _namespace_owner = crate::PtrDropGuard::new(namespace_ptr);
        unsafe {
            let _ = dict_update_apply(
                _py,
                copied_namespace,
                dict_update_set_in_place,
                namespace_bits,
            );
        }
        if exception_pending(_py) {
            if bases_owned {
                dec_ref_bits(_py, bases_tuple_bits);
            }
            return MoltObject::none().bits();
        }
        let Some(slot_declaration) =
            (unsafe { crate::builtins::attr::prepare_class_slot_declaration(_py, namespace_ptr) })
        else {
            if bases_owned {
                dec_ref_bits(_py, bases_tuple_bits);
            }
            return MoltObject::none().bits();
        };
        let mut slot_owner = obj_from_bits(slot_declaration)
            .as_ptr()
            .map(crate::PtrDropGuard::new);
        let class_ptr = crate::object::builders::alloc_class_obj_with_namespace(
            _py,
            name_bits,
            copied_namespace,
        );
        if class_ptr.is_null() {
            if bases_owned {
                dec_ref_bits(_py, bases_tuple_bits);
            }
            return MoltObject::none().bits();
        }
        let class_bits = MoltObject::from_ptr(class_ptr).bits();
        // The payload is completely initialized before attaching its metaclass.
        // Postallocation metadata failure intentionally invokes metaclass __del__
        // (including resurrection), matching type.__new__. Layout sealing is a
        // separate admission boundary, not a finalizer-suppression state.
        let mut class_owner = crate::PtrDropGuard::new(class_ptr);
        unsafe {
            crate::object::layout::class_set_slot_declaration_owned(class_ptr, slot_declaration);
            if let Some(owner) = &mut slot_owner {
                owner.release();
            }
            if !object_init_class_edge_unpublished(
                _py,
                class_ptr,
                cls_bits,
                ClassEdgeOwnership::Owned,
            ) {
                if bases_owned {
                    dec_ref_bits(_py, bases_tuple_bits);
                }
                return MoltObject::none().bits();
            }
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
        if unsafe { crate::object::class_finish_definition(_py, class_ptr) }.is_err() {
            if bases_owned {
                dec_ref_bits(_py, bases_tuple_bits);
            }
            return MoltObject::none().bits();
        }

        if unsafe { !class_apply_descriptor_names(_py, class_ptr) } {
            if bases_owned {
                dec_ref_bits(_py, bases_tuple_bits);
            }
            return MoltObject::none().bits();
        }
        if unsafe {
            !crate::call::bind::dispatch_init_subclass_hooks(_py, class_bits, &kw_names, &kw_values)
        } {
            if bases_owned {
                dec_ref_bits(_py, bases_tuple_bits);
            }
            return MoltObject::none().bits();
        }
        if bases_owned {
            dec_ref_bits(_py, bases_tuple_bits);
        }
        class_owner.release();
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
            std::mem::size_of::<u64>(),
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
            if crate::object::class_finish_definition(_py, cls_ptr).is_err() {
                return MoltObject::none().bits();
            }
        }
        let owns_plain_object_layout = unsafe {
            crate::object::class_instance_type_id(cls_ptr) == TYPE_ID_OBJECT
                && crate::object::class_instance_shape_id(cls_ptr)
                    == crate::object::ObjectShapeId::Plain
        };
        if !owns_plain_object_layout {
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
        if cls_bits == builtins.tuple {
            if iterable_bits == missing_bits(_py) {
                let ptr = alloc_tuple(_py, &[]);
                return if ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(ptr).bits()
                };
            }
            return unsafe { tuple_from_iter_bits(_py, iterable_bits) }
                .unwrap_or_else(|| MoltObject::none().bits());
        }
        if !unsafe { crate::object::builders::admit_tuple_subclass_layout(_py, cls_ptr) } {
            return MoltObject::none().bits();
        }
        if iterable_bits == missing_bits(_py) {
            return unsafe { crate::object::builders::alloc_tuple_subclass(_py, cls_bits, &[]) };
        }

        let Some(tuple_bits) = (unsafe { tuple_from_iter_bits(_py, iterable_bits) }) else {
            return MoltObject::none().bits();
        };
        let Some(tuple_ptr) = obj_from_bits(tuple_bits).as_ptr() else {
            dec_ref_bits(_py, tuple_bits);
            return MoltObject::none().bits();
        };
        let Some(elems) = (unsafe { snapshot(_py, tuple_ptr, "tuple subclass allocation failed") })
        else {
            dec_ref_bits(_py, tuple_bits);
            return MoltObject::none().bits();
        };
        let result =
            unsafe { crate::object::builders::alloc_tuple_subclass(_py, cls_bits, &elems) };
        dec_ref_bits(_py, tuple_bits);
        result
    })
}

const CLASS_ATTACHMENT_LAYOUT_ERROR: &str =
    "object class assignment requires an identical sealed instance layout";

unsafe fn class_attachment_fields(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    class_ptr: *mut u8,
) -> Vec<crate::object::field_storage::InstanceField> {
    let mut fields = Vec::new();
    unsafe {
        crate::object::field_storage::for_each_instance_field(
            _py,
            obj_ptr,
            class_ptr,
            &mut |field, _| fields.push(field),
        );
    }
    fields.sort_unstable_by_key(|field| field.offset);
    fields
}

unsafe fn class_attachment_layouts_match(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    current_class: *mut u8,
    target_class: *mut u8,
) -> bool {
    unsafe {
        let Some(current_size) = crate::object::layout::class_cached_layout_size(current_class)
        else {
            return false;
        };
        let Some(target_size) = crate::object::layout::class_cached_layout_size(target_class)
        else {
            return false;
        };
        if current_size != target_size
            || object_payload_size(obj_ptr) < target_size
            || object_type_id(obj_ptr) != crate::object::class_instance_type_id(current_class)
            || object_type_id(obj_ptr) != crate::object::class_instance_type_id(target_class)
            || crate::object::object_shape_id(obj_ptr)
                != crate::object::class_instance_shape_id(current_class)
            || crate::object::class_instance_shape_id(current_class)
                != crate::object::class_instance_shape_id(target_class)
            || crate::object::class_exception_layout_root(current_class)
                != crate::object::class_exception_layout_root(target_class)
        {
            return false;
        }

        let current_slots = crate::builtins::attr::class_slots_info(_py, current_class)
            .map(|info| (info.allows_dict, info.allows_weakref));
        let target_slots = crate::builtins::attr::class_slots_info(_py, target_class)
            .map(|info| (info.allows_dict, info.allows_weakref));
        if exception_pending(_py) || current_slots != target_slots {
            return false;
        }

        let current_fields = class_attachment_fields(_py, obj_ptr, current_class);
        let target_fields = class_attachment_fields(_py, obj_ptr, target_class);
        current_fields.len() == target_fields.len()
            && current_fields
                .iter()
                .zip(target_fields.iter())
                .all(|(current, target)| {
                    current.offset == target.offset
                        && current.declared_slot == target.declared_slot
                        && crate::builtins::attr::exact_string_bits_equal(current.name, target.name)
                })
    }
}

fn reject_class_attachment<T: ExceptionSentinel>(_py: &PyToken<'_>) -> T {
    raise_exception::<T>(_py, "TypeError", CLASS_ATTACHMENT_LAYOUT_ERROR)
}

pub(crate) unsafe fn object_set_class(_py: &PyToken<'_>, obj_ptr: *mut u8, class_bits: u64) -> u64 {
    unsafe {
        if obj_ptr.is_null() {
            return MoltObject::none().bits();
        }
        if crate::object::object_poll_fn(obj_ptr) != 0 {
            return raise_exception::<_>(_py, "TypeError", "cannot set class on async object");
        }
        if class_bits == 0 || obj_from_bits(class_bits).is_none() {
            return reject_class_attachment(_py);
        }
        let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
            return reject_class_attachment(_py);
        };
        if object_type_id(class_ptr) != TYPE_ID_TYPE {
            return reject_class_attachment(_py);
        }
        if crate::object::class_finish_definition(_py, class_ptr).is_err() {
            return MoltObject::none().bits();
        }
        if object_type_id(obj_ptr) == TYPE_ID_DATACLASS {
            // Dataclass values carry descriptor-owned field storage whose
            // physical compatibility is not described by generic class field
            // offsets. Unpublished construction has a separate initializer;
            // published reassignment remains closed until that representation
            // has a sealed compatibility authority.
            return reject_class_attachment(_py);
        }

        let current_class_bits = object_class_bits(obj_ptr);
        if current_class_bits == class_bits {
            return MoltObject::none().bits();
        }
        if current_class_bits == 0 {
            return reject_class_attachment(_py);
        }

        let Some(current_class) = obj_from_bits(current_class_bits).as_ptr() else {
            return reject_class_attachment(_py);
        };
        if object_type_id(current_class) != TYPE_ID_TYPE
            || crate::object::class_finish_definition(_py, current_class).is_err()
        {
            return MoltObject::none().bits();
        }
        if !class_attachment_layouts_match(_py, obj_ptr, current_class, class_ptr) {
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            return reject_class_attachment(_py);
        }
        if !object_replace_class_edge(_py, obj_ptr, class_bits, ClassEdgeOwnership::Owned) {
            return reject_class_attachment(_py);
        }
        MoltObject::none().bits()
    }
}

/// # Safety
/// `obj_ptr_bits` must encode a valid Molt object header that can be mutated,
/// and `class_bits` must encode a valid Molt type object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_object_set_class(obj_ptr_bits: u64, class_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            let Some(obj_ptr) = crate::provenance::abi::mut_ptr::<u8>(obj_ptr_bits) else {
                return MoltObject::none().bits();
            };
            object_set_class(_py, obj_ptr, class_bits)
        })
    }
}
