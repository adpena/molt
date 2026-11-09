// dir() and object.__dir__ introspection authority.
// Kept separate from call dispatch and object slot wrappers so builtin method-surface facts have one home.

use crate::object::foreign::NativeObjectOwner;
use crate::*;
use molt_obj_model::MoltObject;

/// Materialize physical native iterator/sequence slots through their existing
/// linked owner. Managed results retain the runtime list constructor fast path.
unsafe fn dir_materialize(py: &PyToken<'_>, result: u64) -> Option<u64> {
    if let Some(pointer) = obj_from_bits(result).as_ptr()
        && unsafe { object_type_id(pointer) == TYPE_ID_FOREIGN }
    {
        use molt_cpython_abi::api::abstract_sequence;
        use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
        let native = unsafe { crate::object::foreign::foreign_ptr_from_obj(pointer) };
        let list = unsafe {
            abstract_sequence::PySequence_List(std::ptr::with_exposed_provenance_mut(native))
        };
        if list.is_null() {
            crate::cpython_abi_hooks::propagate_native_failure(py, "directory native iterable");
            return None;
        }
        let list = unsafe { NativeObjectOwner::from_owned(list) };
        let result = unsafe { GLOBAL_BRIDGE.molt_value_for_pyobj(list.as_ptr()) };
        if result.is_none() {
            crate::cpython_abi_hooks::propagate_native_failure(py, "directory list projection");
        }
        result
    } else {
        unsafe { list_from_iter_bits(py, result) }
    }
}

/// `object.__dir__`: the default protocol over generic attribute lookups, so no
/// raw class-layout walker bypasses `__dict__`/`__class__`/`__bases__` callbacks.
/// The default `object.__dir__` and `type.__dir__` protocol (CPython
/// `_dir_object`, `_dir_type` and `merge_class_dict`). `__dict__`, `__class__`
/// and `__bases__` are generic attribute lookups, so descriptors and other
/// callbacks run exactly as in CPython; the merging uses Molt's own dict and
/// sequence operations. A foreign (C-extension) object keeps the C-API
/// authority, whose type slots may implement those attributes.
unsafe fn dir_default_collect(py: &PyToken<'_>, obj_bits: u64, type_directory: bool) -> u64 {
    let foreign = obj_from_bits(obj_bits)
        .as_ptr()
        .is_some_and(|pointer| unsafe { object_type_id(pointer) } == TYPE_ID_FOREIGN);
    if foreign {
        return unsafe { dir_default_collect_foreign(py, obj_bits, type_directory) };
    }
    match unsafe { dir_default_collect_native(py, obj_bits, type_directory) } {
        Ok(keys) => keys,
        Err(()) => MoltObject::none().bits(),
    }
}

/// A generic attribute lookup that treats a missing attribute as absent.
fn dir_optional_attr(
    py: &PyToken<'_>,
    object: u64,
    name: crate::NameId,
) -> Result<Option<u64>, ()> {
    crate::lookup_attribute(py, object, name.bits(py)).into_result()
}

/// Merge `source` into the fresh `target` dict as `PyDict_Update` does: a
/// mapping only, never an iterable of pairs.
unsafe fn dir_merge_mapping(py: &PyToken<'_>, target: *mut u8, source: u64) -> Result<(), ()> {
    use crate::object::mapping_merge::{MergeOutcome, apply, insert_dict};
    match unsafe {
        apply(
            py,
            source,
            Some(target),
            |_, _| true,
            |key, value, hash| insert_dict(py, target, key, value, hash),
        )
    } {
        MergeOutcome::Complete => Ok(()),
        MergeOutcome::Error | MergeOutcome::NotMapping => Err(()),
    }
}

/// Merge `class`'s namespace and, recursively, its bases' (`merge_class_dict`).
/// `__bases__` is read through the sequence protocol, as `PySequence_Size` and
/// `PySequence_GetItem` read it.
unsafe fn dir_merge_class(py: &PyToken<'_>, target: *mut u8, class: u64) -> Result<(), ()> {
    let Some(_recursion) = crate::state::recursion::RecursionGuard::enter(py) else {
        return Err(());
    };
    if let Some(namespace) = dir_optional_attr(py, class, crate::NameId::DunderDict)? {
        let merged = unsafe { dir_merge_mapping(py, target, namespace) };
        dec_ref_bits(py, namespace);
        merged?;
    }
    let Some(bases) = dir_optional_attr(py, class, crate::NameId::DunderBases)? else {
        return Ok(());
    };
    let merged = (|| {
        let count_bits = crate::molt_len(bases);
        if exception_pending(py) {
            return Err(());
        }
        let Some(count) = to_i64(obj_from_bits(count_bits)) else {
            dec_ref_bits(py, count_bits);
            return Err(());
        };
        for index in 0..count {
            let base = crate::molt_index(bases, MoltObject::from_int(index).bits());
            if exception_pending(py) {
                dec_ref_bits(py, base);
                return Err(());
            }
            let merged = unsafe { dir_merge_class(py, target, base) };
            dec_ref_bits(py, base);
            merged?;
        }
        Ok(())
    })();
    dec_ref_bits(py, bases);
    merged
}

