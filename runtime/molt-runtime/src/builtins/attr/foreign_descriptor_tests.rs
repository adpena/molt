//! Exercise native physical descriptors through the compiled-Python authority.
use super::*;
use molt_cpython_abi::abi_types::{PyObject, PyTypeObject};
use molt_cpython_abi::api::{errors, refcount};
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};

static OLD_TYPE_FINALIZED: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn retire_test_type(_object: *mut PyObject) {
    OLD_TYPE_FINALIZED.store(true, Ordering::SeqCst);
}

#[repr(C)]
struct ClassSwitchDescriptor {
    prefix: Descriptor,
    replacement: *mut PyTypeObject,
    name: *mut PyObject,
    owner_alive_during_callback: bool,
}

unsafe extern "C" fn switch_receiver_class(
    descriptor: *mut PyObject,
    receiver: *mut PyObject,
    owner: *mut PyObject,
) -> *mut PyObject {
    let descriptor = unsafe { &mut *descriptor.cast::<ClassSwitchDescriptor>() };
    let original = unsafe { (*receiver).ob_type };
    unsafe {
        refcount::Py_INCREF(descriptor.replacement.cast());
        (*receiver).ob_type = descriptor.replacement;
        refcount::Py_DECREF(original.cast());
    }
    descriptor.owner_alive_during_callback =
        owner == original.cast() && !OLD_TYPE_FINALIZED.load(Ordering::SeqCst);
    if !descriptor.owner_alive_during_callback {
        return ptr::null_mut();
    }
    unsafe {
        molt_cpython_abi::api::mapping::PyDict_DelItem((*original).tp_dict, descriptor.name);
        refcount::Py_INCREF(receiver);
    }
    receiver
}

#[test]
fn native_descriptor_reentry_retains_original_type_and_namespace_entry() {
    use molt_cpython_abi::api::{mapping, object, strings};
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil(|py| unsafe {
        OLD_TYPE_FINALIZED.store(false, Ordering::SeqCst);
        let mut metaclass: PyTypeObject = std::mem::zeroed();
        metaclass.tp_dealloc = Some(retire_test_type);
        let mut original: PyTypeObject = std::mem::zeroed();
        original.ob_base.ob_base.ob_refcnt = 1; // receiver owns this heap-type reference
        original.ob_base.ob_base.ob_type = &raw mut metaclass;
        original.tp_name = c"OriginalReceiver".as_ptr();
        original.tp_flags = molt_cpython_abi::abi_types::Py_TPFLAGS_READY;
        original.tp_dict = mapping::PyDict_New();
        let mut replacement: PyTypeObject = std::mem::zeroed();
        replacement.ob_base.ob_base.ob_refcnt = 1;
        replacement.tp_name = c"ReplacementReceiver".as_ptr();
        let mut descriptor_type: PyTypeObject = std::mem::zeroed();
        descriptor_type.tp_name = c"ClassSwitchDescriptor".as_ptr();
        descriptor_type.tp_descr_get = Some(switch_receiver_class);
        let name = strings::PyUnicode_FromString(c"field".as_ptr());
        let mut descriptor = ClassSwitchDescriptor {
            prefix: Descriptor {
                object: PyObject {
                    ob_refcnt: 1,
                    ob_type: &raw mut descriptor_type,
                },
                receiver: ptr::null_mut(),
                operand: ptr::null_mut(),
                calls: 0,
            },
            replacement: &raw mut replacement,
            name,
            owner_alive_during_callback: false,
        };
        assert_eq!(
            mapping::PyDict_SetItem(original.tp_dict, name, &raw mut descriptor.prefix.object),
            0
        );
        let mut receiver = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut original,
        };
        let result =
            object::_PyObject_GenericGetAttrWithDict(&raw mut receiver, name, ptr::null_mut(), 0);
        assert_eq!(result, &raw mut receiver);
        assert!(descriptor.owner_alive_during_callback);
        assert!(
            OLD_TYPE_FINALIZED.load(Ordering::SeqCst),
            "old type retires only after lookup ends"
        );
        assert_eq!(descriptor.prefix.object.ob_refcnt, 1);
        refcount::Py_DECREF(result);
        refcount::Py_DECREF(receiver.ob_type.cast());
        refcount::Py_DECREF(original.tp_dict);
        refcount::Py_DECREF(name);
        assert!(!exception_pending(&py));
        assert!(errors::PyErr_Occurred().is_null());
    });
}

