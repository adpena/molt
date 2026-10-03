//! Real-runtime witnesses for native descriptor operand publication. Assertions
//! read runtime storage directly, before any later C-to-runtime observation.
use super::*;
use molt_cpython_abi::abi_types::{self, PyBaseExceptionObject};
use molt_cpython_abi::api::{mapping, object, sequences, strings, typeobj};
use std::ffi::{CStr, c_int};

unsafe fn declared_member(owner: *mut PyTypeObject, name: &CStr) -> *mut PyObject {
    let mut entry = unsafe { (*owner).tp_members };
    assert!(!entry.is_null(), "builtin member table is initialized");
    while !unsafe { (*entry).name.is_null() } {
        if unsafe { CStr::from_ptr((*entry).name) } == name {
            let descriptor = unsafe { typeobj::PyDescr_NewMember(owner, entry) };
            assert!(!descriptor.is_null());
            return descriptor;
        }
        entry = unsafe { entry.add(1) };
    }
    panic!("builtin member {} is declared", name.to_string_lossy());
}

unsafe fn reserved_notes_member() -> *mut PyObject {
    // An explicit test descriptor for reserved physical metadata. CPython does
    // not declare BaseException.__notes__; Python notes live in __dict__.
    static mut MEMBER: abi_types::PyMemberDef = abi_types::PyMemberDef {
        name: c"_reserved_notes_probe".as_ptr(),
        type_: 16, // T_OBJECT_EX
        offset: core::mem::offset_of!(PyBaseExceptionObject, notes) as isize,
        flags: 0,
        doc: ptr::null(),
    };
    let descriptor = unsafe {
        typeobj::PyDescr_NewMember(&raw mut abi_types::PyExc_BaseException, &raw mut MEMBER)
    };
    assert!(!descriptor.is_null());
    descriptor
}

#[test]
fn member_set_and_delete_publish_exception_base_and_typed_fields_immediately() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil(|py| unsafe {
        molt_cpython_abi::bridge::molt_cpython_abi_init();
        let receiver = crate::builtins::exceptions::alloc_exception(
            &py,
            "AttributeError",
            "native member return",
        );
        assert!(!receiver.is_null());
        let receiver_bits = MoltObject::from_ptr(receiver).bits();
        for (descriptor, name, typed) in [
            (reserved_notes_member(), c"_reserved_notes_probe", false),
            (
                declared_member(&raw mut abi_types::PyExc_AttributeError, c"obj"),
                c"obj",
                true,
            ),
        ] {
            let descriptor_bits = GLOBAL_BRIDGE.molt_value_for_pyobj(descriptor).unwrap();
            for value in [
                MoltObject::from_int(731).bits(),
                MoltObject::from_float(0.0).bits(),
                MoltObject::none().bits(),
            ] {
                assert_eq!(
                    descriptor_mutate(
                        &py,
                        descriptor_bits,
                        receiver_bits,
                        DescriptorMutation::Set(value)
                    ),
                    DescriptorMutationOutcome::Applied,
                );
                // No bridge lookup of receiver is allowed between the mutation
                // and this assertion: that would commit and hide the regression.
                let observed = if typed {
                    crate::builtins::exceptions::exception_typed_field_raw_bits(
                        receiver,
                        ExceptionTypedField::AttributeErrorObject,
                    )
                    .unwrap()
                } else {
                    crate::exception_notes_bits(receiver)
                };
                assert_eq!(
                    observed,
                    value,
                    "{} runtime field is current",
                    name.to_string_lossy()
                );
                assert!(!crate::builtins::exceptions::exception_field_is_missing(
                    observed
                ));
                let exposed = descriptor_bind(
                    &py,
                    descriptor_bits,
                    Some(crate::object_class_bits(receiver)),
                    Some(receiver_bits),
                )
                .expect("member descriptor returns its present value");
                assert_eq!(exposed, value);
                dec_ref_bits(&py, exposed);
                assert!(!exception_pending(&py));
            }
            assert_eq!(
                descriptor_mutate(
                    &py,
                    descriptor_bits,
                    receiver_bits,
                    DescriptorMutation::Delete
                ),
                DescriptorMutationOutcome::Applied,
            );
            let observed = if typed {
                crate::builtins::exceptions::exception_typed_field_raw_bits(
                    receiver,
                    ExceptionTypedField::AttributeErrorObject,
                )
                .unwrap()
            } else {
                crate::exception_notes_bits(receiver)
            };
            assert!(
                crate::builtins::exceptions::exception_field_is_missing(observed),
                "{} deletion publishes absence, not explicit None",
                name.to_string_lossy(),
            );
            let exposed = descriptor_bind(
                &py,
                descriptor_bits,
                Some(crate::object_class_bits(receiver)),
                Some(receiver_bits),
            )
            .expect("required descriptor binding returns an owned result");
            assert_eq!(exposed, MoltObject::none().bits());
            assert_eq!(exception_pending(&py), !typed);
            if !typed {
                let error = crate::exception_last_bits_noinc(&py).unwrap();
                assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                    &py,
                    error,
                    "AttributeError",
                ));
                crate::clear_exception(&py);
            }
            dec_ref_bits(&py, exposed);
            dec_ref_bits(&py, descriptor_bits);
            refcount::Py_DECREF(descriptor);
        }
        dec_ref_bits(&py, receiver_bits);
        assert!(!exception_pending(&py));
        assert!(errors::PyErr_Occurred().is_null());
    });
}