unsafe fn dir_default_collect_native(
    py: &PyToken<'_>,
    obj_bits: u64,
    type_directory: bool,
) -> Result<u64, ()> {
    let dictionary = crate::object::builders::alloc_dict_with_pairs(py, &[]);
    if dictionary.is_null() {
        return Err(());
    }
    let dictionary_bits = MoltObject::from_ptr(dictionary).bits();
    let collected = (|| unsafe {
        if type_directory {
            dir_merge_class(py, dictionary, obj_bits)?;
        } else {
            // A `__dict__` that is not a dict contributes nothing (CPython
            // `_dir_object` starts from a fresh dict then).
            if let Some(namespace) = dir_optional_attr(py, obj_bits, crate::NameId::DunderDict)? {
                // `PyDict_Copy` copies a dict subclass's storage without
                // calling an overriding `keys()`.
                let copied = match crate::object::ops::dict_backing_bits(py, namespace) {
                    Ok(Some(storage)) => dir_merge_mapping(py, dictionary, storage),
                    Ok(None) => Ok(()),
                    Err(()) => Err(()),
                };
                dec_ref_bits(py, namespace);
                copied?;
            }
            if let Some(class) = dir_optional_attr(py, obj_bits, crate::NameId::DunderClass)? {
                let merged = dir_merge_class(py, dictionary, class);
                dec_ref_bits(py, class);
                merged?;
            }
            dir_filter_native_callable_metadata(py, obj_bits, dictionary)?;
        }
        Ok(())
    })();
    let keys =
        collected.and_then(|()| unsafe { list_from_iter_bits(py, dictionary_bits) }.ok_or(()));
    dec_ref_bits(py, dictionary_bits);
    keys
}

/// A native callable instance hides the metadata fields its kind does not
/// declare, as callable lookup does. A type's own metadata does not become
/// instance metadata.
unsafe fn dir_filter_native_callable_metadata(
    py: &PyToken<'_>,
    obj_bits: u64,
    dictionary: *mut u8,
) -> Result<(), ()> {
    use crate::builtins::functions::native_callable::{CallableMetadata, NativeCallableKind};
    let Some(pointer) = obj_from_bits(obj_bits).as_ptr() else {
        return Ok(());
    };
    let Some(kind) = NativeCallableKind::from_class(py, unsafe { object_class_bits(pointer) })
    else {
        return Ok(());
    };
    let allowed = kind.metadata_fields();
    let fields = crate::object::function_metadata::FunctionMetadataField::ALL
        .into_iter()
        .map(CallableMetadata::Stored)
        .chain(std::iter::once(CallableMetadata::Builtins));
    for field in fields {
        if allowed.contains(&field) {
            continue;
        }
        // The interned name of the typed field: no string is spelled or allocated.
        unsafe { crate::dict_del_in_place(py, dictionary, field.name_id().bits(py)) }?;
    }
    Ok(())
}