#[repr(C)]
struct Descriptor {
    object: PyObject,
    receiver: *mut PyObject,
    operand: *mut PyObject,
    calls: usize,
}

unsafe extern "C" fn get(
    descriptor: *mut PyObject,
    receiver: *mut PyObject,
    owner: *mut PyObject,
) -> *mut PyObject {
    let descriptor = unsafe { &mut *descriptor.cast::<Descriptor>() };
    descriptor.receiver = receiver;
    descriptor.operand = owner;
    descriptor.calls += 1;
    let result = if receiver.is_null() { owner } else { receiver };
    unsafe { refcount::Py_XINCREF(result) };
    result
}

unsafe extern "C" fn set(
    descriptor: *mut PyObject,
    receiver: *mut PyObject,
    value: *mut PyObject,
) -> std::os::raw::c_int {
    let descriptor = unsafe { &mut *descriptor.cast::<Descriptor>() };
    descriptor.receiver = receiver;
    descriptor.operand = value;
    descriptor.calls += 1;
    0
}

unsafe extern "C" fn fail_get(
    descriptor: *mut PyObject,
    _receiver: *mut PyObject,
    _owner: *mut PyObject,
) -> *mut PyObject {
    unsafe { (*descriptor.cast::<Descriptor>()).calls += 1 };
    unsafe {
        errors::PyErr_SetNone((&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast());
    }
    ptr::null_mut()
}

unsafe extern "C" fn fail_set(
    descriptor: *mut PyObject,
    receiver: *mut PyObject,
    _value: *mut PyObject,
) -> std::os::raw::c_int {
    unsafe { fail_get(descriptor, receiver, ptr::null_mut()) };
    -1
}

unsafe extern "C" fn missing_attribute(
    _descriptor: *mut PyObject,
    _receiver: *mut PyObject,
    _owner: *mut PyObject,
) -> *mut PyObject {
    unsafe {
        errors::PyErr_SetNone((&raw mut molt_cpython_abi::abi_types::PyExc_AttributeError).cast());
    }
    ptr::null_mut()
}

unsafe extern "C" fn success_with_error(
    descriptor: *mut PyObject,
    receiver: *mut PyObject,
    owner: *mut PyObject,
) -> *mut PyObject {
    unsafe { fail_get(descriptor, receiver, owner) };
    let result = &raw mut molt_cpython_abi::abi_types::Py_None;
    unsafe { refcount::Py_INCREF(result) };
    result
}

unsafe extern "C" fn mutation_success_with_error(
    descriptor: *mut PyObject,
    receiver: *mut PyObject,
    value: *mut PyObject,
) -> std::os::raw::c_int {
    unsafe { fail_set(descriptor, receiver, value) };
    0
}

unsafe extern "C" fn failure_without_error(
    _descriptor: *mut PyObject,
    _receiver: *mut PyObject,
    _owner: *mut PyObject,
) -> *mut PyObject {
    ptr::null_mut()
}

unsafe extern "C" fn mutation_failure_without_error(
    _descriptor: *mut PyObject,
    _receiver: *mut PyObject,
    _value: *mut PyObject,
) -> std::os::raw::c_int {
    -1
}

extern "C" fn subclass_truth(_self_bits: u64) -> u64 {
    MoltObject::from_bool(true).bits()
}

extern "C" fn subclass_length(_self_bits: u64) -> u64 {
    MoltObject::from_int(13).bits()
}

#[test]
fn native_storage_does_not_bypass_subclass_truth_or_specialized_length() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil(|py| unsafe {
        let classes = builtin_classes(&py);
        let name = attr_name_bits_from_bytes(&py, b"NativeProtocolChild").unwrap();
        let bool_name = attr_name_bits_from_bytes(&py, b"__bool__").unwrap();
        let len_name = attr_name_bits_from_bytes(&py, b"__len__").unwrap();
        let truth = MoltObject::from_ptr(crate::builtins::functions::alloc_runtime_function_obj(
            &py,
            crate::builtins::functions::runtime_fn_addr(
                "subclass_truth",
                subclass_truth as *const (),
            ),
            1,
        ))
        .bits();
        let length = MoltObject::from_ptr(crate::builtins::functions::alloc_runtime_function_obj(
            &py,
            crate::builtins::functions::runtime_fn_addr(
                "subclass_length",
                subclass_length as *const (),
            ),
            1,
        ))
        .bits();
        type Length = extern "C" fn(u64) -> u64;
        let families: [(u64, Option<Length>); 11] = [
            (classes.str, Some(crate::object::ops_sys::molt_len_str)),
            (classes.tuple, Some(crate::object::ops_sys::molt_len_tuple)),
            (classes.dict, Some(crate::object::ops_sys::molt_len_dict)),
            (classes.list, Some(crate::object::ops_sys::molt_len_list)),
            (classes.set, Some(crate::object::ops_sys::molt_len_set)),
            (
                classes.frozenset,
                Some(crate::object::ops_sys::molt_len_set),
            ),
            (classes.bytes, None),
            (classes.bytearray, None),
            (classes.int, None),
            (classes.float, None),
            (classes.complex, None),
        ];
        for (base, specialized_len) in families {
            let class = crate::molt_class_new(name);
            let status = crate::molt_class_set_base(class, base);
            dec_ref_bits(&py, status);
            crate::object::class_finish_definition(&py, obj_from_bits(class).as_ptr().unwrap())
                .unwrap();
            let status = crate::molt_set_attr_name(class, bool_name, truth);
            dec_ref_bits(&py, status);
            let status = crate::molt_set_attr_name(class, len_name, length);
            dec_ref_bits(&py, status);
            let instance = crate::call_callable0(&py, class);
            assert!(!exception_pending(&py));
            assert!(crate::object::ops::is_truthy(&py, obj_from_bits(instance)));
            if let Some(length) = specialized_len {
                assert_eq!(obj_from_bits(length(instance)).as_int(), Some(13));
            }
            assert!(!exception_pending(&py));
            dec_ref_bits(&py, instance);
            dec_ref_bits(&py, class);
        }
        for value in [name, bool_name, len_name, truth, length] {
            dec_ref_bits(&py, value);
        }
    });
}