#[test]
fn explicit_bound_member_wrappers_share_the_descriptor_return_boundary() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil(|py| unsafe {
        molt_cpython_abi::bridge::molt_cpython_abi_init();
        let receiver = crate::builtins::exceptions::alloc_exception(
            &py,
            "AttributeError",
            "bound native member return",
        );
        assert!(!receiver.is_null());
        let receiver_bits = MoltObject::from_ptr(receiver).bits();
        let descriptor = declared_member(&raw mut abi_types::PyExc_AttributeError, c"obj");
        let setter = object::PyObject_GetAttrString(descriptor, c"__set__".as_ptr());
        assert!(!setter.is_null());
        assert_eq!((*setter).ob_type, &raw mut abi_types::_PyMethodWrapper_Type);
        let setter_bits = GLOBAL_BRIDGE.molt_value_for_pyobj(setter).unwrap();
        for value in [MoltObject::from_int(947).bits(), MoltObject::none().bits()] {
            let result = crate::call_callable2(&py, setter_bits, receiver_bits, value);
            assert_eq!(
                crate::builtins::exceptions::exception_typed_field_raw_bits(
                    receiver,
                    ExceptionTypedField::AttributeErrorObject,
                ),
                Some(value),
                "a bound __set__ call must publish before returning to runtime",
            );
            assert!(!exception_pending(&py));
            assert_eq!(result, MoltObject::none().bits());
            dec_ref_bits(&py, result);
        }
        dec_ref_bits(&py, setter_bits);
        refcount::Py_DECREF(setter);

        let deleter = object::PyObject_GetAttrString(descriptor, c"__delete__".as_ptr());
        assert!(!deleter.is_null());
        assert_eq!(
            (*deleter).ob_type,
            &raw mut abi_types::_PyMethodWrapper_Type
        );
        let deleter_bits = GLOBAL_BRIDGE.molt_value_for_pyobj(deleter).unwrap();
        let result = crate::call_callable1(&py, deleter_bits, receiver_bits);
        assert!(crate::builtins::exceptions::exception_field_is_missing(
            crate::builtins::exceptions::exception_typed_field_raw_bits(
                receiver,
                ExceptionTypedField::AttributeErrorObject,
            )
            .unwrap(),
        ));
        assert!(!exception_pending(&py));
        assert_eq!(result, MoltObject::none().bits());
        dec_ref_bits(&py, result);
        dec_ref_bits(&py, deleter_bits);
        refcount::Py_DECREF(deleter);
        refcount::Py_DECREF(descriptor);
        dec_ref_bits(&py, receiver_bits);
        assert!(errors::PyErr_Occurred().is_null());
    });
}