/// The C-API path for foreign objects: their `__dict__`, `__class__` and
/// `__bases__` may be C type slots that only the C-API authority implements.
unsafe fn dir_default_collect_foreign(
    py: &PyToken<'_>,
    obj_bits: u64,
    type_directory: bool,
) -> u64 {
    use molt_cpython_abi::abi_types::PyObject;
    use molt_cpython_abi::api::{abstract_sequence, mapping, object, refcount::ProtectedPyObject};
    use molt_cpython_abi::bridge::GLOBAL_BRIDGE;

    unsafe fn optional(
        object_ptr: *mut PyObject,
        name: &std::ffi::CStr,
    ) -> Result<Option<NativeObjectOwner>, ()> {
        let mut value = std::ptr::null_mut();
        let status = unsafe {
            object::PyObject_GetOptionalAttrString(object_ptr, name.as_ptr(), &raw mut value)
        };
        match status {
            -1 => Err(()),
            0 => Ok(None),
            _ => Ok(Some(unsafe { NativeObjectOwner::from_owned(value) })),
        }
    }

    unsafe fn merge_class(
        py: &PyToken<'_>,
        dictionary: *mut PyObject,
        class: *mut PyObject,
    ) -> Result<(), ()> {
        let Some(_recursion) = crate::state::recursion::RecursionGuard::enter(py) else {
            return Err(());
        };
        if let Some(namespace) = unsafe { optional(class, c"__dict__") }?
            && unsafe { mapping::PyDict_Update(dictionary, namespace.as_ptr()) } < 0
        {
            return Err(());
        }
        if let Some(bases) = unsafe { optional(class, c"__bases__") }? {
            let count = unsafe { abstract_sequence::PySequence_Size(bases.as_ptr()) };
            if count < 0 {
                return Err(());
            }
            for index in 0..count {
                let base = unsafe { abstract_sequence::PySequence_GetItem(bases.as_ptr(), index) };
                if base.is_null() {
                    return Err(());
                }
                let base = unsafe { NativeObjectOwner::from_owned(base) };
                unsafe { merge_class(py, dictionary, base.as_ptr()) }?;
            }
        }
        Ok(())
    }

    let result = (|| -> Result<NativeObjectOwner, ()> {
        unsafe {
            let source = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(obj_bits);
            if source.is_null() {
                return Err(());
            }
            let source = ProtectedPyObject::from_owned(source);
            let dictionary = if type_directory {
                mapping::PyDict_New()
            } else if let Some(namespace) = optional(source.as_ptr(), c"__dict__")? {
                if mapping::PyDict_Check(namespace.as_ptr()) != 0 {
                    mapping::PyDict_Copy(namespace.as_ptr())
                } else {
                    mapping::PyDict_New()
                }
            } else {
                mapping::PyDict_New()
            };
            if dictionary.is_null() {
                return Err(());
            }
            let dictionary = NativeObjectOwner::from_owned(dictionary);
            if type_directory {
                merge_class(py, dictionary.as_ptr(), source.as_ptr())?;
            } else if let Some(class) = optional(source.as_ptr(), c"__class__")? {
                merge_class(py, dictionary.as_ptr(), class.as_ptr())?;
            }
            if !type_directory
                && let Some(pointer) = obj_from_bits(obj_bits).as_ptr()
                && let Some(kind) =
                    crate::builtins::functions::native_callable::NativeCallableKind::from_class(
                        py,
                        object_class_bits(pointer),
                    )
            {
                // Project the same role declarations as callable lookup. A
                // type's own metadata does not become instance metadata.
                use crate::builtins::functions::native_callable::CallableMetadata;
                let allowed = kind.metadata_fields();
                let fields = crate::object::function_metadata::FunctionMetadataField::ALL
                    .into_iter()
                    .map(CallableMetadata::Stored)
                    .chain(std::iter::once(CallableMetadata::Builtins));
                for field in fields {
                    if allowed.contains(&field) {
                        continue;
                    }
                    let name = std::ffi::CString::new(field.name()).unwrap();
                    let value =
                        mapping::_PyDict_GetItemStringWithError(dictionary.as_ptr(), name.as_ptr());
                    if value.is_null() {
                        if !molt_cpython_abi::api::errors::PyErr_Occurred().is_null() {
                            return Err(());
                        }
                    } else if mapping::PyDict_DelItemString(dictionary.as_ptr(), name.as_ptr()) < 0
                    {
                        return Err(());
                    }
                }
            }
            let result = mapping::PyDict_Keys(dictionary.as_ptr());
            if result.is_null() {
                return Err(());
            }
            let result = NativeObjectOwner::from_owned(result);
            Ok(result)
        }
    })();
    match result {
        Ok(result) => match unsafe { GLOBAL_BRIDGE.molt_value_for_pyobj(result.as_ptr()) } {
            Some(bits) => bits,
            None => {
                crate::cpython_abi_hooks::propagate_native_failure(
                    py,
                    "directory result projection",
                );
                MoltObject::none().bits()
            }
        },
        Err(()) => {
            crate::cpython_abi_hooks::propagate_native_failure(
                py,
                "directory namespace observation",
            );
            MoltObject::none().bits()
        }
    }
}

