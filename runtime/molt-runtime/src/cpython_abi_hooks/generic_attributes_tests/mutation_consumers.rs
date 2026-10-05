//! Independent call-count assertions at the normal, explicit and raw boundaries.
use super::*;
use molt_cpython_abi::abi_types::{Py_TPFLAGS_READY, PyBaseObject_Type, PyType_Type, PyTypeObject};
use molt_cpython_abi::api::typeobj;

#[repr(C)]
struct NativeMutationReceiver {
    object: PyObject,
    dictionary: *mut PyObject,
    calls: usize,
    fail: bool,
}

unsafe extern "C" fn native_mutation(
    receiver: *mut PyObject,
    name: *mut PyObject,
    value: *mut PyObject,
) -> std::os::raw::c_int {
    let receiver = unsafe { &mut *receiver.cast::<NativeMutationReceiver>() };
    receiver.calls += 1;
    if receiver.fail {
        unsafe {
            errors::PyErr_SetString(
                (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast(),
                c"native mutation sentinel".as_ptr(),
            );
        }
        -1
    } else {
        unsafe { object::PyObject_GenericSetAttr(&raw mut receiver.object, name, value) }
    }
}

unsafe extern "C" fn native_lookup(receiver: *mut PyObject, name: *mut PyObject) -> *mut PyObject {
    unsafe {
        let bytes = strings::PyUnicode_AsUTF8(name);
        if !bytes.is_null() && std::ffi::CStr::from_ptr(bytes).to_bytes() == b"__dict__" {
            let dictionary = (*receiver.cast::<NativeMutationReceiver>()).dictionary;
            refcount::Py_INCREF(dictionary);
            return dictionary;
        }
        object::PyObject_GenericGetAttr(receiver, name)
    }
}

#[test]
fn native_normal_mutation_consumers_invoke_override_and_preserve_failures() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let mut class: PyTypeObject = std::mem::zeroed();
            class.ob_base.ob_base.ob_refcnt = 1;
            class.ob_base.ob_base.ob_type = &raw mut PyType_Type;
            class.tp_base = &raw mut PyBaseObject_Type;
            class.tp_name = c"NativeMutationConsumer".as_ptr();
            class.tp_flags = Py_TPFLAGS_READY;
            class.tp_basicsize = std::mem::size_of::<NativeMutationReceiver>() as isize;
            class.tp_dictoffset = std::mem::offset_of!(NativeMutationReceiver, dictionary) as isize;
            class.tp_setattro = Some(native_mutation);
            class.tp_getattro = Some(native_lookup);
            class.tp_dict = mapping::PyDict_New();
            class.tp_mro = sequences::PyTuple_New(2);
            for (index, base) in [(&raw mut class).cast(), (&raw mut PyBaseObject_Type).cast()]
                .into_iter()
                .enumerate()
            {
                refcount::Py_INCREF(base);
                assert_eq!(
                    sequences::PyTuple_SetItem(class.tp_mro, index as isize, base),
                    0
                );
            }
            let mut receiver = NativeMutationReceiver {
                object: PyObject {
                    ob_refcnt: 1,
                    ob_type: &raw mut class,
                },
                dictionary: mapping::PyDict_New(),
                calls: 0,
                fail: false,
            };
            let bits = GLOBAL_BRIDGE
                .molt_value_for_pyobj(&raw mut receiver.object)
                .unwrap();
            let name = crate::attr_name_bits_from_bytes(py, b"field").unwrap();
            let value = MoltObject::from_int(19).bits();
            assert_eq!(
                crate::molt_setattr_builtin(bits, name, value),
                MoltObject::none().bits()
            );
            assert_eq!(receiver.calls, 1);
            crate::molt_delattr_builtin(bits, name);
            assert_eq!(receiver.calls, 2);
            assert_eq!(crate::c_api::PyObject_SetAttr(bits, name, value), 0);
            assert_eq!(receiver.calls, 3);
            assert_eq!(crate::c_api::PyObject_DelAttr(bits, name), 0);
            assert_eq!(receiver.calls, 4);
            assert!(!crate::exception_pending(py));

            let state = crate::molt_dict_new(0);
            crate::dict_set_in_place(py, obj_from_bits(state).as_ptr().unwrap(), name, value);
            // Dict BUILD does not call the setter; slot BUILD must call it.
            assert_eq!(
                crate::builtins::functions_pickle::pickle_apply_build(py, bits, state),
                Ok(bits)
            );
            assert_eq!(receiver.calls, 4);
            let slot_state =
                MoltObject::from_ptr(crate::alloc_tuple(py, &[MoltObject::none().bits(), state]))
                    .bits();
            assert_eq!(
                crate::builtins::functions_pickle::pickle_apply_build(py, bits, slot_state),
                Ok(bits)
            );
            assert_eq!(receiver.calls, 5);

            // Assigned members and the final __wrapped__ store both use setattr.
            let source = attribute_class(py, crate::builtin_classes(py).object, false);
            member(py, source, b"field", value);
            let assigned = MoltObject::from_ptr(crate::alloc_tuple(py, &[name])).bits();
            let empty = MoltObject::from_ptr(crate::alloc_tuple(py, &[])).bits();
            let result = crate::builtins::functools::molt_functools_update_wrapper(
                bits, source, assigned, empty,
            );
            assert_eq!(result, bits);
            dec_ref_bits(py, result);
            assert_eq!(receiver.calls, 7);
            assert!(!crate::exception_pending(py));

            // Rejection is prior to name validation, and never calls native code.
            for delete in [false, true] {
                let invalid_name = MoltObject::from_int(5).bits();
                if delete {
                    crate::molt_object_delattr(bits, invalid_name);
                } else {
                    crate::molt_object_setattr(bits, invalid_name, value);
                }
                assert_native_setter_rejection(delete, "NativeMutationConsumer");
            }
            assert_eq!(receiver.calls, 7);

            receiver.fail = true;
            for consumer in 0..6 {
                let before = receiver.calls;
                match consumer {
                    0 => {
                        crate::molt_setattr_builtin(bits, name, value);
                    }
                    1 => {
                        assert_eq!(crate::c_api::PyObject_SetAttr(bits, name, value), -1);
                    }
                    2 => {
                        assert_eq!(crate::c_api::PyObject_DelAttr(bits, name), -1);
                    }
                    3 => {
                        assert!(
                            crate::builtins::functions_pickle::pickle_apply_build(
                                py, bits, slot_state
                            )
                            .is_err()
                        );
                    }
                    4 => {
                        crate::builtins::functools::molt_functools_update_wrapper(
                            bits, source, assigned, empty,
                        );
                    }
                    _ => {
                        crate::builtins::functools::molt_functools_update_wrapper(
                            bits, source, empty, empty,
                        );
                    }
                }
                assert_eq!(
                    receiver.calls,
                    before + 1,
                    "consumer must not retry or suppress an override failure"
                );
                let raised = errors::PyErr_GetRaisedException();
                assert!(!raised.is_null());
                assert_ne!(
                    errors::PyErr_GivenExceptionMatches(
                        raised,
                        (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast()
                    ),
                    0
                );
                let message = typeobj::PyObject_Str(raised);
                assert_eq!(
                    std::ffi::CStr::from_ptr(strings::PyUnicode_AsUTF8(message)).to_bytes(),
                    b"native mutation sentinel"
                );
                refcount::Py_DECREF(message);
                refcount::Py_DECREF(raised);
            }
            for value in [empty, assigned, source, slot_state, state, name, bits] {
                dec_ref_bits(py, value);
            }
            let mro = std::mem::replace(&mut class.tp_mro, ptr::null_mut());
            for value in [mro, class.tp_dict, receiver.dictionary] {
                refcount::Py_DECREF(value);
            }
            assert_eq!(receiver.object.ob_refcnt, 1);
            assert_eq!(class.ob_base.ob_base.ob_refcnt, 1);
            assert!(!crate::exception_pending(py));
        }
    });
}