#[repr(C)]
struct MutatingDescriptor {
    object: PyObject,
    value: *mut PyObject,
    error: *mut PyObject,
    fail: bool,
    invalidate: bool,
    last_receiver: *mut PyBaseExceptionObject,
    result: *mut PyObject,
}

unsafe fn mutate_projection(descriptor: *mut PyObject, receiver: *mut PyObject) -> bool {
    let descriptor = unsafe { &mut *descriptor.cast::<MutatingDescriptor>() };
    let receiver = receiver.cast::<PyBaseExceptionObject>();
    descriptor.last_receiver = receiver;
    unsafe {
        refcount::Py_INCREF(descriptor.value);
        let old = std::mem::replace(&mut (*receiver).notes, descriptor.value);
        refcount::Py_XDECREF(old);
        if descriptor.invalidate {
            (*receiver).suppress_context = 2;
        }
        if descriptor.fail {
            errors::PyErr_SetObject(
                (&raw mut abi_types::PyExc_ValueError).cast(),
                descriptor.error,
            );
        }
    }
    !descriptor.fail
}

unsafe extern "C" fn mutate_get(
    descriptor: *mut PyObject,
    receiver: *mut PyObject,
    _owner: *mut PyObject,
) -> *mut PyObject {
    if !unsafe { mutate_projection(descriptor, receiver) } {
        return ptr::null_mut();
    }
    unsafe { object::Py_NewRef((*descriptor.cast::<MutatingDescriptor>()).result) }
}

unsafe extern "C" fn mutate_set(
    descriptor: *mut PyObject,
    receiver: *mut PyObject,
    _value: *mut PyObject,
) -> c_int {
    if unsafe { mutate_projection(descriptor, receiver) } {
        0
    } else {
        -1
    }
}

#[test]
fn getter_and_setter_partial_mutations_commit_without_replacing_callback_errors() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil(|py| unsafe {
        molt_cpython_abi::bridge::molt_cpython_abi_init();
        let receiver =
            crate::builtins::exceptions::alloc_exception(&py, "AttributeError", "receiver");
        let original = crate::builtins::exceptions::alloc_exception(
            &py,
            "ValueError",
            "original callback error",
        );
        assert!(!receiver.is_null() && !original.is_null());
        let receiver_bits = MoltObject::from_ptr(receiver).bits();
        let original_bits = MoltObject::from_ptr(original).bits();
        let original_view = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(original_bits);
        assert!(!original_view.is_null());
        let mut kind: PyTypeObject = std::mem::zeroed();
        kind.tp_name = c"MutatingProjectionDescriptor".as_ptr();
        kind.tp_descr_get = Some(mutate_get);
        kind.tp_descr_set = Some(mutate_set);
        let mut descriptor = MutatingDescriptor {
            object: PyObject {
                ob_refcnt: 1,
                ob_type: &raw mut kind,
            },
            value: ptr::null_mut(),
            error: original_view,
            fail: false,
            invalidate: false,
            last_receiver: ptr::null_mut(),
            result: &raw mut abi_types::Py_None,
        };
        let descriptor_bits = GLOBAL_BRIDGE
            .molt_value_for_pyobj(&raw mut descriptor.object)
            .unwrap();
        for (index, (setter, fail)) in [(false, false), (false, true), (true, false), (true, true)]
            .into_iter()
            .enumerate()
        {
            let expected = MoltObject::from_int(1100 + index as i64).bits();
            descriptor.value = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(expected);
            descriptor.fail = fail;
            if setter {
                let outcome = descriptor_mutate(
                    &py,
                    descriptor_bits,
                    receiver_bits,
                    DescriptorMutation::Delete,
                );
                assert_eq!(
                    outcome,
                    if fail {
                        DescriptorMutationOutcome::Error
                    } else {
                        DescriptorMutationOutcome::Applied
                    }
                );
            } else {
                let result = descriptor_bind(
                    &py,
                    descriptor_bits,
                    Some(builtin_classes(&py).object),
                    Some(receiver_bits),
                );
                if let Some(result) = result {
                    dec_ref_bits(&py, result);
                }
            }
            assert_eq!(crate::exception_notes_bits(receiver), expected);
            assert_eq!(exception_pending(&py), fail);
            if fail {
                assert_eq!(crate::exception_last_bits_noinc(&py), Some(original_bits));
                crate::clear_exception(&py);
            }
            refcount::Py_DECREF(descriptor.value);
            descriptor.value = ptr::null_mut();
        }
        dec_ref_bits(&py, descriptor_bits);
        assert_eq!(descriptor.object.ob_refcnt, 1);
        refcount::Py_DECREF(original_view);
        dec_ref_bits(&py, original_bits);
        dec_ref_bits(&py, receiver_bits);
        assert!(errors::PyErr_Occurred().is_null());
    });
}