#[test]
fn malformed_native_descriptor_callbacks_cannot_publish_success_or_silent_failure() {
    type GetSlot =
        unsafe extern "C" fn(*mut PyObject, *mut PyObject, *mut PyObject) -> *mut PyObject;
    type SetSlot =
        unsafe extern "C" fn(*mut PyObject, *mut PyObject, *mut PyObject) -> std::os::raw::c_int;
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil(|py| unsafe {
        let mut kind: PyTypeObject = std::mem::zeroed();
        kind.tp_name = c"MalformedNativeDescriptor".as_ptr();
        let mut descriptor = Descriptor {
            object: PyObject {
                ob_refcnt: 1,
                ob_type: &raw mut kind,
            },
            receiver: ptr::null_mut(),
            operand: ptr::null_mut(),
            calls: 0,
        };
        let bits = GLOBAL_BRIDGE
            .molt_value_for_pyobj(&raw mut descriptor.object)
            .unwrap();
        let owner = builtin_classes(&py).object;
        let none = MoltObject::none().bits();
        for (get_slot, set_slot) in [
            (
                success_with_error as GetSlot,
                mutation_success_with_error as SetSlot,
            ),
            (
                failure_without_error as GetSlot,
                mutation_failure_without_error as SetSlot,
            ),
        ] {
            (*descriptor.object.ob_type).tp_descr_get = Some(get_slot);
            (*descriptor.object.ob_type).tp_descr_set = Some(set_slot);
            descriptor_bind(&py, bits, Some(owner), Some(none));
            assert!(exception_pending(&py));
            let error = crate::exception_last_bits_noinc(&py).unwrap();
            assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                &py,
                error,
                "SystemError"
            ));
            crate::clear_exception(&py);
            assert_eq!(
                descriptor_mutate(&py, bits, none, DescriptorMutation::Set(none)),
                DescriptorMutationOutcome::Error
            );
            assert!(exception_pending(&py));
            let error = crate::exception_last_bits_noinc(&py).unwrap();
            assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                &py,
                error,
                "SystemError"
            ));
            crate::clear_exception(&py);
        }
        dec_ref_bits(&py, bits);
        assert_eq!(descriptor.object.ob_refcnt, 1);
        assert!(errors::PyErr_Occurred().is_null());
    });
}