#[test]
fn managed_class_mutation_separates_normal_explicit_and_raw_generic() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            for custom_metaclass in [false, true] {
                CALLS.with(|calls| calls.set([0; 6]));
                FAILURE.with(|failure| failure.set(0));
                let classes = crate::builtin_classes(py);
                let meta = if custom_metaclass {
                    attribute_class(py, classes.type_obj, false)
                } else {
                    classes.type_obj
                };
                let name = crate::attr_name_bits_from_bytes(py, b"MutableClass").unwrap();
                let bases = MoltObject::from_ptr(crate::alloc_tuple(py, &[])).bits();
                let namespace = crate::molt_dict_new(0);
                let class = crate::builtins::types::molt_type_new(
                    meta,
                    name,
                    bases,
                    namespace,
                    MoltObject::none().bits(),
                );
                assert!(!crate::exception_pending(py));
                let field = crate::attr_name_bits_from_bytes(py, b"field").unwrap();
                let view = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(class);
                let name_view = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(field);
                let value =
                    GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(MoltObject::from_float(0.0).bits());
                crate::molt_setattr_builtin(class, field, MoltObject::from_int(5).bits());
                crate::molt_delattr_builtin(class, field);
                assert!(!crate::exception_pending(py));
                assert_eq!(
                    &CALLS.with(Cell::get)[1..3],
                    if custom_metaclass { &[1, 1] } else { &[0, 0] }
                );
                for delete in [false, true] {
                    if delete {
                        crate::molt_object_delattr(class, field);
                    } else {
                        crate::molt_object_setattr(class, field, MoltObject::none().bits());
                    }
                    assert_native_setter_rejection(
                        delete,
                        if custom_metaclass {
                            "ManagedGenericAttributes"
                        } else {
                            "type"
                        },
                    );
                }
                let class_ptr = obj_from_bits(class).as_ptr().unwrap();
                let version = crate::class_layout_version_bits(class_ptr);
                assert_eq!(object::PyObject_GenericSetAttr(view, name_view, value), 0);
                let raw = object::PyObject_GenericGetAttr(view, name_view);
                assert!(!raw.is_null());
                assert_eq!(
                    GLOBAL_BRIDGE
                        .observed_handle_for_pyobj(raw)
                        .map(|value| value.bits()),
                    Some(0)
                );
                refcount::Py_DECREF(raw);
                let dictionary = obj_from_bits(crate::class_dict_bits(class_ptr))
                    .as_ptr()
                    .unwrap();
                assert_eq!(crate::dict_get_in_place(py, dictionary, field), Some(0));
                assert_eq!(
                    object::PyObject_GenericSetAttr(view, name_view, ptr::null_mut()),
                    0
                );
                assert_eq!(crate::dict_get_in_place(py, dictionary, field), None);
                // Explicit type defaults remain valid for metaclass delegation
                // and bypass that metaclass's Python mutation overrides.
                let setter = crate::builtins::methods::type_method_bits(py, "__setattr__").unwrap();
                let deleter =
                    crate::builtins::methods::type_method_bits(py, "__delattr__").unwrap();
                inc_ref_bits(py, setter);
                inc_ref_bits(py, deleter);
                let result =
                    crate::call_callable3(py, setter, class, field, MoltObject::from_int(6).bits());
                dec_ref_bits(py, result);
                assert!(!crate::exception_pending(py));
                assert_eq!(
                    crate::dict_get_in_place(py, dictionary, field),
                    Some(MoltObject::from_int(6).bits())
                );
                assert_ne!(crate::class_layout_version_bits(class_ptr), version);
                let result = crate::call_callable2(py, deleter, class, field);
                dec_ref_bits(py, result);
                dec_ref_bits(py, setter);
                dec_ref_bits(py, deleter);
                assert!(!crate::exception_pending(py));
                assert_eq!(crate::dict_get_in_place(py, dictionary, field), None);
                assert_eq!(
                    &CALLS.with(Cell::get)[1..3],
                    if custom_metaclass { &[1, 1] } else { &[0, 0] }
                );
                for pointer in [value, name_view, view] {
                    refcount::Py_DECREF(pointer);
                }
                for bits in [field, class, namespace, bases, name] {
                    dec_ref_bits(py, bits);
                }
                if custom_metaclass {
                    dec_ref_bits(py, meta);
                }
                assert!(!crate::exception_pending(py));
            }
        }
    });
}