#[test]
fn synchronization_failure_rejects_result_and_preserves_an_original_callback_error() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil(|py| unsafe {
        molt_cpython_abi::bridge::molt_cpython_abi_init();
        let receiver =
            crate::builtins::exceptions::alloc_exception(&py, "AttributeError", "invalid mutation");
        let original =
            crate::builtins::exceptions::alloc_exception(&py, "ValueError", "callback precedence");
        assert!(!receiver.is_null() && !original.is_null());
        let receiver_bits = MoltObject::from_ptr(receiver).bits();
        let original_bits = MoltObject::from_ptr(original).bits();
        let original_view = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(original_bits);
        let replacement =
            GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(MoltObject::from_int(1200).bits());
        assert!(!original_view.is_null() && !replacement.is_null());
        let before = crate::exception_notes_bits(receiver);
        let mut result_type: PyTypeObject = std::mem::zeroed();
        result_type.tp_name = c"RejectedDescriptorResult".as_ptr();
        let mut result = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut result_type,
        };
        let mut kind: PyTypeObject = std::mem::zeroed();
        kind.tp_name = c"InvalidProjectionDescriptor".as_ptr();
        kind.tp_descr_get = Some(mutate_get);
        let mut descriptor = MutatingDescriptor {
            object: PyObject {
                ob_refcnt: 1,
                ob_type: &raw mut kind,
            },
            value: replacement,
            error: original_view,
            fail: false,
            invalidate: true,
            last_receiver: ptr::null_mut(),
            result: &raw mut result,
        };
        let descriptor_bits = GLOBAL_BRIDGE
            .molt_value_for_pyobj(&raw mut descriptor.object)
            .unwrap();
        for fail in [false, true] {
            descriptor.fail = fail;
            descriptor_bind(
                &py,
                descriptor_bits,
                Some(builtin_classes(&py).object),
                Some(receiver_bits),
            );
            assert_eq!(
                crate::exception_notes_bits(receiver),
                before,
                "rejected snapshot is atomic"
            );
            assert_eq!(
                result.ob_refcnt, 1,
                "synchronization failure releases an owned callback result"
            );
            assert!(exception_pending(&py));
            let error = crate::exception_last_bits_noinc(&py).unwrap();
            if fail {
                assert_eq!(error, original_bits);
            } else {
                assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                    &py,
                    error,
                    "SystemError"
                ));
            }
            crate::clear_exception(&py);
            // Repair only the test-injected invalid physical scalar before the
            // next call; no observing conversion may publish the rejected notes.
            (*descriptor.last_receiver).suppress_context = 0;
        }
        dec_ref_bits(&py, descriptor_bits);
        refcount::Py_DECREF(replacement);
        refcount::Py_DECREF(original_view);
        dec_ref_bits(&py, original_bits);
        dec_ref_bits(&py, receiver_bits);
        assert_eq!(descriptor.object.ob_refcnt, 1);
        assert!(errors::PyErr_Occurred().is_null());
    });
}