#[test]
fn native_generic_lookup_preserves_descriptor_precedence_and_optional_errors() {
    use molt_cpython_abi::api::{mapping, object, strings};
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil(|py| unsafe {
        let mut descriptor_type: PyTypeObject = std::mem::zeroed();
        descriptor_type.tp_name = c"SetOnlyDescriptor".as_ptr();
        descriptor_type.tp_descr_set = Some(set);
        let mut descriptor = Descriptor {
            object: PyObject {
                ob_refcnt: 1,
                ob_type: &raw mut descriptor_type,
            },
            receiver: ptr::null_mut(),
            operand: ptr::null_mut(),
            calls: 0,
        };
        let name = strings::PyUnicode_FromString(c"field".as_ptr());
        let instance_dict = mapping::PyDict_New();
        let mut receiver_type: PyTypeObject = std::mem::zeroed();
        receiver_type.ob_base.ob_base.ob_refcnt = 1;
        receiver_type.tp_name = c"NativeReceiver".as_ptr();
        receiver_type.tp_flags = molt_cpython_abi::abi_types::Py_TPFLAGS_READY;
        receiver_type.tp_dict = mapping::PyDict_New();
        let mut receiver = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut receiver_type,
        };
        assert_eq!(
            mapping::PyDict_SetItem(receiver_type.tp_dict, name, &raw mut descriptor.object),
            0
        );
        let value = strings::PyUnicode_FromString(c"instance value".as_ptr());
        assert_eq!(mapping::PyDict_SetItem(instance_dict, name, value), 0);
        // A set-only data descriptor has no get slot and does not block reads
        // from the instance namespace. With no instance value it is returned.
        let result =
            object::_PyObject_GenericGetAttrWithDict(&raw mut receiver, name, instance_dict, 0);
        assert_eq!(result, value);
        refcount::Py_DECREF(result);
        assert_eq!(mapping::PyDict_DelItem(instance_dict, name), 0);
        let result =
            object::_PyObject_GenericGetAttrWithDict(&raw mut receiver, name, instance_dict, 0);
        assert_eq!(result, &raw mut descriptor.object);
        refcount::Py_DECREF(result);

        (*descriptor.object.ob_type).tp_descr_get = Some(fail_get);
        assert!(
            object::_PyObject_GenericGetAttrWithDict(&raw mut receiver, name, instance_dict, 1)
                .is_null()
        );
        assert_eq!(
            errors::PyErr_ExceptionMatches(
                (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast()
            ),
            1
        );
        errors::PyErr_Clear();
        crate::clear_exception(&py);
        (*descriptor.object.ob_type).tp_descr_get = Some(missing_attribute);
        assert!(
            object::_PyObject_GenericGetAttrWithDict(&raw mut receiver, name, instance_dict, 1)
                .is_null()
        );
        assert!(errors::PyErr_Occurred().is_null());
        assert!(!exception_pending(&py));

        refcount::Py_DECREF(value);
        refcount::Py_DECREF(instance_dict);
        refcount::Py_DECREF(receiver_type.tp_dict);
        refcount::Py_DECREF(name);
        assert_eq!(descriptor.object.ob_refcnt, 1);
    });
}