#[test]
fn raw_static_class_mutation_is_visible_only_through_generic_dictionary() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let classes = crate::builtin_classes(py);
            let name = crate::attr_name_bits_from_bytes(py, b"_molt_raw_class_oracle").unwrap();
            let name_view = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(name);
            let value =
                GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(MoltObject::from_int(137).bits());
            for class in [classes.int, classes.bool, classes.object, classes.type_obj] {
                let class_ptr = obj_from_bits(class).as_ptr().unwrap();
                let view = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(class);
                let dictionary = obj_from_bits(crate::class_dict_bits(class_ptr))
                    .as_ptr()
                    .unwrap();
                assert_eq!(object::PyObject_GenericSetAttr(view, name_view, value), 0);
                assert_eq!(crate::dict_get_in_place(py, dictionary, name), None);
                let raw = object::PyObject_GenericGetAttr(view, name_view);
                assert!(!raw.is_null());
                assert_eq!(
                    GLOBAL_BRIDGE
                        .observed_handle_for_pyobj(raw)
                        .map(|value| value.bits()),
                    Some(MoltObject::from_int(137).bits())
                );
                refcount::Py_DECREF(raw);
                assert!(object::PyObject_GetAttr(view, name_view).is_null());
                assert_ne!(
                    errors::PyErr_ExceptionMatches(
                        (&raw mut molt_cpython_abi::abi_types::PyExc_AttributeError).cast()
                    ),
                    0
                );
                errors::PyErr_Clear();
                assert_eq!(
                    object::PyObject_GenericSetAttr(view, name_view, ptr::null_mut()),
                    0
                );
                assert!(object::PyObject_GenericGetAttr(view, name_view).is_null());
                assert_ne!(
                    errors::PyErr_ExceptionMatches(
                        (&raw mut molt_cpython_abi::abi_types::PyExc_AttributeError).cast()
                    ),
                    0
                );
                errors::PyErr_Clear();
                assert_eq!(crate::dict_get_in_place(py, dictionary, name), None);
                assert_eq!(object::PyObject_SetAttr(view, name_view, value), -1);
                assert_ne!(
                    errors::PyErr_ExceptionMatches(
                        (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast()
                    ),
                    0
                );
                errors::PyErr_Clear();
                refcount::Py_DECREF(view);
            }
            refcount::Py_DECREF(value);
            refcount::Py_DECREF(name_view);
            dec_ref_bits(py, name);
            assert!(!crate::exception_pending(py));
        }
    });
}