#[test]
fn inherited_exception_init_wrapper_publishes_args_before_foreign_call_returns() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil(|py| unsafe {
        molt_cpython_abi::bridge::molt_cpython_abi_init();
        let receiver = crate::builtins::exceptions::alloc_exception(&py, "AttributeError", "old");
        assert!(!receiver.is_null());
        let receiver_bits = MoltObject::from_ptr(receiver).bits();
        let receiver_view = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(receiver_bits);
        let inherited = mapping::PyDict_GetItemString(
            abi_types::PyExc_BaseException.tp_dict,
            c"__init__".as_ptr(),
        );
        assert!(!inherited.is_null());
        // The canonical class dictionary owns a managed semantic descriptor.
        // Bind and invoke it without treating its C carrier as a physical slot
        // wrapper. This remains an independent consumer of inherited __init__.
        let inherited_bits = GLOBAL_BRIDGE
            .observed_handle_for_pyobj(inherited)
            .unwrap()
            .bits();
        assert_eq!(
            crate::object_class_bits(crate::obj_from_bits(inherited_bits).as_ptr().unwrap()),
            builtin_classes(&py).wrapper_descriptor,
        );
        let managed_bound = descriptor_bind(
            &py,
            inherited_bits,
            Some(crate::object_class_bits(receiver)),
            Some(receiver_bits),
        )
        .unwrap();
        assert_eq!(
            crate::object_class_bits(crate::obj_from_bits(managed_bound).as_ptr().unwrap()),
            builtin_classes(&py).method_wrapper,
        );
        let managed_value = MoltObject::from_int(1700).bits();
        let result = crate::call_callable1(&py, managed_bound, managed_value);
        let managed_args = crate::builtins::exceptions::exception_args_bits(receiver);
        crate::object::seq_access::with_immutable_tuple_slice(
            crate::obj_from_bits(managed_args).as_ptr().unwrap(),
            |args| assert_eq!(args, &[managed_value]),
        )
        .expect("managed initializer publishes tuple arguments");
        assert!(!exception_pending(&py));
        assert_eq!(result, MoltObject::none().bits());
        dec_ref_bits(&py, result);
        dec_ref_bits(&py, managed_bound);

        // Select the actual production Init adapter from the canonical slot
        // table, then test a real native wrapper's hidden-self publication.
        let physical = typeobj::init_slot_wrapper_for_test(
            &raw mut abi_types::PyExc_BaseException,
            abi_types::PyExc_BaseException.tp_init.unwrap(),
        );
        assert!(!physical.is_null());
        assert_eq!((*physical).ob_type, &raw mut abi_types::PyWrapperDescr_Type);
        assert_eq!(
            (*physical.cast::<abi_types::PyWrapperDescrObject>())
                .d_common
                .d_type,
            &raw mut abi_types::PyExc_BaseException,
        );
        let bound = typeobj::PyWrapper_New(physical, receiver_view);
        assert!(!bound.is_null());
        assert_eq!((*bound).ob_type, &raw mut abi_types::_PyMethodWrapper_Type);
        let args = crate::alloc_tuple(&py, &[MoltObject::from_int(1701).bits()]);
        assert!(!args.is_null());
        let args_bits = MoltObject::from_ptr(args).bits();
        let result =
            molt_cpython_abi::bridge::molt_foreign_call(bound.expose_provenance(), args_bits, 0);
        assert_eq!(
            crate::builtins::exceptions::exception_args_bits(receiver),
            args_bits,
            "the native initializer publishes its hidden self before returning",
        );
        let molt_cpython_abi::hooks::DecodedHandleResult::Ok(value) = result.decode() else {
            panic!("inherited native initializer failed");
        };
        assert_eq!(value, MoltObject::none().bits());
        dec_ref_bits(&py, value);
        refcount::Py_DECREF(bound);
        refcount::Py_DECREF(physical);
        refcount::Py_DECREF(receiver_view);
        dec_ref_bits(&py, args_bits);
        dec_ref_bits(&py, receiver_bits);
        assert!(errors::PyErr_Occurred().is_null());
    });
}

unsafe fn callback_mutations(
    receiver: *mut PyObject,
    positional: *mut PyObject,
    keyword: *mut PyObject,
    value: *mut PyObject,
    error: *mut PyObject,
    invalid: *mut PyObject,
) -> c_int {
    for target in [receiver, positional, keyword] {
        if !target.is_null() {
            let target = target.cast::<PyBaseExceptionObject>();
            unsafe {
                refcount::Py_INCREF(value);
                let old = std::mem::replace(&mut (*target).notes, value);
                refcount::Py_XDECREF(old);
            }
        }
    }
    if invalid == (&raw mut abi_types::Py_True).cast::<PyObject>() {
        unsafe { (*positional.cast::<PyBaseExceptionObject>()).suppress_context = 2 };
    }
    if error != &raw mut abi_types::Py_None {
        unsafe { errors::PyErr_SetObject((&raw mut abi_types::PyExc_ValueError).cast(), error) };
        -1
    } else {
        0
    }
}

