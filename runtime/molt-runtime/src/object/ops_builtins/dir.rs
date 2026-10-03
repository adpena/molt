// dir() and object.__dir__ introspection authority.
// Kept separate from call dispatch and object slot wrappers so builtin method-surface facts have one home.

use crate::*;
use molt_obj_model::MoltObject;

/// Materialize physical native iterator/sequence slots through their existing
/// linked owner. Managed results retain the runtime list constructor fast path.
unsafe fn dir_materialize(py: &PyToken<'_>, result: u64) -> Option<u64> {
    if let Some(pointer) = obj_from_bits(result).as_ptr()
        && unsafe { object_type_id(pointer) == TYPE_ID_FOREIGN }
    {
        use molt_cpython_abi::api::{abstract_sequence, refcount::OwnedPyObject};
        use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
        let native = unsafe { crate::object::foreign::foreign_ptr_from_obj(pointer) };
        let list = unsafe {
            abstract_sequence::PySequence_List(std::ptr::with_exposed_provenance_mut(native))
        };
        if list.is_null() {
            crate::cpython_abi_hooks::propagate_native_failure(py, "directory native iterable");
            return None;
        }
        let list = unsafe { OwnedPyObject::from_owned(list) };
        let result = unsafe { GLOBAL_BRIDGE.molt_value_for_pyobj(list.as_ptr()) };
        if result.is_none() {
            crate::cpython_abi_hooks::propagate_native_failure(py, "directory list projection");
        }
        result
    } else {
        unsafe { list_from_iter_bits(py, result) }
    }
}

/// Namespace discovery uses the existing optional-attribute, mapping and
/// sequence owners. This is the one default dir protocol for both representations;
/// no raw class-layout walker can bypass __dict__/__class__/__bases__ callbacks.
unsafe fn dir_default_collect(py: &PyToken<'_>, obj_bits: u64, type_directory: bool) -> u64 {
    use molt_cpython_abi::abi_types::PyObject;
    use molt_cpython_abi::api::{abstract_sequence, mapping, object, refcount::OwnedPyObject};
    use molt_cpython_abi::bridge::GLOBAL_BRIDGE;

    unsafe fn optional(
        object_ptr: *mut PyObject,
        name: &std::ffi::CStr,
    ) -> Result<Option<OwnedPyObject>, ()> {
        let mut value = std::ptr::null_mut();
        let status = unsafe {
            object::PyObject_GetOptionalAttrString(object_ptr, name.as_ptr(), &raw mut value)
        };
        match status {
            -1 => Err(()),
            0 => Ok(None),
            _ => Ok(Some(unsafe { OwnedPyObject::from_owned(value) })),
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
                let base = unsafe { OwnedPyObject::from_owned(base) };
                unsafe { merge_class(py, dictionary, base.as_ptr()) }?;
            }
        }
        Ok(())
    }

    let result = (|| -> Result<OwnedPyObject, ()> {
        unsafe {
            let source = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(obj_bits);
            if source.is_null() {
                return Err(());
            }
            let source = OwnedPyObject::from_owned(source);
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
            let dictionary = OwnedPyObject::from_owned(dictionary);
            if type_directory {
                merge_class(py, dictionary.as_ptr(), source.as_ptr())?;
            } else if let Some(class) = optional(source.as_ptr(), c"__class__")? {
                merge_class(py, dictionary.as_ptr(), class.as_ptr())?;
            }
            let result = mapping::PyDict_Keys(dictionary.as_ptr());
            if result.is_null() {
                return Err(());
            }
            let result = OwnedPyObject::from_owned(result);
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_dir_method(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe { dir_default_collect(_py, self_bits, false) }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_type_dir_method(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe { dir_default_collect(_py, self_bits, true) }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dir_builtin(obj_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let missing = missing_bits(_py);
        if obj_bits == missing {
            // CPython: dir() (no args) lists the caller's local scope.
            unsafe {
                // Note: `molt_locals_builtin` is safe to call here; `with_gil_entry` is
                // re-entrant and many runtime helpers rely on nested calls.
                let locals_bits = crate::molt_locals_builtin();
                if exception_pending(_py) {
                    if !obj_from_bits(locals_bits).is_none() {
                        molt_cpython_abi::api::errors::with_preserved_error(|| {
                            dec_ref_bits(_py, locals_bits)
                        });
                    }
                    return MoltObject::none().bits();
                }
                let list_bits = list_from_iter_bits(_py, locals_bits)
                    .unwrap_or_else(|| MoltObject::none().bits());
                if !obj_from_bits(locals_bits).is_none() {
                    molt_cpython_abi::api::errors::with_preserved_error(|| {
                        dec_ref_bits(_py, locals_bits)
                    });
                }
                if obj_from_bits(list_bits).is_none() || exception_pending(_py) {
                    if !obj_from_bits(list_bits).is_none() {
                        molt_cpython_abi::api::errors::with_preserved_error(|| {
                            dec_ref_bits(_py, list_bits)
                        });
                    }
                    return MoltObject::none().bits();
                }
                let none_bits = MoltObject::none().bits();
                let reverse_bits = MoltObject::from_int(0).bits();
                let _ = molt_list_sort(list_bits, none_bits, reverse_bits);
                if exception_pending(_py) {
                    molt_cpython_abi::api::errors::with_preserved_error(|| {
                        dec_ref_bits(_py, list_bits)
                    });
                    return MoltObject::none().bits();
                }
                return list_bits;
            }
        }

        if maybe_ptr_from_bits(obj_bits).is_some() {
            unsafe {
                // dir() follows type-special lookup, including the native
                // foreign descriptor owner. Instance attributes are not hooks.
                let override_bits =
                    crate::builtins::attr::lookup_special_method(_py, obj_bits, b"__dir__");
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }

                if let Some(override_bits) = override_bits {
                    let res_bits = call_callable0(_py, override_bits);
                    molt_cpython_abi::api::errors::with_preserved_error(|| {
                        dec_ref_bits(_py, override_bits)
                    });
                    if exception_pending(_py) {
                        molt_cpython_abi::api::errors::with_preserved_error(|| {
                            dec_ref_bits(_py, res_bits)
                        });
                        return MoltObject::none().bits();
                    }
                    // CPython materializes and sorts a user `__dir__` result.
                    let Some(list_bits) = dir_materialize(_py, res_bits) else {
                        molt_cpython_abi::api::errors::with_preserved_error(|| {
                            dec_ref_bits(_py, res_bits)
                        });
                        return MoltObject::none().bits();
                    };
                    molt_cpython_abi::api::errors::with_preserved_error(|| {
                        dec_ref_bits(_py, res_bits)
                    });
                    let none_bits = MoltObject::none().bits();
                    let reverse_bits = MoltObject::from_int(0).bits();
                    let _ = molt_list_sort(list_bits, none_bits, reverse_bits);
                    if exception_pending(_py) {
                        molt_cpython_abi::api::errors::with_preserved_error(|| {
                            dec_ref_bits(_py, list_bits)
                        });
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
        let _ = molt_list_sort(
            result,
            MoltObject::none().bits(),
            MoltObject::from_bool(false).bits(),
        );
        if exception_pending(_py) {
            molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(_py, result));
            MoltObject::none().bits()
        } else {
            result
        }
    })
}