#[test]
fn explicit_object_mutation_admits_builtin_storage_and_managed_subclasses() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let classes = crate::builtin_classes(py);
        let name = crate::attr_name_bits_from_bytes(py, b"managed").unwrap();
        for base in [classes.object, classes.list, classes.tuple] {
            CALLS.with(|calls| calls.set([0; 6]));
            FAILURE.with(|failure| failure.set(0));
            let class = attribute_class(py, base, false);
            // The finished fixture class is owned and the runtime token is held.
            let receiver =
                unsafe { crate::call::bind::call_bind_borrowed(py, class, None, &[], &[], &[]) };
            crate::molt_object_setattr(receiver, name, MoltObject::from_int(77).bits());
            crate::molt_object_delattr(receiver, name);
            assert!(!crate::exception_pending(py));
            assert_eq!(CALLS.with(Cell::get), [0, 0, 0, 1, 1, 0]);
            for value in [receiver, class] {
                dec_ref_bits(py, value);
            }
        }
        for receiver in [
            MoltObject::from_int(4).bits(),
            MoltObject::from_float(0.0).bits(),
            MoltObject::none().bits(),
        ] {
            for delete in [false, true] {
                if delete {
                    crate::molt_object_delattr(receiver, name);
                } else {
                    crate::molt_object_setattr(receiver, name, receiver);
                }
                let error = crate::exception_last_bits_noinc(py).unwrap();
                assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                    py,
                    error,
                    "AttributeError"
                ));
                crate::clear_exception(py);
            }
        }
        dec_ref_bits(py, name);
    });
}

#[test]
fn explicit_object_defaults_mutate_managed_storage_without_projection() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            FAILURE.with(|failure| failure.set(0));
            for base in [
                crate::builtin_classes(py).object,
                crate::builtin_classes(py).list,
            ] {
                let class = attribute_class(py, base, false);
                let receiver = snapshot_receiver(py, class, false);
                let pointer = obj_from_bits(receiver).as_ptr().unwrap();
                let field =
                    crate::attr_name_bits_from_bytes(py, b"unprojected_object_field").unwrap();
                CALLS.with(|calls| calls.set([0; 6]));
                assert!(
                    !(*crate::header_from_obj_ptr(pointer))
                        .has_flag(crate::object::HEADER_FLAG_HAS_ABI_VIEW)
                );
                assert!(!GLOBAL_BRIDGE.type_has_projection(class));
                crate::molt_object_setattr(receiver, field, MoltObject::from_int(47).bits());
                assert!(!crate::exception_pending(py));
                let dictionary = crate::object::field_storage::current_dictionary(py, pointer)
                    .unwrap()
                    .unwrap();
                let dictionary = obj_from_bits(dictionary).as_ptr().unwrap();
                assert_eq!(
                    crate::dict_get_in_place(py, dictionary, field),
                    Some(MoltObject::from_int(47).bits())
                );
                crate::molt_object_delattr(receiver, field);
                assert_eq!(crate::dict_get_in_place(py, dictionary, field), None);
                assert!(!crate::exception_pending(py));
                assert_eq!(&CALLS.with(Cell::get)[1..3], &[0, 0]);
                assert!(
                    !(*crate::header_from_obj_ptr(pointer))
                        .has_flag(crate::object::HEADER_FLAG_HAS_ABI_VIEW)
                );
                assert!(!GLOBAL_BRIDGE.type_has_projection(class));
                for bits in [field, receiver, class] {
                    dec_ref_bits(py, bits);
                }
            }
        }
    });
}

