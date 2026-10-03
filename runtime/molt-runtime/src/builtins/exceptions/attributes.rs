//! Exception fields are ordinary descriptors in their declaring class's MRO.
//! Python __notes__ lives only in the instance dictionary. The reserved C notes
//! edge remains physical exception metadata and has no Python member descriptor.

use super::*;
use crate::builtins::types::{
    NativeDescriptorFlavor, NativeDescriptorSpec, alloc_native_descriptor,
};
use crate::object::layout;

use molt_obj_model::{ExceptionAttributeField as Field, ExceptionDescriptorKind};

fn finish_field_mutation(py: &PyToken<'_>, result: Result<(), &'static str>) -> u64 {
    match result {
        Ok(()) => MoltObject::none().bits(),
        Err(_) if exception_pending(py) => MoltObject::none().bits(),
        Err(message) => raise_exception::<u64>(py, "TypeError", message),
    }
}

unsafe fn callback_field(descriptor: u64, instance: u64) -> Option<(*mut u8, Field)> {
    unsafe {
        let descriptor = obj_from_bits(descriptor).as_ptr()?;
        if object_type_id(descriptor) != crate::TYPE_ID_NATIVE_DESCRIPTOR {
            return None;
        }
        let object = obj_from_bits(instance).as_ptr()?;
        if !matches!(
            object_type_id(object),
            TYPE_ID_EXCEPTION | crate::TYPE_ID_FOREIGN
        ) {
            return None;
        }
        // The shared descriptor boundary already admits the real receiver
        // class. Field accessors validate the immutable physical layout.
        Some((
            object,
            Field::from_operation(layout::native_descriptor_operation(descriptor)?)?,
        ))
    }
}
/// A C-owned exception keeps its physical member/getset storage. Invoke the
/// already-declared physical descriptor directly through the shared foreign
/// callback boundary; do not re-enter ordinary lookup on the semantic entry.
unsafe fn native_field_call(
    py: &PyToken<'_>,
    descriptor: u64,
    instance: u64,
    mutation: Option<Option<u64>>,
) -> u64 {
    use molt_cpython_abi::api::{errors, refcount};
    use molt_cpython_abi::bridge::{self, GLOBAL_BRIDGE};
    use molt_cpython_abi::hooks::DecodedHandleResult;
    unsafe {
        let descriptor = obj_from_bits(descriptor).as_ptr().unwrap();
        let owner = layout::native_descriptor_owner_bits(descriptor);
        let name = crate::object::ops_format::string_obj_bytes(obj_from_bits(
            layout::native_descriptor_name_bits(descriptor),
        ))
        .expect("native descriptor name is sealed as a string");
        let c_owner = GLOBAL_BRIDGE
            .handle_to_borrowed_pyobj(owner)
            .cast::<molt_cpython_abi::abi_types::PyTypeObject>();
        if c_owner.is_null() {
            crate::cpython_abi_hooks::propagate_native_failure(
                py,
                "exception descriptor owner projection",
            );
            return MoltObject::none().bits();
        }
        let physical = errors::native_exception_field_descriptor(c_owner, &name);
        if physical.is_null() {
            crate::cpython_abi_hooks::propagate_native_failure(
                py,
                "exception physical descriptor construction",
            );
            return MoltObject::none().bits();
        }
        let result = match mutation {
            None => bridge::molt_foreign_descriptor_get(
                physical.addr(),
                Some(instance),
                Some(type_of_bits(py, instance)),
            ),
            Some(value) => bridge::molt_foreign_descriptor_set(physical.addr(), instance, value),
        };
        errors::with_preserved_error(|| refcount::Py_DECREF(physical));
        match result.decode() {
            DecodedHandleResult::Ok(bits) => bits,
            DecodedHandleResult::Error => {
                crate::cpython_abi_hooks::propagate_native_failure(
                    py,
                    "native exception field access",
                );
                MoltObject::none().bits()
            }
            DecodedHandleResult::Missing => raise_exception::<u64>(
                py,
                "SystemError",
                "native exception field has no descriptor operation",
            ),
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_member_get(descriptor: u64, instance: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let Some((object, field)) = callback_field(descriptor, instance) else {
                return raise_exception::<u64>(
                    py,
                    "TypeError",
                    "invalid exception member receiver",
                );
            };
            if object_type_id(object) == crate::TYPE_ID_FOREIGN {
                return native_field_call(py, descriptor, instance, None);
            }
            let bits = match field {
                Field::Dictionary => {
                    let Some(bits) = crate::object::field_storage::materialize(py, object) else {
                        return MoltObject::none().bits();
                    };
                    bits
                }
                Field::Args => {
                    let Some(bits) = exception_materialized_args_bits(py, object) else {
                        return MoltObject::none().bits();
                    };
                    bits
                }
                Field::Traceback => crate::exception_materialize_traceback_bits(py, object),
                Field::Context => exception_context_bits(object),
                Field::Cause => exception_cause_bits(object),
                Field::SuppressContext => exception_suppress_bits(object),
                Field::Typed(field) => {
                    return match exception_typed_field_get(py, object, field) {
                        Some(Ok(bits)) => bits,
                        Some(Err(message)) => raise_exception::<u64>(py, "AttributeError", message),
                        None => raise_exception::<u64>(
                            py,
                            "SystemError",
                            "exception field is absent from its layout",
                        ),
                    };
                }
            };
            if exception_pending(py) {
                return MoltObject::none().bits();
            }
            inc_ref_bits(py, bits);
            bits
        }
    })
}