unsafe fn callback_tuple_mutations(
    receiver: *mut PyObject,
    args: *mut PyObject,
    kwargs: *mut PyObject,
) -> c_int {
    let keyword = unsafe { mapping::PyDict_GetItemString(kwargs, c"target".as_ptr()) };
    assert!(!keyword.is_null());
    let status = unsafe {
        callback_mutations(
            receiver,
            sequences::PyTuple_GetItem(args, 0),
            keyword,
            sequences::PyTuple_GetItem(args, 1),
            sequences::PyTuple_GetItem(args, 2),
            sequences::PyTuple_GetItem(args, 3),
        )
    };
    // The completion boundary must retain the original keyword operand even
    // though it is no longer reachable from the kwargs container on return.
    unsafe { errors::with_preserved_error(|| mapping::PyDict_Clear(kwargs)) };
    status
}

thread_local! {
    static CALLBACK_TUPLE_OPERANDS: std::cell::Cell<Option<[usize; 3]>> = const {
        std::cell::Cell::new(None)
    };
}

fn record_tuple_operands(receiver: *mut PyObject, args: *mut PyObject, kwargs: *mut PyObject) {
    CALLBACK_TUPLE_OPERANDS.with(|operands| {
        assert!(
            operands
                .replace(Some([receiver.addr(), args.addr(), kwargs.addr()]))
                .is_none(),
            "one native tuple callback per invocation"
        );
    });
}

unsafe extern "C" fn callback_native_call(
    callable: *mut PyObject,
    args: *mut PyObject,
    kwargs: *mut PyObject,
) -> *mut PyObject {
    record_tuple_operands(callable, args, kwargs);
    if unsafe { callback_tuple_mutations(ptr::null_mut(), args, kwargs) } < 0 {
        ptr::null_mut()
    } else {
        unsafe { object::Py_NewRef(&raw mut abi_types::Py_None) }
    }
}

unsafe extern "C" fn callback_bound_call(
    receiver: *mut PyObject,
    args: *mut PyObject,
    kwargs: *mut PyObject,
) -> *mut PyObject {
    record_tuple_operands(receiver, args, kwargs);
    if unsafe { callback_tuple_mutations(receiver, args, kwargs) } < 0 {
        ptr::null_mut()
    } else {
        unsafe { object::Py_NewRef(&raw mut abi_types::Py_None) }
    }
}

unsafe extern "C" fn callback_slot_init(
    receiver: *mut PyObject,
    args: *mut PyObject,
    kwargs: *mut PyObject,
) -> c_int {
    record_tuple_operands(receiver, args, kwargs);
    unsafe { callback_tuple_mutations(receiver, args, kwargs) }
}

unsafe extern "C" fn callback_native_vectorcall(
    _callable: *mut PyObject,
    args: *mut *mut PyObject,
    nargsf: usize,
    kwnames: *mut PyObject,
) -> *mut PyObject {
    assert_eq!(nargsf & !(1usize << (usize::BITS - 1)), 4);
    assert_eq!(unsafe { sequences::PyTuple_Size(kwnames) }, 1);
    let values = unsafe { std::slice::from_raw_parts(args, 5) };
    if unsafe {
        callback_mutations(
            ptr::null_mut(),
            values[0],
            values[4],
            values[1],
            values[2],
            values[3],
        )
    } < 0
    {
        ptr::null_mut()
    } else {
        unsafe { object::Py_NewRef(&raw mut abi_types::Py_None) }
    }
}

#[repr(C)]
struct ProjectionNativeCallable {
    object: PyObject,
    vectorcall: Option<abi_types::PyVectorcallFunc>,
}