#[test]
fn metaclass_default_pairs_select_raw_type_or_explicit_admission() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            for (object_set, object_delete) in
                [(true, true), (false, false), (true, false), (false, true)]
            {
                let name = crate::attr_name_bits_from_bytes(py, b"ObjectDefaultMeta").unwrap();
                let meta = crate::molt_class_new(name);
                crate::molt_class_set_base(meta, crate::builtin_classes(py).type_obj);
                let setter = if object_set {
                    crate::builtins::methods::object_method_bits(py, "__setattr__")
                } else {
                    crate::builtins::methods::type_method_bits(py, "__setattr__")
                }
                .unwrap();
                let deleter = if object_delete {
                    crate::builtins::methods::object_method_bits(py, "__delattr__")
                } else {
                    crate::builtins::methods::type_method_bits(py, "__delattr__")
                }
                .unwrap();
                member(py, meta, b"__setattr__", setter);
                member(py, meta, b"__delattr__", deleter);
                crate::object::class_finish_definition(py, obj_from_bits(meta).as_ptr().unwrap())
                    .unwrap();
                let bases = MoltObject::from_ptr(crate::alloc_tuple(py, &[])).bits();
                let namespace = crate::molt_dict_new(0);
                let class = crate::builtins::types::molt_type_new(
                    meta,
                    name,
                    bases,
                    namespace,
                    MoltObject::none().bits(),
                );
                assert!(!crate::exception_pending(py));
                let class_ptr = obj_from_bits(class).as_ptr().unwrap();
                let dictionary = obj_from_bits(crate::class_dict_bits(class_ptr))
                    .as_ptr()
                    .unwrap();
                for delete in [false, true] {
                    crate::builtins::attributes::generic_set_attr_name(
                        class,
                        name,
                        MoltObject::none().bits(),
                    );
                    let version = crate::class_layout_version_bits(class_ptr);
                    if delete {
                        crate::molt_del_attr_name(class, name);
                    } else {
                        crate::molt_set_attr_name(class, name, MoltObject::from_int(41).bits());
                    }
                    let rejects = object_set != object_delete
                        && if delete { object_delete } else { object_set };
                    if rejects {
                        assert_native_setter_rejection(delete, "ObjectDefaultMeta");
                        assert_eq!(crate::class_layout_version_bits(class_ptr), version);
                    } else {
                        assert!(!crate::exception_pending(py));
                        assert_eq!(
                            crate::dict_get_in_place(py, dictionary, name),
                            if delete {
                                None
                            } else {
                                Some(MoltObject::from_int(41).bits())
                            }
                        );
                        if object_set && object_delete {
                            assert_eq!(crate::class_layout_version_bits(class_ptr), version);
                        } else {
                            assert_ne!(crate::class_layout_version_bits(class_ptr), version);
                        }
                    }
                }
                assert!(!GLOBAL_BRIDGE.type_has_projection(class));
                for bits in [class, namespace, bases, meta, name] {
                    dec_ref_bits(py, bits);
                }
            }
        }
    });
}