#[test]
fn physical_descriptor_get_set_delete_preserve_absence_and_exact_values() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil(|py| unsafe {
        let mut kind: PyTypeObject = std::mem::zeroed();
        kind.tp_name = c"NativeDescriptor".as_ptr();
        kind.tp_descr_get = Some(get);
        kind.tp_descr_set = Some(set);
        let mut descriptor = Descriptor {
            object: PyObject {
                ob_refcnt: 1,
                ob_type: &raw mut kind,
            },
            receiver: ptr::null_mut(),
            operand: ptr::null_mut(),
            calls: 0,
        };
        let bits = GLOBAL_BRIDGE
            .molt_value_for_pyobj(&raw mut descriptor.object)
            .unwrap();
        let owner = builtin_classes(&py).object;
        let value = MoltObject::from_ptr(alloc_string(&py, b"receiver")).bits();
        let none = MoltObject::none().bits();
        assert!(descriptor_is_data(&py, bits));

        // Class access has an absent receiver; an explicit None receiver does not.
        let class_access = descriptor_bind(&py, bits, Some(owner), None).unwrap();
        assert_eq!(class_access, owner);
        assert!(descriptor.receiver.is_null());
        assert!(!descriptor.operand.is_null());
        dec_ref_bits(&py, class_access);
        let none_access = descriptor_bind(&py, bits, Some(owner), Some(none)).unwrap();
        assert_eq!(none_access, none);
        assert_eq!(
            descriptor.receiver,
            &raw mut molt_cpython_abi::abi_types::Py_None
        );
        dec_ref_bits(&py, none_access);
        let instance_access = descriptor_bind(&py, bits, Some(owner), Some(value)).unwrap();
        assert_eq!(instance_access, value);
        dec_ref_bits(&py, instance_access);

        assert_eq!(
            descriptor_mutate(&py, bits, value, DescriptorMutation::Set(none)),
            DescriptorMutationOutcome::Applied
        );
        assert_eq!(
            descriptor.operand,
            &raw mut molt_cpython_abi::abi_types::Py_None
        );
        let zero = MoltObject::from_float(0.0).bits();
        assert_eq!(
            descriptor_mutate(&py, bits, value, DescriptorMutation::Set(zero)),
            DescriptorMutationOutcome::Applied
        );
        assert!(
            !descriptor.operand.is_null(),
            "float zero is a value, never missing"
        );
        assert_eq!(
            descriptor_mutate(&py, bits, value, DescriptorMutation::Delete),
            DescriptorMutationOutcome::Applied
        );
        assert!(descriptor.operand.is_null());
        assert_eq!(descriptor.calls, 6);

        // Native slot replacement is observed live; there is no name/cache proxy.
        (*descriptor.object.ob_type).tp_descr_get = None;
        (*descriptor.object.ob_type).tp_descr_set = None;
        assert!(!descriptor_is_data(&py, bits));
        let unbound = descriptor_bind(&py, bits, Some(owner), Some(value)).unwrap();
        assert_eq!(unbound, bits);
        dec_ref_bits(&py, unbound);
        assert_eq!(
            descriptor_mutate(&py, bits, value, DescriptorMutation::Delete),
            DescriptorMutationOutcome::NotDescriptor
        );
        assert_eq!(descriptor.calls, 6);
        dec_ref_bits(&py, value);
        dec_ref_bits(&py, bits);
        assert_eq!(
            descriptor.object.ob_refcnt, 1,
            "all crossing owners released"
        );
        assert!(!exception_pending(&py));
        assert!(errors::PyErr_Occurred().is_null());
    });
}

#[test]
fn native_descriptor_failures_keep_the_original_exception_and_stop_callbacks() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil(|py| unsafe {
        let mut kind: PyTypeObject = std::mem::zeroed();
        kind.tp_name = c"FailingNativeDescriptor".as_ptr();
        kind.tp_descr_get = Some(fail_get);
        kind.tp_descr_set = Some(fail_set);
        let mut descriptor = Descriptor {
            object: PyObject {
                ob_refcnt: 1,
                ob_type: &raw mut kind,
            },
            receiver: ptr::null_mut(),
            operand: ptr::null_mut(),
            calls: 0,
        };
        let bits = GLOBAL_BRIDGE
            .molt_value_for_pyobj(&raw mut descriptor.object)
            .unwrap();
        let owner = builtin_classes(&py).object;
        let none = MoltObject::none().bits();
        descriptor_bind(&py, bits, Some(owner), Some(none));
        assert!(exception_pending(&py));
        let original = crate::exception_last_bits_noinc(&py).unwrap();
        assert!(crate::builtins::exceptions::exception_matches_builtin_name(
            &py,
            original,
            "ValueError"
        ));
        assert_eq!(
            descriptor_mutate(&py, bits, none, DescriptorMutation::Delete),
            DescriptorMutationOutcome::Error
        );
        assert_eq!(
            descriptor.calls, 1,
            "pending failure prevents subsequent native callbacks"
        );
        assert_eq!(crate::exception_last_bits_noinc(&py), Some(original));
        crate::clear_exception(&py);
        assert_eq!(
            descriptor_mutate(&py, bits, none, DescriptorMutation::Set(none)),
            DescriptorMutationOutcome::Error
        );
        assert_eq!(descriptor.calls, 2);
        assert!(exception_pending(&py));
        crate::clear_exception(&py);
        dec_ref_bits(&py, bits);
        assert_eq!(descriptor.object.ob_refcnt, 1);
    });
}