unsafe fn projection_call_tuple(values: &[*mut PyObject]) -> *mut PyObject {
    let args = unsafe { sequences::PyTuple_New(values.len() as isize) };
    assert!(!args.is_null());
    for (index, &value) in values.iter().enumerate() {
        unsafe { refcount::Py_INCREF(value) };
        assert_eq!(
            unsafe { sequences::PyTuple_SetItem(args, index as isize, value) },
            0
        );
    }
    args
}

#[test]
fn native_call_returns_publish_direct_and_hidden_operands_with_original_errors() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil(|py| unsafe {
        molt_cpython_abi::bridge::molt_cpython_abi_init();
        // tp_call, direct vectorcall, tuple adapter, dict adapter, CFunction,
        // and bound slot wrapper all cross the same publication contract.
        for mode in 0..6 {
            for (fail, invalid) in [(false, false), (true, false), (false, true), (true, true)] {
                let targets = ["hidden receiver", "positional", "keyword"].map(|message| {
                    crate::builtins::exceptions::alloc_exception(&py, "AttributeError", message)
                });
                assert!(targets.iter().all(|target| !target.is_null()));
                let target_bits = targets.map(|target| MoltObject::from_ptr(target).bits());
                let views =
                    target_bits.map(|bits| GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(bits));
                assert!(views.iter().all(|view| !view.is_null()));
                let original = crate::builtins::exceptions::alloc_exception(
                    &py,
                    "ValueError",
                    "exact native callback error",
                );
                assert!(!original.is_null());
                let original_bits = MoltObject::from_ptr(original).bits();
                let original_view = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(original_bits);
                let expected = MoltObject::from_int(1800 + mode).bits();
                let value = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(expected);
                assert!(!original_view.is_null() && !value.is_null());
                let error = if fail {
                    original_view
                } else {
                    &raw mut abi_types::Py_None
                };
                let flag = if invalid {
                    (&raw mut abi_types::Py_True).cast::<PyObject>()
                } else {
                    (&raw mut abi_types::Py_False).cast::<PyObject>()
                };
                let positional = [views[1], value, error, flag];
                let args = projection_call_tuple(&positional);
                let kwargs = mapping::PyDict_New();
                assert!(!kwargs.is_null());
                assert_eq!(
                    mapping::PyDict_SetItemString(kwargs, c"target".as_ptr(), views[2]),
                    0
                );
                let name = strings::PyUnicode_FromString(c"target".as_ptr());
                let names = projection_call_tuple(&[name]);
                let mut kind: PyTypeObject = std::mem::zeroed();
                kind.tp_name = c"ProjectionNativeCallable".as_ptr();
                kind.tp_basicsize = std::mem::size_of::<ProjectionNativeCallable>() as isize;
                kind.tp_call = Some(callback_native_call);
                if (1..=3).contains(&mode) {
                    kind.tp_flags = abi_types::Py_TPFLAGS_HAVE_VECTORCALL;
                    kind.tp_vectorcall_offset =
                        std::mem::offset_of!(ProjectionNativeCallable, vectorcall) as isize;
                }
                let mut native = ProjectionNativeCallable {
                    object: PyObject {
                        ob_refcnt: 1,
                        ob_type: &raw mut kind,
                    },
                    vectorcall: Some(callback_native_vectorcall),
                };
                let mut method = abi_types::PyMethodDef {
                    ml_name: c"mutate_callback_operands".as_ptr(),
                    ml_meth: Some(std::mem::transmute::<
                        unsafe extern "C" fn(
                            *mut PyObject,
                            *mut PyObject,
                            *mut PyObject,
                        ) -> *mut PyObject,
                        abi_types::PyCFunction,
                    >(callback_bound_call)),
                    ml_flags: abi_types::METH_VARARGS | abi_types::METH_KEYWORDS,
                    ml_doc: ptr::null(),
                };
                let mut owned_callable = ptr::null_mut();
                let mut owned_descriptor = ptr::null_mut();
                let callable = if mode == 4 {
                    owned_callable =
                        object::PyCFunction_NewEx(&raw mut method, views[0], ptr::null_mut());
                    owned_callable
                } else if mode == 5 {
                    owned_descriptor = typeobj::init_slot_wrapper_for_test(
                        &raw mut abi_types::PyExc_BaseException,
                        callback_slot_init,
                    );
                    assert!(!owned_descriptor.is_null());
                    assert_eq!(
                        (*owned_descriptor).ob_type,
                        &raw mut abi_types::PyWrapperDescr_Type,
                    );
                    owned_callable = typeobj::PyWrapper_New(owned_descriptor, views[0]);
                    assert!(!owned_callable.is_null());
                    assert_eq!(
                        (*owned_callable).ob_type,
                        &raw mut abi_types::_PyMethodWrapper_Type,
                    );
                    owned_callable
                } else {
                    &raw mut native.object
                };
                assert!(!callable.is_null());
                let before = crate::exception_notes_bits(targets[1]);
                let mut stack = [ptr::null_mut(), views[1], value, error, flag, views[2]];
                let result = match mode {
                    1 => object::PyObject_Vectorcall(
                        callable,
                        stack.as_mut_ptr().add(1),
                        4 | (1usize << (usize::BITS - 1)),
                        names,
                    ),
                    3 => object::PyObject_VectorcallDict(
                        callable,
                        stack.as_mut_ptr().add(1),
                        4 | (1usize << (usize::BITS - 1)),
                        kwargs,
                    ),
                    _ => object::PyObject_Call(callable, args, kwargs),
                };
                let tuple_operands = CALLBACK_TUPLE_OPERANDS.with(|operands| operands.take());
                match mode {
                    0 | 5 => {
                        let receiver = if mode == 0 { callable } else { views[0] };
                        assert_eq!(
                            tuple_operands,
                            Some([receiver.addr(), args.addr(), kwargs.addr()]),
                            "mode {mode} preserves receiver, tuple and non-NULL dictionary identity"
                        );
                    }
                    4 => {
                        // CFunction's vectorcall adapter packs fresh containers;
                        // hidden self and the direct values retain identity.
                        let [receiver, packed_args, packed_kwargs] =
                            tuple_operands.expect("CFunction keyword callback was invoked");
                        assert_eq!(receiver, views[0].addr());
                        assert_ne!(packed_args, 0);
                        assert_ne!(packed_kwargs, 0);
                    }
                    _ => assert_eq!(tuple_operands, None, "mode {mode} uses vectorcall"),
                }
                // Nothing observes these C pointers before the direct runtime
                // reads below: stale publication cannot hide behind conversion.
                assert_eq!(
                    crate::exception_notes_bits(targets[1]),
                    if invalid { before } else { expected },
                    "mode {mode}"
                );
                assert_eq!(
                    crate::exception_notes_bits(targets[2]),
                    expected,
                    "mode {mode}"
                );
                if mode >= 4 {
                    assert_eq!(
                        crate::exception_notes_bits(targets[0]),
                        expected,
                        "hidden self, mode {mode}"
                    );
                }
                assert_eq!(result.is_null(), fail || invalid, "mode {mode}");
                if fail || invalid {
                    let raised = errors::PyErr_GetRaisedException();
                    assert!(!raised.is_null());
                    if fail {
                        assert_eq!(
                            raised, original_view,
                            "mode {mode} preserves the callback error"
                        );
                    } else {
                        assert_eq!(
                            errors::PyErr_GivenExceptionMatches(
                                raised,
                                (&raw mut abi_types::PyExc_SystemError).cast(),
                            ),
                            1,
                            "mode {mode} reports the publication error",
                        );
                    }
                    refcount::Py_DECREF(raised);
                }
                (*views[1].cast::<PyBaseExceptionObject>()).suppress_context = 0;
                for pointer in [
                    result,
                    owned_callable,
                    owned_descriptor,
                    names,
                    name,
                    args,
                    kwargs,
                    value,
                    original_view,
                ] {
                    refcount::Py_XDECREF(pointer);
                }
                for view in views {
                    refcount::Py_DECREF(view);
                }
                for bits in target_bits {
                    dec_ref_bits(&py, bits);
                }
                dec_ref_bits(&py, original_bits);
                assert_eq!(native.object.ob_refcnt, 1);
                assert!(errors::PyErr_Occurred().is_null());
            }
        }
    });
}