unsafe fn mutate_field(
    py: &PyToken<'_>,
    descriptor: u64,
    instance: u64,
    value: Option<u64>,
) -> u64 {
    unsafe {
        let Some((object, field)) = callback_field(descriptor, instance) else {
            return raise_exception::<u64>(py, "TypeError", "invalid exception member receiver");
        };
        if object_type_id(object) == crate::TYPE_ID_FOREIGN {
            return native_field_call(py, descriptor, instance, Some(value));
        }
        if let Field::Typed(field) = field {
            let result = match value {
                Some(value) => exception_typed_field_replace(py, instance, field, value),
                None => exception_typed_field_delete(py, instance, field),
            };
            return match result {
                Some(Ok(())) => MoltObject::none().bits(),
                Some(Err(_)) if exception_pending(py) => MoltObject::none().bits(),
                Some(Err(message)) => raise_exception::<u64>(py, "AttributeError", message),
                None => raise_exception::<u64>(
                    py,
                    "SystemError",
                    "exception field is absent from its layout",
                ),
            };
        }
        let Some(value) = value else {
            let message = match field {
                Field::Dictionary => "cannot delete __dict__",
                Field::Args => "args may not be deleted",
                Field::Traceback => "__traceback__ may not be deleted",
                Field::Context => "__context__ may not be deleted",
                Field::Cause => "__cause__ may not be deleted",
                Field::SuppressContext => "can't delete numeric/char attribute",
                Field::Typed(_) => unreachable!(),
            };
            return raise_exception::<u64>(py, "TypeError", message);
        };
        let slot = match field {
            Field::Dictionary => {
                crate::object::field_storage::replace_dictionary(py, object, Some(value));
                return MoltObject::none().bits();
            }
            Field::Args => {
                let Some(args) = tuple_from_iter_bits(py, value) else {
                    return MoltObject::none().bits();
                };
                let result =
                    exception_replace_field_bits(py, instance, ExceptionFieldSlot::Args, args);
                dec_ref_bits(py, args);
                return finish_field_mutation(py, result);
            }
            Field::Traceback => ExceptionFieldSlot::Traceback,
            Field::Context => ExceptionFieldSlot::Context,
            Field::Cause => ExceptionFieldSlot::Cause,
            Field::SuppressContext => {
                let Some(suppress) = obj_from_bits(value).as_bool() else {
                    return raise_exception::<u64>(
                        py,
                        "TypeError",
                        "attribute value type must be bool",
                    );
                };
                return finish_field_mutation(
                    py,
                    exception_replace_suppress_context(py, instance, suppress),
                );
            }
            Field::Typed(_) => unreachable!(),
        };
        finish_field_mutation(py, exception_replace_field_bits(py, instance, slot, value))
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_member_set(descriptor: u64, instance: u64, value: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        unsafe { mutate_field(py, descriptor, instance, Some(value)) }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_member_delete(descriptor: u64, instance: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        unsafe { mutate_field(py, descriptor, instance, None) }
    })
}

pub(crate) fn publish_exception_fields(py: &PyToken<'_>, owner: u64) -> bool {
    unsafe {
        let Some(class) = obj_from_bits(owner).as_ptr() else {
            return false;
        };
        let root = crate::object::class_exception_layout_root(class);
        if owner != exception_type_bits_from_name(py, root.owner_name()) {
            return true;
        }
        let declarations: Vec<_> = root.attribute_declarations().collect();
        if declarations.is_empty() {
            return true;
        }
        let mut callbacks = Vec::new();
        let mut callback_owners = Vec::new();
        for (symbol, arity) in [
            (fn_addr!(molt_exception_member_get), 2),
            (fn_addr!(molt_exception_member_set), 3),
            (fn_addr!(molt_exception_member_delete), 2),
        ] {
            let callback = crate::alloc_function_obj(py, symbol, arity);
            if callback.is_null() {
                return false;
            }
            callback_owners.push(crate::PtrDropGuard::new(callback));
            if !crate::builtins::functions::native_callable::configure_native_callable(
                py,
                callback,
                NativeCallableSpec::uncached_function(),
            ) {
                return false;
            }
            callbacks.push(MoltObject::from_ptr(callback).bits());
        }
        let Some(dictionary) = obj_from_bits(class_dict_bits(class)).as_ptr() else {
            return false;
        };
        for declaration in declarations {
            let operation = declaration.field.operation();
            let name = declaration.python_name;
            let flavor = match declaration.field.descriptor_kind() {
                ExceptionDescriptorKind::Member => NativeDescriptorFlavor::Member,
                ExceptionDescriptorKind::GetSet => NativeDescriptorFlavor::GetSet,
            };
            let Some(name) = attr_name_bits_from_bytes(py, name.as_bytes()) else {
                return false;
            };
            let _name_owner = crate::PtrDropGuard::new(obj_from_bits(name).as_ptr().unwrap());
            if dict_get_in_place(py, dictionary, name).is_some() {
                continue;
            }
            if exception_pending(py) {
                return false;
            }
            let descriptor = alloc_native_descriptor(
                py,
                NativeDescriptorSpec {
                    flavor,
                    operation,
                    owner,
                    name,
                    doc: MoltObject::none().bits(),
                    getter: callbacks[0],
                    setter: callbacks[1],
                    deleter: callbacks[2],
                },
            );
            if exception_pending(py) || obj_from_bits(descriptor).as_ptr().is_none() {
                dec_ref_bits(py, descriptor);
                return false;
            }
            dict_set_in_place(py, dictionary, name, descriptor);
            dec_ref_bits(py, descriptor);
            if exception_pending(py) {
                return false;
            }
            crate::class_bump_layout_version(class);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use molt_cpython_abi::abi_types::{self, PyAttributeErrorObject};
    use molt_cpython_abi::api::{errors, refcount, sequences};
    use molt_cpython_abi::bridge::GLOBAL_BRIDGE;

    #[test]
    fn declared_exception_fields_publish_once_and_accept_real_native_storage() {
        let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
        crate::with_gil(|py| unsafe {
            // Class construction owns publication before reflection or a C
            // view. Ordinary lookup only consumes that completed namespace.
            let base = builtin_classes(&py).base_exception;
            let base_ptr = obj_from_bits(base).as_ptr().unwrap();
            let args_name = attr_name_bits_from_bytes(&py, b"args").unwrap();
            let cause_name = attr_name_bits_from_bytes(&py, b"__cause__").unwrap();
            let namespace = obj_from_bits(class_dict_bits(base_ptr)).as_ptr().unwrap();
            let previous_cause = dict_get_in_place(&py, namespace, cause_name);
            assert!(previous_cause.is_some());
            assert!(crate::object::class_storage::class_declares(
                base_ptr,
                crate::object::class_storage::ClassDeclaration::NativeNamespacePublished,
            ));
            let args_descriptor =
                crate::builtins::attr::class_namespace_lookup_raw(&py, base_ptr, args_name)
                    .expect("ordinary lookup sees the declared field immediately");
            assert_eq!(
                object_type_id(obj_from_bits(args_descriptor).as_ptr().unwrap()),
                crate::TYPE_ID_NATIVE_DESCRIPTOR
            );
            assert_eq!(
                dict_get_in_place(&py, namespace, cause_name),
                previous_cause
            );
            inc_ref_bits(&py, args_descriptor);

            molt_cpython_abi::bridge::molt_cpython_abi_init();
            let c_args = sequences::PyTuple_New(0);
            assert!(!c_args.is_null());
            let native = errors::molt_native_exception_new(
                &raw mut abi_types::PyExc_AttributeError,
                c_args,
                std::ptr::null_mut(),
            );
            refcount::Py_DECREF(c_args);
            assert!(!native.is_null());
            let receiver = GLOBAL_BRIDGE.molt_value_for_pyobj(native).unwrap();
            assert_eq!(
                object_type_id(obj_from_bits(receiver).as_ptr().unwrap()),
                crate::TYPE_ID_FOREIGN
            );

            // Runtime fields may own real native exceptions, and native
            // fields may own runtime views. Both projection directions retain
            // the same identity without interpreting foreign payload words.
            let managed = alloc_exception(&py, "ValueError", "mixed chain");
            assert!(!managed.is_null());
            let managed_bits = MoltObject::from_ptr(managed).bits();
            let context_name = attr_name_bits_from_bytes(&py, b"__context__").unwrap();
            for name in [cause_name, context_name] {
                let result = crate::molt_set_attr_name(managed_bits, name, receiver);
                dec_ref_bits(&py, result);
                assert!(!exception_pending(&py));
                let observed = crate::molt_get_attr_name(managed_bits, name);
                assert_eq!(observed, receiver);
                dec_ref_bits(&py, observed);
            }
            let managed_view = GLOBAL_BRIDGE
                .handle_to_borrowed_pyobj(managed_bits)
                .cast::<abi_types::PyBaseExceptionObject>();
            assert!(!managed_view.is_null());
            assert_eq!((*managed_view).cause, native);
            assert_eq!((*managed_view).context, native);
            assert!(GLOBAL_BRIDGE.commit_exception_view(managed_bits));
            for name in [cause_name, context_name] {
                let result =
                    crate::molt_set_attr_name(managed_bits, name, MoltObject::none().bits());
                dec_ref_bits(&py, result);
            }
            refcount::Py_INCREF(managed_view.cast());
            errors::PyException_SetCause(native, managed_view.cast());
            refcount::Py_INCREF(managed_view.cast());
            errors::PyException_SetContext(native, managed_view.cast());
            assert!(errors::PyErr_Occurred().is_null());
            for name in [cause_name, context_name] {
                let observed = crate::molt_get_attr_name(receiver, name);
                assert_eq!(observed, managed_bits);
                dec_ref_bits(&py, observed);
            }
            errors::PyException_SetCause(native, std::ptr::null_mut());
            errors::PyException_SetContext(native, std::ptr::null_mut());
            dec_ref_bits(&py, context_name);
            dec_ref_bits(&py, managed_bits);
            assert!(!exception_pending(&py));
            assert!(errors::PyErr_Occurred().is_null());

            let tuple = alloc_tuple(&py, &[MoltObject::from_int(1702).bits()]);
            assert!(!tuple.is_null());
            let tuple_bits = MoltObject::from_ptr(tuple).bits();
            let tuple_view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(tuple_bits);
            let result = crate::builtins::types::native_descriptor_mutate(
                &py,
                args_descriptor,
                receiver,
                Some(tuple_bits),
            );
            assert!(!exception_pending(&py));
            assert!(errors::PyErr_Occurred().is_null());
            assert_eq!(
                (*native.cast::<PyAttributeErrorObject>()).base.args,
                tuple_view
            );
            dec_ref_bits(&py, result);
            let observed = crate::builtins::types::native_descriptor_get(
                &py,
                args_descriptor,
                Some(receiver),
                Some(base),
            );
            assert_eq!(observed, tuple_bits);
            dec_ref_bits(&py, observed);

            let typed_owner = exception_type_bits_from_name(&py, "AttributeError");
            let typed_name = attr_name_bits_from_bytes(&py, b"obj").unwrap();
            let typed_descriptor = crate::builtins::attr::class_namespace_lookup_raw(
                &py,
                obj_from_bits(typed_owner).as_ptr().unwrap(),
                typed_name,
            )
            .unwrap();
            inc_ref_bits(&py, typed_descriptor);
            let result = crate::builtins::types::native_descriptor_mutate(
                &py,
                typed_descriptor,
                receiver,
                Some(tuple_bits),
            );
            assert_eq!((*native.cast::<PyAttributeErrorObject>()).obj, tuple_view);
            assert!(!exception_pending(&py));
            dec_ref_bits(&py, result);
            let observed = crate::builtins::types::native_descriptor_get(
                &py,
                typed_descriptor,
                Some(receiver),
                Some(typed_owner),
            );
            assert_eq!(observed, tuple_bits);
            dec_ref_bits(&py, observed);
            let result = crate::builtins::types::native_descriptor_mutate(
                &py,
                typed_descriptor,
                receiver,
                None,
            );
            assert!((*native.cast::<PyAttributeErrorObject>()).obj.is_null());
            assert!(!exception_pending(&py));
            dec_ref_bits(&py, result);
            for bits in [
                typed_descriptor,
                typed_name,
                tuple_bits,
                receiver,
                args_descriptor,
                args_name,
                cause_name,
            ] {
                dec_ref_bits(&py, bits);
            }
            refcount::Py_DECREF(native);
            assert!(!exception_pending(&py));
            assert!(errors::PyErr_Occurred().is_null());
        });
    }
}