#[test]
fn capi_native_family_checks_distinguish_semantic_subclasses_from_storage() {
    use molt_cpython_abi::api::{mapping, modules, numbers, sequences, strings};
    type Check = unsafe extern "C" fn(*mut PyObject) -> std::os::raw::c_int;
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil(|py| unsafe {
        let classes = builtin_classes(&py);
        let name = attr_name_bits_from_bytes(&py, b"NativePredicateChild").unwrap();
        let families: [(u64, Check, Check, Option<u64>); 12] = [
            (
                classes.list,
                sequences::PyList_Check,
                sequences::PyList_CheckExact,
                None,
            ),
            (
                classes.tuple,
                sequences::PyTuple_Check,
                sequences::PyTuple_CheckExact,
                None,
            ),
            (
                classes.dict,
                mapping::PyDict_Check,
                mapping::PyDict_CheckExact,
                None,
            ),
            (
                classes.set,
                sequences::PySet_Check,
                sequences::PySet_CheckExact,
                None,
            ),
            (
                classes.frozenset,
                sequences::PyFrozenSet_Check,
                sequences::PyFrozenSet_CheckExact,
                None,
            ),
            (
                classes.str,
                strings::PyUnicode_Check,
                strings::PyUnicode_CheckExact,
                None,
            ),
            (
                classes.bytes,
                strings::PyBytes_Check,
                strings::PyBytes_CheckExact,
                None,
            ),
            (
                classes.bytearray,
                strings::PyByteArray_Check,
                strings::PyByteArray_CheckExact,
                None,
            ),
            (
                classes.int,
                numbers::PyLong_Check,
                numbers::PyLong_CheckExact,
                None,
            ),
            (
                classes.float,
                numbers::PyFloat_Check,
                numbers::PyFloat_CheckExact,
                None,
            ),
            (
                classes.complex,
                numbers::PyComplex_Check,
                numbers::PyComplex_CheckExact,
                None,
            ),
            (
                classes.module,
                modules::PyModule_Check,
                modules::PyModule_CheckExact,
                Some(name),
            ),
        ];
        for (base, inclusive, exact, argument) in families {
            let class = crate::molt_class_new(name);
            let status = crate::molt_class_set_base(class, base);
            dec_ref_bits(&py, status);
            crate::object::class_finish_definition(&py, obj_from_bits(class).as_ptr().unwrap())
                .unwrap();
            assert!(!exception_pending(&py));
            for (constructor, expected_exact) in [(base, 1), (class, 0)] {
                let instance = match argument {
                    Some(argument) => crate::call_callable1(&py, constructor, argument),
                    None => crate::call_callable0(&py, constructor),
                };
                assert!(
                    !exception_pending(&py),
                    "native family construction must succeed"
                );
                assert_eq!(type_of_bits(&py, instance), constructor);
                let view = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(instance);
                assert!(!view.is_null());
                if let Some(pointer) = obj_from_bits(instance).as_ptr() {
                    assert_eq!(
                        molt_cpython_abi::abi_types::is_immortal_refcnt((*view).ob_refcnt),
                        (*crate::header_from_obj_ptr(pointer)).ref_count_snapshot()
                            == molt_codegen_abi::IMMORTAL_REFCOUNT,
                        "ABI lifetime follows runtime ownership, including empty subclasses"
                    );
                }
                assert_eq!(inclusive(view), 1);
                assert_eq!(exact(view), expected_exact);
                let observed_class = molt_cpython_abi::bridge::molt_capi_semantic_type(view);
                assert_eq!(
                    observed_class.cast::<PyObject>(),
                    GLOBAL_BRIDGE.handle_to_borrowed_pyobj(constructor)
                );
                assert_eq!(inclusive(ptr::null_mut()), 0);
                assert_eq!(exact(ptr::null_mut()), 0);
                refcount::Py_DECREF(view);
                dec_ref_bits(&py, instance);
            }
            dec_ref_bits(&py, class);
        }
        // bool inherits int but is never an exact int.
        let boolean =
            GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(MoltObject::from_bool(true).bits());
        assert_eq!(numbers::PyLong_Check(boolean), 1);
        assert_eq!(numbers::PyLong_CheckExact(boolean), 0);
        refcount::Py_DECREF(boolean);
        dec_ref_bits(&py, name);
        assert!(!exception_pending(&py));
        assert!(errors::PyErr_Occurred().is_null());
    });
}

mod attribute_failure_tests;
mod projection_tests;
mod public_type_tests;