pub(crate) fn molt_object_dir_method_attached_body(
    _py: &crate::PyToken<'_>,
    self_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        unsafe { dir_default_collect(_py, self_bits, false) }
    })
}

pub(crate) fn molt_type_dir_method_attached_body(_py: &crate::PyToken<'_>, self_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        unsafe { dir_default_collect(_py, self_bits, true) }
    })
}

pub(crate) fn molt_dir_builtin_attached_body(_py: &crate::PyToken<'_>, obj_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let missing = missing_bits(_py);
        if obj_bits == missing {
            // CPython: dir() (no args) lists the caller's local scope.
            unsafe {
                // Observe the caller's namespace under the current attachment.
                let locals_bits = crate::molt_locals_builtin_attached_body(_py);
                if exception_pending(_py) {
                    if !obj_from_bits(locals_bits).is_none() {
                        dec_ref_bits(_py, locals_bits);
                    }
                    return MoltObject::none().bits();
                }
                let list_bits = if type_of_bits(_py, locals_bits) == builtin_classes(_py).dict {
                    list_from_iter_bits(_py, locals_bits)
                } else {
                    // PyMapping_Keys consults keys(), not __iter__. Prepared
                    // class namespaces need not implement the iterator protocol.
                    let name = crate::NameId::Keys.bits(_py);
                    let method =
                        molt_getattr_builtin_attached_body(_py, locals_bits, name, missing);
                    if exception_pending(_py) {
                        dec_ref_bits(_py, method);
                        None
                    } else {
                        let output = call_callable0(_py, method);
                        dec_ref_bits(_py, method);
                        crate::object::mapping_merge::output_as_list(
                            _py,
                            locals_bits,
                            crate::NameId::Keys.as_str().as_bytes(),
                            output,
                        )
                    }
                }
                .unwrap_or_else(|| MoltObject::none().bits());
                if !obj_from_bits(locals_bits).is_none() {
                    dec_ref_bits(_py, locals_bits);
                }
                if obj_from_bits(list_bits).is_none() || exception_pending(_py) {
                    if !obj_from_bits(list_bits).is_none() {
                        dec_ref_bits(_py, list_bits);
                    }
                    return MoltObject::none().bits();
                }
                let none_bits = MoltObject::none().bits();
                let reverse_bits = MoltObject::from_int(0).bits();
                let _ = molt_list_sort_attached_body(_py, list_bits, none_bits, reverse_bits);
                if exception_pending(_py) {
                    dec_ref_bits(_py, list_bits);
                    return MoltObject::none().bits();
                }
                return list_bits;
            }
        }

        if maybe_ptr_from_bits(obj_bits).is_some() {
            unsafe {
                // dir() follows type-special lookup, including the native
                // foreign descriptor owner. Instance attributes are not hooks.
                let override_bits = match crate::builtins::attr::lookup_special_method(
                    _py,
                    obj_bits,
                    crate::NameId::DunderDir,
                )
                .into_result()
                {
                    Ok(method) => method,
                    Err(()) => return MoltObject::none().bits(),
                };

                if let Some(override_bits) = override_bits {
                    let res_bits = call_callable0(_py, override_bits);
                    dec_ref_bits(_py, override_bits);
                    if exception_pending(_py) {
                        dec_ref_bits(_py, res_bits);
                        return MoltObject::none().bits();
                    }
                    // CPython materializes and sorts a user `__dir__` result.
                    let Some(list_bits) = dir_materialize(_py, res_bits) else {
                        dec_ref_bits(_py, res_bits);
                        return MoltObject::none().bits();
                    };
                    dec_ref_bits(_py, res_bits);
                    let none_bits = MoltObject::none().bits();
                    let reverse_bits = MoltObject::from_int(0).bits();
                    let _ = molt_list_sort_attached_body(_py, list_bits, none_bits, reverse_bits);
                    if exception_pending(_py) {
                        dec_ref_bits(_py, list_bits);
                        return MoltObject::none().bits();
                    }
                    return list_bits;
                }
            }
        }

        let result = unsafe { dir_default_collect(_py, obj_bits, false) };
        if exception_pending(_py) {
            return result;
        }
        let _ = molt_list_sort_attached_body(
            _py,
            result,
            MoltObject::none().bits(),
            MoltObject::from_bool(false).bits(),
        );
        if exception_pending(_py) {
            dec_ref_bits(_py, result);
            MoltObject::none().bits()
        } else {
            result
        }
    })
}

include!("../../attached_exports_generated/object/ops_builtins/dir.rs");