#[test]
fn raw_managed_class_dictionary_retires_displaced_ownership() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let class = snapshot_class(py, b"RawOwnedClass");
            let name = crate::attr_name_bits_from_bytes(py, b"raw_owned_value").unwrap();
            let value = crate::molt_dict_new(0);
            let value_ptr = obj_from_bits(value).as_ptr().unwrap();
            let before = (*crate::header_from_obj_ptr(value_ptr)).ref_count_snapshot();
            crate::builtins::attributes::generic_set_attr_name(class, name, value);
            assert!(!crate::exception_pending(py));
            assert_eq!(
                (*crate::header_from_obj_ptr(value_ptr)).ref_count_snapshot(),
                before + 1
            );
            let class_ptr = obj_from_bits(class).as_ptr().unwrap();
            let namespace = obj_from_bits(crate::class_dict_bits(class_ptr))
                .as_ptr()
                .unwrap();
            assert_eq!(crate::dict_get_in_place(py, namespace, name), Some(value));
            crate::builtins::attributes::generic_set_attr_name(
                class,
                name,
                MoltObject::from_int(29).bits(),
            );
            assert_eq!(
                (*crate::header_from_obj_ptr(value_ptr)).ref_count_snapshot(),
                before
            );
            assert_eq!(
                crate::dict_get_in_place(py, namespace, name),
                Some(MoltObject::from_int(29).bits())
            );
            crate::builtins::attributes::generic_del_attr_name(class, name);
            assert_eq!(crate::dict_get_in_place(py, namespace, name), None);
            assert!(!crate::exception_pending(py));
            for bits in [value, name, class] {
                dec_ref_bits(py, bits);
            }
        }
    });
}

/// Parent-only serial profile. Each family reports elapsed time and asserts
/// the actual namespace result plus absence of newly manufactured ABI views.
#[test]
#[ignore = "bounded parent-owned mutation projection profile"]
fn managed_mutation_projection_profile() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let class = snapshot_class(py, b"MutationProjectionProfile");
            let receiver = snapshot_receiver(py, class, false);
            let pointer = obj_from_bits(receiver).as_ptr().unwrap();
            let name = crate::attr_name_bits_from_bytes(py, b"profile_field").unwrap();
            let setter = crate::builtins::methods::type_method_bits(py, "__setattr__").unwrap();
            let deleter = crate::builtins::methods::type_method_bits(py, "__delattr__").unwrap();
            inc_ref_bits(py, setter);
            inc_ref_bits(py, deleter);
            let iterations = 4096_u64;
            for family in ["normal_class", "explicit_type", "explicit_object"] {
                let started = std::time::Instant::now();
                for _ in 0..iterations {
                    let value = MoltObject::from_int(79).bits();
                    match family {
                        "normal_class" => {
                            crate::molt_set_attr_name(class, name, value);
                            crate::molt_del_attr_name(class, name);
                        }
                        "explicit_type" => {
                            let result = crate::call_callable3(py, setter, class, name, value);
                            dec_ref_bits(py, result);
                            let result = crate::call_callable2(py, deleter, class, name);
                            dec_ref_bits(py, result);
                        }
                        _ => {
                            crate::molt_object_setattr(receiver, name, value);
                            crate::molt_object_delattr(receiver, name);
                        }
                    }
                }
                let elapsed = started.elapsed().as_nanos();
                assert!(!crate::exception_pending(py));
                let class_projection = GLOBAL_BRIDGE.type_has_projection(class);
                let object_projection = (*crate::header_from_obj_ptr(pointer))
                    .has_flag(crate::object::HEADER_FLAG_HAS_ABI_VIEW);
                let name_projection =
                    (*crate::header_from_obj_ptr(obj_from_bits(name).as_ptr().unwrap()))
                        .has_flag(crate::object::HEADER_FLAG_HAS_ABI_VIEW);
                println!(
                    "{{\"profile\":\"managed_mutation_projection\",\"family\":\"{family}\",\"iterations\":{iterations},\"elapsed_ns\":{elapsed},\"class_projection\":{class_projection},\"object_projection\":{object_projection},\"name_projection\":{name_projection}}}"
                );
                assert!(!class_projection && !object_projection && !name_projection);
                let dictionary = if family == "explicit_object" {
                    crate::object::field_storage::current_dictionary(py, pointer)
                        .unwrap()
                        .unwrap()
                } else {
                    crate::class_dict_bits(obj_from_bits(class).as_ptr().unwrap())
                };
                assert_eq!(
                    crate::dict_get_in_place(py, obj_from_bits(dictionary).as_ptr().unwrap(), name),
                    None
                );
            }
            for bits in [setter, deleter, name, receiver, class] {
                dec_ref_bits(py, bits);
            }
        }
    });
}
