use super::*;
use crate::builtins::exceptions::{RaisedSnapshot, alloc_exception, resolve_raised, take_raised};
use crate::object::builders::{alloc_code_obj, alloc_tuple};
use crate::resource::{LimitedTracker, ResourceLimits, UnlimitedTracker, set_tracker};
use molt_cpython_abi::api::{errors, refcount};
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;

struct DenyRuntimeAllocations;

impl DenyRuntimeAllocations {
    fn enter() -> Self {
        set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
            max_memory: Some(0),
            ..Default::default()
        })));
        Self
    }
}

impl Drop for DenyRuntimeAllocations {
    fn drop(&mut self) {
        set_tracker(Box::new(UnlimitedTracker));
    }
}

fn exception_bits(py: &crate::PyToken<'_>, kind: &str) -> u64 {
    let exception = alloc_exception(py, kind, "pending cleanup");
    assert!(!exception.is_null());
    MoltObject::from_ptr(exception).bits()
}

thread_local! {
    static REPORTED_ERROR: std::cell::Cell<(u64, usize)> = const {
        std::cell::Cell::new((0, 0))
    };
}

extern "C" fn capture_unraisable(args: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let key = crate::attr_name_bits_from_bytes(py, b"exc_value").unwrap();
        let value = crate::molt_get_attr_name(args, key);
        REPORTED_ERROR.with(|reported| reported.set((value, reported.get().1 + 1)));
        dec_ref_bits(py, key);
        dec_ref_bits(py, value);
        MoltObject::none().bits()
    })
}

fn install_unraisable_capture(py: &crate::PyToken<'_>) {
    let name = crate::attr_name_bits_from_bytes(py, b"sys").unwrap();
    let sys = crate::builtins::modules::molt_module_new(name);
    crate::builtins::module_table::publish_interpreter_sys_for_test(py, sys);
    let key = crate::attr_name_bits_from_bytes(py, b"unraisablehook").unwrap();
    let target = capture_unraisable as *const ();
    let hook = crate::object::builders::alloc_function_obj(
        py,
        crate::provenance::abi::expose_function_address(target),
        1,
    );
    assert!(!hook.is_null());
    unsafe {
        crate::object::layout::function_set_call_target_ptr(hook, target);
    }
    let hook = MoltObject::from_ptr(hook).bits();
    crate::builtins::modules::molt_module_set_attr(sys, key, hook);
    assert!(!crate::exception_pending(py));
    for bits in [name, sys, key, hook] {
        dec_ref_bits(py, bits);
    }
    REPORTED_ERROR.with(|reported| reported.set((0, 0)));
}

#[test]
fn pending_reporting_consumes_runtime_error_and_observes_c_precedence() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        install_unraisable_capture(py);
        let runtime = exception_bits(py, "ValueError");
        let c_error = exception_bits(py, "LookupError");
        unsafe {
            crate::record_exception(py, crate::obj_from_bits(runtime).as_ptr().unwrap());
            assert!(errors::take_current_error().is_none());
            errors::PyErr_WriteUnraisable(ptr::null_mut());
            assert_eq!(REPORTED_ERROR.with(|reported| reported.get()), (runtime, 1));
            assert!(errors::PyErr_Occurred().is_null());
            assert!(!crate::exception_pending(py));

            let c_view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(c_error);
            assert!(!c_view.is_null());
            set_c_error(c_view);
            crate::test_support::restore_runtime_raised(py, runtime);
            errors::PyErr_WriteUnraisable(ptr::null_mut());
            assert_eq!(REPORTED_ERROR.with(|reported| reported.get()), (c_error, 2));
            assert!(errors::PyErr_Occurred().is_null());
            assert!(!crate::exception_pending(py));

            crate::record_exception(py, crate::obj_from_bits(runtime).as_ptr().unwrap());
            errors::PyErr_PrintEx(0);
            assert!(errors::PyErr_Occurred().is_null());
            assert!(!crate::exception_pending(py));

            crate::record_exception(py, crate::obj_from_bits(runtime).as_ptr().unwrap());
            errors::PyErr_Print();
            assert!(errors::PyErr_Occurred().is_null());
            let sys = crate::builtins::modules::interpreter_sys_module(py).unwrap();
            let key = crate::attr_name_bits_from_bytes(py, b"last_exc").unwrap();
            let last = crate::molt_get_attr_name(sys, key);
            assert_eq!(
                last, runtime,
                "PyErr_Print publishes the exact sys.last_exc"
            );
            dec_ref_bits(py, last);
            dec_ref_bits(py, key);
            assert!(!crate::exception_pending(py));
        }
        dec_ref_bits(py, runtime);
        dec_ref_bits(py, c_error);
    });
}

#[repr(C)]
struct ClearCallback {
    object: PyObject,
    raised: u64,
}

unsafe extern "C" fn clear_raises_runtime_error(object: *mut PyObject) -> c_int {
    let raised = unsafe { (*object.cast::<ClearCallback>()).raised };
    crate::with_gil_entry_nopanic!(py, {
        crate::record_exception(py, crate::obj_from_bits(raised).as_ptr().unwrap());
    });
    -1
}

#[test]
fn pending_native_clear_reports_inner_runtime_error_and_preserves_both_outer_channels() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        install_unraisable_capture(py);
        let runtime = exception_bits(py, "ValueError");
        let c_error = exception_bits(py, "LookupError");
        let inner = exception_bits(py, "TypeError");
        let mut kind: PyTypeObject = unsafe { std::mem::zeroed() };
        kind.tp_name = c"ClearCallback".as_ptr();
        kind.tp_clear = Some(clear_raises_runtime_error);
        let mut callback = ClearCallback {
            object: PyObject {
                ob_refcnt: 1,
                ob_type: &raw mut kind,
            },
            raised: inner,
        };
        unsafe {
            let c_view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(c_error);
            assert!(!c_view.is_null());
            set_c_error(c_view);
            crate::test_support::restore_runtime_raised(py, runtime);
            assert_eq!(
                molt_cpython_abi::api::memory::native_gc_node_clear(
                    (&raw mut callback.object).addr()
                ),
                0
            );
            assert_eq!(REPORTED_ERROR.with(|reported| reported.get()), (inner, 1));
            assert_eq!(crate::exception_last_bits_noinc(py), Some(runtime));
            let restored = errors::take_current_error().expect("exact C outer error");
            assert_eq!(restored.value, c_view);
            errors::with_preserved_error(|| drop(restored));
            crate::clear_exception(py);
        }
        for bits in [runtime, c_error, inner] {
            dec_ref_bits(py, bits);
        }
    });
}

unsafe fn set_c_error(view: *mut PyObject) {
    unsafe {
        refcount::Py_INCREF(view);
        errors::PyErr_SetRaisedException(view);
    }
}

fn assert_emergency_pending(py: &crate::PyToken<'_>) {
    assert!(crate::exception_pending(py));
    let mut raised = Some(take_raised(py));
    let Some(RaisedSnapshot::Thread(bits) | RaisedSnapshot::Task(_, bits)) = raised else {
        panic!("allocation failure must own a real raised exception");
    };
    assert!(crate::builtins::exceptions::exception_matches_builtin_name(
        py,
        bits,
        "MemoryError"
    ));
    resolve_raised(py, &mut raised);
}

#[test]
fn pending_cleanup_preserves_lazy_runtime_traceback_without_projection() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let filename = MoltObject::from_ptr(alloc_string(py, b"<pending-cleanup>")).bits();
        let name = MoltObject::from_ptr(alloc_string(py, b"pending_cleanup")).bits();
        let empty = MoltObject::from_ptr(alloc_tuple(py, &[])).bits();
        let code = alloc_code_obj(
            py,
            filename,
            name,
            7,
            MoltObject::none().bits(),
            empty,
            empty,
            0,
            0,
            0,
        );
        assert!(!code.is_null());
        for bits in [filename, name, empty] {
            dec_ref_bits(py, bits);
        }
        crate::builtins::frames::frame_stack_push_owned(
            py,
            MoltObject::from_ptr(code).bits(),
            0,
            0,
            0,
        );
        let exception = exception_bits(py, "ValueError");
        let exception_ptr = crate::obj_from_bits(exception).as_ptr().unwrap();
        crate::record_exception(py, exception_ptr);
        crate::builtins::frames::frame_stack_pop(py);
        let lazy_trace = unsafe { crate::exception_trace_bits(exception_ptr) };
        let trace_ptr = crate::obj_from_bits(lazy_trace)
            .as_ptr()
            .expect("captured frame");
        assert_eq!(
            unsafe { object_type_id(trace_ptr) },
            crate::TYPE_ID_TRACEBACK_PAYLOAD
        );
        assert_eq!(
            unsafe { (*header_from_obj_ptr(exception_ptr)).load_synchronized_flags() }
                & crate::object::HEADER_FLAG_HAS_ABI_VIEW,
            0,
        );

        let denied = DenyRuntimeAllocations::enter();
        assert_eq!(
            unsafe { errors::PyErr_Occurred() },
            (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast(),
        );
        assert_eq!(
            unsafe {
                errors::PyErr_ExceptionMatches(
                    (&raw mut molt_cpython_abi::abi_types::PyExc_Exception).cast(),
                )
            },
            1
        );
        assert_eq!(crate::exception_last_bits_noinc(py), Some(exception));
        assert_eq!(
            unsafe { crate::exception_trace_bits(exception_ptr) },
            lazy_trace
        );
        assert!(errors::take_current_error().is_none());
        let result = errors::with_preserved_error(|| {
            assert!(!crate::exception_pending(py));
            assert!(errors::take_current_error().is_none());
            crate::record_memory_error_without_allocation(py);
            42
        });
        assert_eq!(result, 42);
        assert_eq!(crate::exception_last_bits_noinc(py), Some(exception));
        assert_eq!(
            unsafe { crate::exception_trace_bits(exception_ptr) },
            lazy_trace
        );
        assert_eq!(
            unsafe { (*header_from_obj_ptr(exception_ptr)).load_synchronized_flags() }
                & crate::object::HEADER_FLAG_HAS_ABI_VIEW,
            0,
            "cleanup must not publish a C view",
        );
        assert!(errors::take_current_error().is_none());

        // The same live lazy error really does fail public projection under
        // this limit, leaving a fresh runtime-only failure. Cleanup must never
        // enter this path or accidentally drain its incoming error with it.
        let (mut class, mut traceback) = (0, 0);
        let projected = unsafe { hook_take_pending_exception(&raw mut class, &raw mut traceback) };
        assert!(matches!(projected.decode(), DecodedHandleResult::Error));
        assert_emergency_pending(py);
        assert!(errors::take_current_error().is_none());
        drop(denied);
        unsafe { errors::PyErr_Clear() };
        dec_ref_bits(py, exception);
    });
}

#[repr(C)]
struct ClassProjectionNumeric {
    object: PyObject,
    error: u64,
    fail: bool,
    calls: usize,
    index: *mut PyObject,
    float: *mut PyObject,
}

unsafe fn class_projection_numeric_result(object: *mut PyObject, index: bool) -> *mut PyObject {
    let state = unsafe { &mut *object.cast::<ClassProjectionNumeric>() };
    state.calls += 1;
    if state.fail {
        with_gil(|py| {
            crate::record_exception(&py, crate::obj_from_bits(state.error).as_ptr().unwrap());
        });
        ptr::null_mut()
    } else {
        let result = if index { state.index } else { state.float };
        unsafe { refcount::Py_INCREF(result) };
        result
    }
}

unsafe extern "C" fn class_projection_index(object: *mut PyObject) -> *mut PyObject {
    unsafe { class_projection_numeric_result(object, true) }
}

unsafe extern "C" fn class_projection_float(object: *mut PyObject) -> *mut PyObject {
    unsafe { class_projection_numeric_result(object, false) }
}

#[test]
fn cold_exception_class_projection_failure_preserves_numeric_error_and_outputs() {
    use super::native_test_fixture::NativeType;
    use crate::builtins::exceptions::ExceptionValue;
    use molt_cpython_abi::abi_types::{PyMemberDef, PyNumberMethods};
    use molt_cpython_abi::api::{numbers, typeobj};
    use molt_cpython_abi::hooks::PendingExceptionClass;

    let transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        let name = ExceptionValue::adopt(
            py,
            crate::attr_name_bits_from_bytes(py, b"ColdNumericConversionError").unwrap(),
        );
        let namespace = crate::alloc_dict_with_pairs(py, &[]);
        assert!(!namespace.is_null());
        let namespace = ExceptionValue::adopt(py, MoltObject::from_ptr(namespace).bits());
        let base = crate::builtins::exceptions::exception_type_bits_from_name(py, "LookupError");
        let class = ExceptionValue::adopt(
            py,
            crate::builtins::types::molt_type_new(
                crate::builtin_classes(py).type_obj,
                name.bits(),
                base,
                namespace.bits(),
                MoltObject::none().bits(),
            ),
        );
        assert!(!crate::exception_pending(py));
        let class_ptr = crate::obj_from_bits(class.bits()).as_ptr().unwrap();
        let args = alloc_tuple(py, &[]);
        assert!(!args.is_null());
        let args = ExceptionValue::adopt(py, MoltObject::from_ptr(args).bits());
        let exception = crate::builtins::exceptions::alloc_exception_from_class_bits(
            py,
            class.bits(),
            args.bits(),
        );
        assert!(!exception.is_null());
        let exception = ExceptionValue::adopt(py, MoltObject::from_ptr(exception).bits());
        let exception_ptr = crate::obj_from_bits(exception.bits()).as_ptr().unwrap();
        assert_eq!(
            unsafe { crate::object_class_bits(exception_ptr) },
            class.bits()
        );
        assert_eq!(
            unsafe { object_type_id(exception_ptr) },
            crate::TYPE_ID_EXCEPTION
        );
        let refs = || unsafe { (*header_from_obj_ptr(exception_ptr)).ref_count_snapshot() };
        let before = refs();
        let class_refs = unsafe { (*header_from_obj_ptr(class_ptr)).ref_count_snapshot() };
        let assert_cold_class = || {
            assert_eq!(
                unsafe { (*header_from_obj_ptr(class_ptr)).load_synchronized_flags() }
                    & crate::object::HEADER_FLAG_HAS_ABI_VIEW,
                0,
            );
        };
        assert_cold_class();

        // This is the production Class(bits) case, not a pending marker paired
        // with a stub None class. The existing publication admission fails;
        // the real preserving hook restores the original exception owner.
        crate::record_exception(py, exception_ptr);
        let (_, attempts) = GLOBAL_BRIDGE.with_denied_publication_for_test(class.bits(), || {
            assert!(matches!(
                unsafe { hook_pending_exception_class() },
                PendingExceptionClass::Class(bits) if bits == class.bits()
            ));
            assert!(unsafe { errors::PyErr_Occurred() }.is_null());
            assert!(crate::exception_pending(py));
            assert_eq!(crate::exception_last_bits_noinc(py), Some(exception.bits()));
            assert!(errors::take_current_error().is_none());
            assert_eq!(refs(), before + 1);
            assert_cold_class();
        });
        assert!(
            attempts > 0,
            "the cold class entered actual publication admission"
        );
        crate::clear_exception(py);
        assert_eq!(refs(), before);

        unsafe {
            let index = refcount::OwnedPyObject::from_owned(numbers::PyLong_FromLong(41));
            let float = refcount::OwnedPyObject::from_owned(numbers::PyFloat_FromDouble(2.5));
            assert!(!index.as_ptr().is_null() && !float.as_ptr().is_null());
            let mut slots: PyNumberMethods = std::mem::zeroed();
            slots.nb_index = class_projection_index as *const () as *mut std::ffi::c_void;
            slots.nb_float = class_projection_float as *const () as *mut std::ffi::c_void;
            let mut kind = NativeType::subtype(
                &raw mut molt_cpython_abi::abi_types::PyBaseObject_Type,
                c"ClassProjectionNumeric",
            );
            kind.tp_as_number = (&raw mut slots).cast();
            assert_eq!(kind.ready(), 0);
            let mut value = ClassProjectionNumeric {
                object: PyObject {
                    ob_refcnt: 1,
                    ob_type: &raw mut *kind,
                },
                error: exception.bits(),
                fail: true,
                calls: 0,
                index: index.as_ptr(),
                float: float.as_ptr(),
            };
            let operand = (&raw mut value).cast::<PyObject>();
            let mut long_field: std::os::raw::c_long = 79;
            let mut float_field = 83.0f64;
            // CPython public structmember codes T_LONG=2 and T_DOUBLE=4.
            let mut member = PyMemberDef {
                name: c"field".as_ptr(),
                type_: 2,
                offset: 0,
                flags: 0,
                doc: ptr::null(),
            };
            let (_, attempts) =
                GLOBAL_BRIDGE.with_denied_publication_for_test(class.bits(), || {
                    assert_eq!(
                        transaction.with_target_python_minor(py, 13, || {
                            typeobj::PyMember_SetOne(
                                (&raw mut long_field).cast(),
                                &raw mut member,
                                operand,
                            )
                        }),
                        -1
                    );
                    assert_eq!(long_field, 79);
                    assert_eq!(crate::exception_last_bits_noinc(py), Some(exception.bits()));
                    assert_eq!(refs(), before + 1);
                    crate::clear_exception(py);
                    member.type_ = 4;
                    assert_eq!(
                        transaction.with_target_python_minor(py, 13, || {
                            typeobj::PyMember_SetOne(
                                (&raw mut float_field).cast(),
                                &raw mut member,
                                operand,
                            )
                        }),
                        -1
                    );
                    assert_eq!(float_field, 83.0);
                    assert_eq!(crate::exception_last_bits_noinc(py), Some(exception.bits()));
                    assert_eq!(refs(), before + 1);
                    crate::clear_exception(py);
                    for minor in [13, 14] {
                        transaction.with_target_python_minor(py, minor, || {
                            assert_eq!(numbers::PyComplex_ImagAsDouble(operand), -1.0);
                        });
                        assert_eq!(crate::exception_last_bits_noinc(py), Some(exception.bits()));
                        assert_eq!(refs(), before + 1);
                        crate::clear_exception(py);
                    }
                    // An explicit type observation proves the selected class is
                    // still unpublishable even if repaired consumers never query it.
                    crate::record_exception(py, exception_ptr);
                    assert!(errors::PyErr_Occurred().is_null());
                    assert_eq!(crate::exception_last_bits_noinc(py), Some(exception.bits()));
                    crate::clear_exception(py);
                    assert_cold_class();
                });
            assert!(attempts > 0);
            assert_eq!(value.calls, 4);
            assert_eq!(value.object.ob_refcnt, 1);
            assert_eq!(refs(), before);
            assert_eq!(
                (*header_from_obj_ptr(class_ptr)).ref_count_snapshot(),
                class_refs
            );

            // Removing denial makes the real class observable; conversion
            // recovery also uses the same actual callbacks and output owners.
            crate::record_exception(py, exception_ptr);
            let projected_class = errors::PyErr_Occurred();
            assert!(!projected_class.is_null());
            assert_eq!(
                GLOBAL_BRIDGE
                    .molt_handle_for_pyobj(projected_class)
                    .unwrap()
                    .bits(),
                class.bits()
            );
            assert_eq!(crate::exception_last_bits_noinc(py), Some(exception.bits()));
            crate::clear_exception(py);
            value.fail = false;
            member.type_ = 2;
            assert_eq!(
                typeobj::PyMember_SetOne((&raw mut long_field).cast(), &raw mut member, operand),
                0
            );
            assert_eq!(long_field, 41);
            member.type_ = 4;
            assert_eq!(
                typeobj::PyMember_SetOne((&raw mut float_field).cast(), &raw mut member, operand),
                0
            );
            assert_eq!(float_field, 2.5);
            for minor in [13, 14] {
                transaction.with_target_python_minor(py, minor, || {
                    assert_eq!(numbers::PyComplex_ImagAsDouble(operand), 0.0);
                });
            }
            assert_eq!(value.calls, 8);
            assert_eq!(value.object.ob_refcnt, 1);
            assert_eq!(refs(), before);
            assert!(!crate::exception_pending(py));
            assert!(errors::PyErr_Occurred().is_null());
        }
    });
}

#[test]
fn pending_cleanup_restores_both_channels_through_nested_callbacks() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let original = exception_bits(py, "ValueError");
        let runtime = exception_bits(py, "LookupError");
        let cleanup = exception_bits(py, "TypeError");
        let handled = exception_bits(py, "RuntimeError");
        unsafe {
            let original_view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(original);
            let cleanup_view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(cleanup);
            let handled_view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(handled);
            assert!(!original_view.is_null() && !cleanup_view.is_null() && !handled_view.is_null());
            errors::PyErr_SetHandledException(handled_view);
            set_c_error(original_view);
            crate::test_support::restore_runtime_raised(py, runtime);
            let result = errors::with_preserved_error(|| {
                assert!(errors::PyErr_Occurred().is_null());
                assert!(!crate::exception_pending(py));
                assert_eq!(
                    crate::builtins::exceptions::exception_context_active_bits(),
                    Some(handled)
                );
                set_c_error(cleanup_view);
                crate::test_support::restore_runtime_raised(py, handled);
                errors::with_preserved_error(|| {
                    assert!(errors::PyErr_Occurred().is_null());
                    assert!(!crate::exception_pending(py));
                    set_c_error(original_view);
                    crate::test_support::restore_runtime_raised(py, runtime);
                });
                assert_eq!(crate::exception_last_bits_noinc(py), Some(handled));
                let restored = errors::PyErr_GetRaisedException();
                let restored_owner = refcount::OwnedPyObject::from_owned(restored);
                assert_eq!(restored, cleanup_view);
                drop(restored_owner);
                crate::record_memory_error_without_allocation(py);
                "restored"
            });
            assert_eq!(result, "restored");
            assert_eq!(crate::exception_last_bits_noinc(py), Some(runtime));
            let restored = errors::PyErr_GetRaisedException();
            let restored_owner = refcount::OwnedPyObject::from_owned(restored);
            assert_eq!(restored, original_view);
            drop(restored_owner);
            assert!(!crate::exception_pending(py));
            assert_eq!(
                crate::builtins::exceptions::exception_context_active_bits(),
                Some(handled)
            );
            errors::PyErr_SetHandledException(ptr::null_mut());
        }
        for bits in [original, runtime, cleanup, handled] {
            dec_ref_bits(py, bits);
        }
    });
}

#[repr(C)]
struct RetiringSequenceInput {
    object: PyObject,
    runtime_error: u64,
    c_error: *mut PyObject,
}

thread_local! {
    static SEQUENCE_INPUT_DROPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

unsafe extern "C" fn sequence_input_drop(object: *mut PyObject) {
    let input = unsafe { Box::from_raw(object.cast::<RetiringSequenceInput>()) };
    SEQUENCE_INPUT_DROPS.with(|count| count.set(count.get() + 1));
    crate::with_gil_entry_nopanic!(py, {
        unsafe { set_c_error(input.c_error) };
        crate::test_support::restore_runtime_raised(py, input.runtime_error);
    });
}

#[test]
fn explicit_cleanup_preserves_both_errors_through_sequence_retirement_and_item_transfer() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        use super::native_test_fixture::NativeType;
        use crate::object::seq_access::{pin_item, pin_tuple, snapshot};
        let runtime = exception_bits(py, "LookupError");
        let original = exception_bits(py, "ValueError");
        let cleanup = exception_bits(py, "TypeError");
        unsafe {
            let original_view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(original);
            let cleanup_view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(cleanup);
            assert!(!original_view.is_null() && !cleanup_view.is_null());
            let mut kind = NativeType::subtype(
                &raw mut molt_cpython_abi::abi_types::PyBaseObject_Type,
                c"RetiringSequenceInput",
            );
            kind.tp_dealloc = Some(sequence_input_drop);
            assert_eq!(kind.ready(), 0);
            for mode in 0..4 {
                SEQUENCE_INPUT_DROPS.with(|count| count.set(0));
                let input = Box::into_raw(Box::new(RetiringSequenceInput {
                    object: PyObject {
                        ob_refcnt: 1,
                        ob_type: &raw mut *kind,
                    },
                    runtime_error: cleanup,
                    c_error: cleanup_view,
                }))
                .cast::<PyObject>();
                let bits = GLOBAL_BRIDGE.molt_value_for_pyobj(input).unwrap();
                refcount::Py_DECREF(input);
                let tuple = alloc_tuple(py, &[bits]);
                assert!(!tuple.is_null());
                dec_ref_bits(py, bits);
                let tuple_owner = (mode == 0).then(|| pin_tuple(py, tuple).unwrap());
                let item_owner = (mode == 1 || mode == 3).then(|| pin_item(py, tuple, 0).unwrap());
                let snapshot_owner =
                    (mode == 2).then(|| snapshot(py, tuple, "sequence owner test").unwrap());
                dec_ref_bits(py, MoltObject::from_ptr(tuple).bits());
                assert_eq!(SEQUENCE_INPUT_DROPS.with(|count| count.get()), 0);
                set_c_error(original_view);
                crate::test_support::restore_runtime_raised(py, runtime);
                if mode == 3 {
                    let transferred = item_owner.unwrap().into_bits();
                    assert_eq!(
                        SEQUENCE_INPUT_DROPS.with(|count| count.get()),
                        0,
                        "into_bits transfers custody without retiring the item"
                    );
                    assert_eq!(crate::exception_last_bits_noinc(py), Some(runtime));
                    errors::with_preserved_error(|| dec_ref_bits(py, transferred));
                } else {
                    errors::with_preserved_error(|| drop(item_owner));
                }
                errors::with_preserved_error(|| drop(tuple_owner));
                errors::with_preserved_error(|| drop(snapshot_owner));
                assert_eq!(
                    SEQUENCE_INPUT_DROPS.with(|count| count.get()),
                    1,
                    "the shared owner must really reach the foreign deallocator"
                );
                assert_eq!(
                    crate::exception_last_bits_noinc(py),
                    Some(runtime),
                    "mode {mode}"
                );
                let restored = errors::take_current_error().expect("exact outer C error");
                assert_eq!(restored.value, original_view, "mode {mode}");
                errors::with_preserved_error(|| drop(restored));
                crate::clear_exception(py);
            }
        }
        for bits in [runtime, original, cleanup] {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn pending_cleanup_restores_both_channels_before_resuming_unwind() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let original = exception_bits(py, "ValueError");
        let runtime = exception_bits(py, "LookupError");
        let cleanup = exception_bits(py, "TypeError");
        unsafe {
            let original_view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(original);
            let cleanup_view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(cleanup);
            assert!(!original_view.is_null() && !cleanup_view.is_null());
            errors::PyErr_SetHandledException(cleanup_view);
            set_c_error(original_view);
            crate::test_support::restore_runtime_raised(py, runtime);
            let unwind = crate::test_support::catch_expected_unwind(|| {
                errors::with_preserved_error(|| {
                    assert!(errors::PyErr_Occurred().is_null());
                    assert!(!crate::exception_pending(py));
                    set_c_error(cleanup_view);
                    crate::record_memory_error_without_allocation(py);
                    panic!("cleanup panic must resume after both channels are restored");
                });
            });
            assert!(unwind.is_err());
            assert_eq!(crate::exception_last_bits_noinc(py), Some(runtime));
            let restored = errors::PyErr_GetRaisedException();
            let restored_owner = refcount::OwnedPyObject::from_owned(restored);
            assert_eq!(restored, original_view);
            drop(restored_owner);
            assert!(!crate::exception_pending(py));
            assert_eq!(
                crate::builtins::exceptions::exception_context_active_bits(),
                Some(cleanup)
            );
            errors::PyErr_SetHandledException(ptr::null_mut());
        }
        for bits in [original, runtime, cleanup] {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn pending_cleanup_and_native_snapshot_preserve_emergency_memory_error() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let original = exception_bits(py, "ValueError");
        let original_view = unsafe { GLOBAL_BRIDGE.handle_to_borrowed_pyobj(original) };
        assert!(!original_view.is_null());
        // This restoration test deliberately holds both domains. A real new
        // allocation failure replaces either prior, so construct the two saved
        // states independently and combine them through the restore authority.
        crate::record_memory_error_without_allocation(py);
        let mut emergency = Some(take_raised(py));
        unsafe { set_c_error(original_view) };
        resolve_raised(py, &mut emergency);
        let denied = DenyRuntimeAllocations::enter();
        errors::with_preserved_error(|| {
            assert!(errors::take_current_error().is_none());
            assert!(!crate::exception_pending(py));
            crate::record_memory_error_without_allocation(py);
        });
        assert_emergency_pending(py);
        let pending = take_native_pending_snapshot();
        assert!(pending.has_error());
        assert!(
            matches!(pending.runtime_error, Some(RaisedSnapshot::Thread(bits))
            if crate::builtins::exceptions::exception_matches_builtin_name(py, bits, "MemoryError"))
        );
        assert_eq!(pending.c_error.as_ref().unwrap().value, original_view);
        assert!(!crate::exception_pending(py));
        assert!(errors::take_current_error().is_none());
        crate::record_memory_error_without_allocation(py);
        restore_native_pending_snapshot(pending);
        assert_emergency_pending(py);
        drop(denied);
        unsafe {
            let restored = errors::PyErr_GetRaisedException();
            let restored_owner = refcount::OwnedPyObject::from_owned(restored);
            assert_eq!(restored, original_view);
            drop(restored_owner);
            assert!(!crate::exception_pending(py));
        }
        dec_ref_bits(py, original);
    });
}

unsafe fn assert_c_emergency_is_observable_and_not_consumed(py: &crate::PyToken<'_>) {
    use molt_cpython_abi::abi_types::{PyExc_Exception, PyExc_MemoryError};
    unsafe {
        assert_eq!(
            errors::PyErr_Occurred(),
            (&raw mut PyExc_MemoryError).cast()
        );
        assert_eq!(
            errors::PyErr_ExceptionMatches((&raw mut PyExc_Exception).cast()),
            1
        );
        let (mut kind, mut value, mut traceback) =
            (ptr::null_mut(), ptr::null_mut(), ptr::null_mut());
        errors::PyErr_Fetch(&raw mut kind, &raw mut value, &raw mut traceback);
        let kind_owner = refcount::OwnedPyObject::from_owned(kind);
        let value_owner = refcount::OwnedPyObject::from_owned(value);
        let traceback_owner = refcount::OwnedPyObject::from_owned(traceback);
        assert_eq!(kind, (&raw mut PyExc_MemoryError).cast());
        assert!(
            !value.is_null(),
            "Fetch transfers the prepared reserved instance"
        );
        assert_eq!((*value).ob_type, &raw mut PyExc_MemoryError);
        assert!(traceback.is_null());
        assert!(errors::PyErr_Occurred().is_null());
        errors::PyErr_Restore(
            kind_owner.into_ptr(),
            value_owner.into_ptr(),
            traceback_owner.into_ptr(),
        );
        let raised = refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
        assert_eq!(
            raised.as_ptr(),
            value,
            "take/fetch round trip preserves exact identity"
        );
        errors::PyErr_SetRaisedException(raised.into_ptr());
    }
    crate::cpython_abi_hooks::transfer_pending_cpython_exception();
    assert_emergency_pending(py);
}

#[repr(C)]
struct PendingCallback {
    object: PyObject,
    raise: bool,
    succeed: bool,
    value: *mut PyObject,
    result: *mut PyObject,
}

unsafe extern "C" fn pending_callback_call(
    callable: *mut PyObject,
    args: *mut PyObject,
    _kwargs: *mut PyObject,
) -> *mut PyObject {
    use molt_cpython_abi::abi_types::PyBaseExceptionObject;
    use molt_cpython_abi::api::sequences;
    let call = unsafe { &*callable.cast::<PendingCallback>() };
    let invalid = unsafe { sequences::PyTuple_GetItem(args, 0) }.cast::<PyBaseExceptionObject>();
    let target = unsafe { sequences::PyTuple_GetItem(args, 1) }.cast::<PyBaseExceptionObject>();
    unsafe {
        (*invalid).suppress_context = 2;
        refcount::Py_INCREF(call.value);
        let old = std::mem::replace(&mut (*target).notes, call.value);
        refcount::Py_XDECREF(old);
    }
    if call.raise {
        with_gil(|py| crate::record_memory_error_without_allocation(&py));
    }
    if call.succeed {
        unsafe { refcount::Py_INCREF(call.result) };
        call.result
    } else {
        ptr::null_mut()
    }
}

unsafe extern "C" fn pending_callback_set(
    receiver: *mut PyObject,
    _name: *mut PyObject,
    _value: *mut PyObject,
) -> c_int {
    with_gil(|py| crate::record_memory_error_without_allocation(&py));
    if unsafe { (*receiver.cast::<PendingCallback>()).succeed } {
        0
    } else {
        -1
    }
}

#[test]
fn pending_callback_completion_preserves_emergency_and_publishes_other_operands() {
    use molt_cpython_abi::abi_types::PyBaseExceptionObject;
    use molt_cpython_abi::api::{object, sequences, strings};
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let targets = [
            exception_bits(py, "AttributeError"),
            exception_bits(py, "AttributeError"),
        ];
        let views = targets.map(|bits| unsafe { GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(bits) });
        assert!(views.iter().all(|view| !view.is_null()));
        let args = unsafe { sequences::PyTuple_FromArray(views.as_ptr(), views.len() as isize) };
        let name = unsafe { strings::PyUnicode_FromString(c"field".as_ptr()) };
        let value_bits = MoltObject::from_int(1979).bits();
        let value = unsafe { GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(value_bits) };
        assert!(!args.is_null() && !name.is_null() && !value.is_null());
        let mut kind: PyTypeObject = unsafe { std::mem::zeroed() };
        kind.ob_base.ob_base.ob_refcnt = 1;
        kind.tp_name = c"PendingCallback".as_ptr();
        kind.tp_call = Some(pending_callback_call);
        kind.tp_setattro = Some(pending_callback_set);
        let mut result = PyObject {
            ob_refcnt: 1,
            ob_type: ptr::null_mut(),
        };
        let mut call = PendingCallback {
            object: PyObject {
                ob_refcnt: 1,
                ob_type: &raw mut kind,
            },
            raise: false,
            succeed: true,
            value,
            result: &raw mut result,
        };
        // First publication failure, callback failure, and malformed success
        // must all preserve emergency state and publish the later operand.
        for (index, (raise, succeed)) in [(false, true), (true, false), (true, true)]
            .into_iter()
            .enumerate()
        {
            call.raise = raise;
            call.succeed = succeed;
            let published_bits = MoltObject::from_int(2000 + index as i64).bits();
            let published = unsafe { GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(published_bits) };
            assert!(!published.is_null());
            call.value = published;
            let denied = DenyRuntimeAllocations::enter();
            assert!(
                unsafe { object::PyObject_Call(&raw mut call.object, args, ptr::null_mut()) }
                    .is_null()
            );
            assert_eq!(
                result.ob_refcnt, 1,
                "rejected successful result must be released"
            );
            assert_eq!(
                unsafe {
                    crate::exception_notes_bits(crate::obj_from_bits(targets[1]).as_ptr().unwrap())
                },
                published_bits
            );
            unsafe { assert_c_emergency_is_observable_and_not_consumed(py) };
            unsafe {
                (*views[0].cast::<PyBaseExceptionObject>()).suppress_context = 0;
                errors::PyErr_Clear();
            }
            assert!(!crate::exception_pending(py));
            assert!(unsafe { errors::PyErr_Occurred() }.is_null());
            drop(denied);
            unsafe { refcount::Py_DECREF(published) };
        }
        // Direct status slots forward their status; they do not apply the
        // generic-call result/error consistency check exercised above.
        for succeed in [false, true] {
            call.succeed = succeed;
            let denied = DenyRuntimeAllocations::enter();
            assert_eq!(
                unsafe { object::PyObject_SetAttr(&raw mut call.object, name, value) },
                if succeed { 0 } else { -1 }
            );
            unsafe {
                assert_c_emergency_is_observable_and_not_consumed(py);
                errors::PyErr_Clear();
            }
            assert!(!crate::exception_pending(py));
            drop(denied);
        }
        assert_eq!(call.object.ob_refcnt, 1);
        unsafe {
            for pointer in [args, name, value, views[0], views[1]] {
                refcount::Py_DECREF(pointer);
            }
        }
        for bits in targets {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn core_protected_value_preserves_both_errors_during_native_retirement() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        use super::native_test_fixture::NativeType;
        let runtime = exception_bits(py, "LookupError");
        let original = exception_bits(py, "ValueError");
        let cleanup = exception_bits(py, "TypeError");
        unsafe {
            let original_view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(original);
            let cleanup_view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(cleanup);
            let mut kind = NativeType::subtype(
                &raw mut molt_cpython_abi::abi_types::PyBaseObject_Type,
                c"OwnedIteratorNativeRetirement",
            );
            kind.tp_dealloc = Some(sequence_input_drop);
            assert_eq!(kind.ready(), 0);
            for transfer in [false, true] {
                SEQUENCE_INPUT_DROPS.with(|count| count.set(0));
                let input = Box::into_raw(Box::new(RetiringSequenceInput {
                    object: PyObject {
                        ob_refcnt: 1,
                        ob_type: &raw mut *kind,
                    },
                    runtime_error: cleanup,
                    c_error: cleanup_view,
                }))
                .cast::<PyObject>();
                let bits = GLOBAL_BRIDGE.molt_value_for_pyobj(input).unwrap();
                refcount::Py_DECREF(input);
                let owner = molt_runtime_core::ProtectedRuntimeValue::from_owned_bits(
                    py.core_token(),
                    bits,
                );
                set_c_error(original_view);
                crate::test_support::restore_runtime_raised(py, runtime);
                if transfer {
                    let bits = owner.into_bits();
                    assert_eq!(SEQUENCE_INPUT_DROPS.with(|count| count.get()), 0);
                    molt_runtime_core::ffi::__molt_runtime_release_protected_value(bits);
                } else {
                    drop(owner);
                }
                assert_eq!(SEQUENCE_INPUT_DROPS.with(|count| count.get()), 1);
                assert_eq!(crate::exception_last_bits_noinc(py), Some(runtime));
                let restored = errors::take_current_error().expect("exact outer C error");
                assert_eq!(restored.value, original_view);
                errors::with_preserved_error(|| drop(restored));
                crate::clear_exception(py);
            }
        }
        for bits in [runtime, original, cleanup] {
            dec_ref_bits(py, bits);
        }
    });
}

#[derive(Clone, Copy, Default)]
struct RetirementObservation {
    drops: usize,
    releases: usize,
    error: usize,
    traceback: usize,
    release_error: usize,
    release_traceback: usize,
}

thread_local! {
    static RETIREMENT_OBSERVATION: std::cell::Cell<RetirementObservation> =
        std::cell::Cell::new(RetirementObservation::default());
    static RETIREMENT_HOSTILE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static RETIREMENT_GET_RESULT: std::cell::Cell<Option<(usize, c_int)>> = const {
        std::cell::Cell::new(None)
    };
}

#[repr(C)]
struct RetirementProbe {
    object: PyObject,
    byte: u8,
}

unsafe fn observe_retirement_error(release: bool) {
    unsafe {
        let error = errors::PyErr_GetRaisedException();
        let trace = if error.is_null() {
            ptr::null_mut()
        } else {
            errors::PyException_GetTraceback(error)
        };
        RETIREMENT_OBSERVATION.with(|cell| {
            let mut observed = cell.get();
            if release {
                observed.releases += 1;
                observed.release_error = error.addr();
                observed.release_traceback = trace.addr();
            } else {
                observed.drops += 1;
                observed.error = error.addr();
                observed.traceback = trace.addr();
            }
            cell.set(observed);
        });
        refcount::Py_XDECREF(trace);
        errors::PyErr_SetRaisedException(error);
        if !release && RETIREMENT_HOSTILE.with(std::cell::Cell::get) {
            errors::PyErr_SetString(
                (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast(),
                c"explicit protected cleanup".as_ptr(),
            );
        }
    }
}

unsafe extern "C" fn retirement_probe_drop(object: *mut PyObject) {
    unsafe {
        observe_retirement_error(false);
    }
    drop(unsafe { Box::from_raw(object.cast::<RetirementProbe>()) });
}

unsafe extern "C" fn retirement_probe_getbuffer(
    object: *mut PyObject,
    view: *mut molt_cpython_abi::abi_types::Py_buffer,
    flags: c_int,
) -> c_int {
    unsafe {
        molt_cpython_abi::api::buffer::PyBuffer_FillInfo(
            view,
            object,
            (&raw mut (*object.cast::<RetirementProbe>()).byte).cast(),
            1,
            0,
            flags,
        )
    }
}

// The callback removes the fixture's original edge. Acquisition's receiver pin
// is therefore the last owner on failure; no fixture guard masks its retirement.
unsafe extern "C" fn retirement_probe_failed_getbuffer(
    object: *mut PyObject,
    view: *mut molt_cpython_abi::abi_types::Py_buffer,
    flags: c_int,
) -> c_int {
    unsafe {
        let Some((error, status)) = RETIREMENT_GET_RESULT.with(std::cell::Cell::get) else {
            return -1;
        };
        if status >= 0 && retirement_probe_getbuffer(object, view, flags) < 0 {
            refcount::Py_DECREF(object);
            return -1;
        }
        refcount::Py_DECREF(object);
        errors::PyErr_SetRaisedException(molt_cpython_abi::api::object::Py_NewRef(
            std::ptr::with_exposed_provenance_mut(error),
        ));
        status
    }
}

unsafe extern "C" fn retirement_probe_releasebuffer(
    _object: *mut PyObject,
    _view: *mut molt_cpython_abi::abi_types::Py_buffer,
) {
    unsafe {
        observe_retirement_error(true);
    }
}

#[test]
fn static_native_owner_and_buffer_policies_preserve_their_actual_error_boundary() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        use super::native_test_fixture::NativeType;
        use crate::object::foreign::{NativeObjectOwner, NativeReferenceRetirement};
        use molt_cpython_abi::{
            abi_types,
            api::{buffer, memory, object},
        };
        let failure = exception_bits(py, "LookupError");
        let _failure =
            crate::PtrDropGuard::preserving(crate::obj_from_bits(failure).as_ptr().unwrap());
        let failure_view = unsafe { GLOBAL_BRIDGE.handle_to_borrowed_pyobj(failure) };
        assert!(!failure_view.is_null());
        let trace_class = crate::obj_from_bits(crate::builtin_classes(py).traceback)
            .as_ptr()
            .unwrap();
        let trace_bits = unsafe { crate::alloc_instance_for_class(py, trace_class) };
        let traceback = unsafe {
            refcount::ProtectedPyObject::from_owned(GLOBAL_BRIDGE.owned_handle_to_pyobj(trace_bits))
        };
        assert!(!traceback.as_ptr().is_null());
        assert_eq!(
            unsafe { errors::PyException_SetTraceback(failure_view, traceback.as_ptr()) },
            0
        );
        let mut kind = NativeType::subtype(
            &raw mut abi_types::PyBaseObject_Type,
            c"RetirementPolicyProbe",
        );
        kind.tp_dealloc = Some(retirement_probe_drop);
        let mut slots = abi_types::PyBufferProcs {
            bf_getbuffer: retirement_probe_getbuffer as *const () as *mut _,
            bf_releasebuffer: retirement_probe_releasebuffer as *const () as *mut _,
        };
        kind.tp_as_buffer = (&raw mut slots).cast();
        assert_eq!(unsafe { kind.ready() }, 0);
        struct HostileReset;
        impl Drop for HostileReset {
            fn drop(&mut self) {
                RETIREMENT_HOSTILE.with(|cell| cell.set(false));
                RETIREMENT_GET_RESULT.with(|cell| cell.set(None));
            }
        }
        let _reset = HostileReset;
        for lease in [false, true] {
            for policy in 0..3 {
                RETIREMENT_OBSERVATION.with(|cell| cell.set(RetirementObservation::default()));
                RETIREMENT_HOSTILE.with(|cell| cell.set(false));
                let object = Box::into_raw(Box::new(RetirementProbe {
                    object: PyObject {
                        ob_refcnt: 1,
                        ob_type: &raw mut *kind,
                    },
                    byte: 42,
                }))
                .cast::<PyObject>();
                // Acquire and clone before publishing the operation error. The
                // final selected owner is the only remaining exporter edge.
                let source = if lease {
                    let object = unsafe { refcount::OwnedPyObject::from_owned(object) };
                    let source = unsafe {
                        memory::MemoryViewLease::acquire(
                            object.as_ptr(),
                            0,
                            buffer::PyObject_GetBuffer,
                        )
                    }
                    .expect("native export");
                    drop(object);
                    Some(source)
                } else {
                    None
                };
                unsafe {
                    errors::PyErr_SetRaisedException(object::Py_NewRef(failure_view));
                }
                if policy != 0 {
                    assert!(transfer_pending_cpython_exception());
                    assert!(crate::exception_pending(py));
                }
                RETIREMENT_HOSTILE.with(|cell| cell.set(policy == 2));
                if let Some(source) = source {
                    match policy {
                        0 => {
                            let selected = source.clone();
                            drop(source);
                            drop(selected);
                        }
                        1 => {
                            let selected = source.clone_with_policy::<NativeReferenceRetirement>();
                            drop(source);
                            drop(selected);
                        }
                        _ => {
                            let selected =
                                source.clone_with_policy::<refcount::ProtectedReferenceRelease>();
                            drop(source);
                            drop(selected);
                        }
                    }
                } else {
                    unsafe {
                        match policy {
                            0 => {
                                let owner = refcount::OwnedPyObject::from_owned(object);
                                let transferred = owner.into_ptr();
                                assert_eq!(RETIREMENT_OBSERVATION.with(|cell| cell.get().drops), 0);
                                drop(refcount::OwnedPyObject::from_owned(transferred));
                            }
                            1 => drop(NativeObjectOwner::from_owned(object)),
                            _ => drop(refcount::ProtectedPyObject::from_owned(object)),
                        }
                    }
                }
                RETIREMENT_HOSTILE.with(|cell| cell.set(false));
                let observed = RETIREMENT_OBSERVATION.with(std::cell::Cell::get);
                assert_eq!(observed.drops, 1, "lease={lease} policy={policy}");
                assert_eq!(observed.releases, usize::from(lease));
                assert_eq!(
                    observed.error,
                    if policy == 2 { 0 } else { failure_view.addr() }
                );
                assert_eq!(
                    observed.traceback,
                    if policy == 2 {
                        0
                    } else {
                        traceback.as_ptr().addr()
                    }
                );
                if lease {
                    assert_eq!(observed.release_error, observed.error);
                    assert_eq!(observed.release_traceback, observed.traceback);
                }
                assert_eq!(crate::exception_pending(py), policy != 0);
                let restored = unsafe {
                    refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException())
                };
                assert_eq!(restored.as_ptr(), failure_view);
                let trace = unsafe {
                    refcount::OwnedPyObject::from_owned(errors::PyException_GetTraceback(
                        restored.as_ptr(),
                    ))
                };
                assert_eq!(trace.as_ptr(), traceback.as_ptr());
                assert!(!crate::exception_pending(py));
                assert!(unsafe { errors::PyErr_Occurred().is_null() });
            }
        }

        fn release_acquisition<P: refcount::ReferenceRetirement>(
            acquired: Result<memory::BufferLease<P>, memory::BufferAcquireError>,
            receiver: *mut PyObject,
            status: c_int,
        ) {
            match acquired {
                Err(stage) => {
                    assert_eq!(stage, memory::BufferAcquireError::Exporter);
                    assert_eq!(status, -1, "only a negative callback status rejects");
                }
                Ok(lease) => {
                    assert_eq!(status, 0);
                    assert_eq!(lease.owner(), receiver);
                    let observed = RETIREMENT_OBSERVATION.with(std::cell::Cell::get);
                    assert_eq!(observed.drops, 0, "the live export still owns the receiver");
                    assert_eq!(
                        observed.releases, 0,
                        "acquisition must not release an export"
                    );
                    unsafe {
                        assert_eq!((*receiver).ob_refcnt, 1, "the export is the last owner");
                        assert_eq!((*lease.descriptor()).len, 1);
                        assert_eq!(*(*lease.descriptor()).buf.cast::<u8>(), 42);
                    }
                    drop(lease);
                }
            }
        }

        // This passes the raw callback directly to the lease authority. The
        // zero-plus-error row proves explicit status forwarding, not valid
        // callback behavior under CPython's debug-only GetBuffer assertion.
        // Public clean acquisition and release-under-error are covered above.
        for runtime in [false, true] {
            for status in [-1, 0] {
                RETIREMENT_OBSERVATION.with(|cell| cell.set(RetirementObservation::default()));
                RETIREMENT_GET_RESULT.with(|cell| cell.set(Some((failure_view.addr(), status))));
                let receiver = Box::into_raw(Box::new(RetirementProbe {
                    object: PyObject {
                        ob_refcnt: 1,
                        ob_type: &raw mut *kind,
                    },
                    byte: 42,
                }))
                .cast::<PyObject>();
                if runtime {
                    let acquired = unsafe {
                        crate::object::foreign::RuntimeBufferLease::acquire(
                            receiver,
                            0,
                            retirement_probe_failed_getbuffer,
                        )
                    };
                    release_acquisition(acquired, receiver, status);
                } else {
                    let acquired = unsafe {
                        memory::MemoryViewLease::acquire(
                            receiver,
                            0,
                            retirement_probe_failed_getbuffer,
                        )
                    };
                    release_acquisition(acquired, receiver, status);
                }
                RETIREMENT_GET_RESULT.with(|cell| cell.set(None));
                let observed = RETIREMENT_OBSERVATION.with(std::cell::Cell::get);
                assert_eq!(observed.drops, 1, "receiver retires after its last owner");
                assert_eq!(observed.releases, usize::from(status >= 0));
                assert_eq!(crate::exception_pending(py), runtime);
                let raised = unsafe {
                    refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException())
                };
                assert_eq!(raised.as_ptr(), failure_view);
                assert_eq!(observed.error, raised.as_ptr().addr());
                assert_eq!(observed.traceback, traceback.as_ptr().addr());
                if status >= 0 {
                    assert_eq!(observed.release_error, observed.error);
                    assert_eq!(observed.release_traceback, observed.traceback);
                }
                assert!(unsafe { errors::PyErr_Occurred().is_null() });
            }
        }
        crate::record_exception(py, crate::obj_from_bits(failure).as_ptr().unwrap());
        unsafe {
            drop(NativeObjectOwner::from_owned(ptr::null_mut()));
        }
        assert_eq!(
            crate::exception_last_bits_noinc(py),
            Some(failure),
            "NULL is an empty owner"
        );
        crate::clear_exception(py);
    });
}

#[test]
fn cold_type_publication_preserves_prior_on_success_and_selects_real_failure() {
    use crate::builtins::exceptions::ExceptionValue;
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        for emergency in [false, true] {
            for fail in [false, true] {
                let prior = ExceptionValue::adopt(py, exception_bits(py, "LookupError"));
                let name = ExceptionValue::adopt(
                    py,
                    crate::attr_name_bits_from_bytes(py, b"ColdPendingPublication").unwrap(),
                );
                let namespace = crate::alloc_dict_with_pairs(py, &[]);
                assert!(!namespace.is_null());
                let namespace = ExceptionValue::adopt(py, MoltObject::from_ptr(namespace).bits());
                let class = ExceptionValue::adopt(
                    py,
                    crate::builtins::types::molt_type_new(
                        crate::builtin_classes(py).type_obj,
                        name.bits(),
                        crate::builtin_classes(py).object,
                        namespace.bits(),
                        MoltObject::none().bits(),
                    ),
                );
                let class_ptr = crate::obj_from_bits(class.bits())
                    .as_ptr()
                    .expect("managed class");
                assert!(!crate::exception_pending(py));
                assert_eq!(
                    unsafe { (*header_from_obj_ptr(class_ptr)).load_synchronized_flags() }
                        & crate::object::HEADER_FLAG_HAS_ABI_VIEW,
                    0
                );
                if emergency {
                    crate::record_memory_error_without_allocation(py);
                } else {
                    crate::record_exception(
                        py,
                        crate::obj_from_bits(prior.bits()).as_ptr().unwrap(),
                    );
                }
                if fail {
                    let (view, attempts) = GLOBAL_BRIDGE
                        .with_denied_publication_for_test(class.bits(), || unsafe {
                            GLOBAL_BRIDGE.handle_to_borrowed_pyobj(class.bits())
                        });
                    assert!(view.is_null());
                    assert_eq!(attempts, 1, "actual cold allocation admission was reached");
                    assert_eq!(
                        unsafe {
                            errors::PyErr_ExceptionMatches(
                                (&raw mut molt_cpython_abi::abi_types::PyExc_MemoryError).cast(),
                            )
                        },
                        1,
                        "the allocation failure replaces the incoming error"
                    );
                    assert_eq!(
                        unsafe { (*header_from_obj_ptr(class_ptr)).load_synchronized_flags() }
                            & crate::object::HEADER_FLAG_HAS_ABI_VIEW,
                        0
                    );
                } else {
                    let view = unsafe { GLOBAL_BRIDGE.handle_to_borrowed_pyobj(class.bits()) };
                    assert!(!view.is_null());
                    assert_eq!(
                        GLOBAL_BRIDGE.molt_handle_for_pyobj(view).unwrap().bits(),
                        class.bits()
                    );
                    if emergency {
                        assert_emergency_pending(py);
                    } else {
                        assert_eq!(crate::exception_last_bits_noinc(py), Some(prior.bits()));
                    }
                }
                unsafe { errors::PyErr_Clear() };
                assert!(!crate::exception_pending(py));
            }
        }
    });
}

thread_local! {
    static OOM_PRIOR_RETIREMENT: std::cell::Cell<(usize, usize)> = const {
        std::cell::Cell::new((0, 0))
    };
    static OOM_PRIOR_ROUNDTRIP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static OOM_SELECTED_INSTANCE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static OOM_ARRAY_HANDLE: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static OOM_ARRAY_OBSERVATION: std::cell::Cell<(bool, Option<i64>, Option<i64>, usize)> = const {
        std::cell::Cell::new((false, None, None, 0))
    };
    static ARRAY_INDEX_OBSERVATION: std::cell::Cell<(usize, bool)> = const {
        std::cell::Cell::new((0, false))
    };
    static OOM_REENTRY_NAME: std::cell::Cell<*const std::ffi::c_char> = const {
        std::cell::Cell::new(ptr::null())
    };
    static OOM_REENTRY_OBSERVATION: std::cell::Cell<(bool, bool, bool, usize)> = const {
        std::cell::Cell::new((false, false, false, 0))
    };
}

unsafe extern "C" fn oom_prior_payload_drop(object: *mut PyObject) {
    // Observing the selected public class leaves it pending and is valid in
    // tp_dealloc, including when allocation has been denied.
    let selected = unsafe { errors::PyErr_Occurred() }.addr();
    let reentry_name = OOM_REENTRY_NAME.with(std::cell::Cell::get);
    let array = OOM_ARRAY_HANDLE.with(std::cell::Cell::get);
    let array_available =
        array == 0 || crate::builtins::array_mod::array_storage_lock_available_for_test(array);
    let locks_available = array_available
        && (reentry_name.is_null()
            || crate::with_gil_entry_nopanic!(py, {
                crate::object::builders::canonical_cache_locks_available_for_test(py)
            }));
    // Observe lock release before the lawful Get/Set roundtrip and any
    // subsequent callback reentry.
    if locks_available && OOM_PRIOR_ROUNDTRIP.with(std::cell::Cell::get) {
        let error = unsafe { errors::PyErr_GetRaisedException() };
        OOM_SELECTED_INSTANCE.with(|cell| cell.set(error.addr()));
        if !error.is_null() {
            unsafe { errors::PyErr_SetRaisedException(error) };
        }
    }
    if !reentry_name.is_null() {
        // The observation is nonblocking: old code holding either cache lock
        // fails the outer assertion instead of hanging this destructor.
        crate::with_gil_entry_nopanic!(py, {
            let available = locks_available;
            let (text, singleton) = if available {
                let text = unsafe {
                    refcount::OwnedPyObject::from_owned(
                        molt_cpython_abi::api::strings::PyUnicode_FromString(reentry_name),
                    )
                };
                let text_ok = !text.as_ptr().is_null();
                drop(text);
                let singleton = crate::object::builders::canonical_singleton_allocation_probe(py);
                (text_ok, singleton)
            } else {
                (false, false)
            };
            OOM_REENTRY_OBSERVATION.with(|cell| {
                cell.set((
                    available,
                    text,
                    singleton,
                    unsafe { errors::PyErr_Occurred() }.addr(),
                ));
            });
        });
    }
    if array != 0 {
        // Try the actual mutex before reentry. A regression records failure;
        // it cannot hang the test while holding the storage lock.
        let (before, after) = if array_available {
            let before = crate::obj_from_bits(crate::molt_array_len(array)).as_int();
            crate::molt_array_append(array, MoltObject::from_int(11).bits());
            let after = crate::obj_from_bits(crate::molt_array_len(array)).as_int();
            (before, after)
        } else {
            (None, None)
        };
        OOM_ARRAY_OBSERVATION.with(|cell| {
            cell.set((
                array_available,
                before,
                after,
                unsafe { errors::PyErr_Occurred() }.addr(),
            ))
        });
    }
    OOM_PRIOR_RETIREMENT.with(|cell| cell.set((cell.get().0 + 1, selected)));
    unsafe { molt_cpython_abi::api::memory::PyObject_Free(object.cast()) };
}

struct RestoreOomObserver;

impl Drop for RestoreOomObserver {
    fn drop(&mut self) {
        OOM_PRIOR_ROUNDTRIP.with(|cell| cell.set(false));
        OOM_REENTRY_NAME.with(|cell| cell.set(ptr::null()));
        OOM_ARRAY_HANDLE.with(|cell| cell.set(0));
        // All declarations outlive this guard, including failed assertions.
        unsafe { errors::PyErr_Clear() };
    }
}

unsafe fn install_oom_prior(kind: *mut PyTypeObject, runtime: bool) -> *mut PyObject {
    use molt_cpython_abi::api::{sequences, typeobj};
    unsafe {
        assert_eq!(
            typeobj::PyType_Ready(&raw mut molt_cpython_abi::abi_types::PyTuple_Type),
            0
        );
        assert_eq!(
            typeobj::PyType_Ready(&raw mut molt_cpython_abi::abi_types::PyExc_ValueError),
            0
        );
        let payload = refcount::OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(kind, 0));
        assert!(!payload.as_ptr().is_null());
        // Keep the C-prior path entirely physical: a managed tuple's final
        // FOREIGN element retirement would itself complete error transport and
        // hide a missing completion at the displaced native indicator boundary.
        let args = refcount::OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(
            &raw mut molt_cpython_abi::abi_types::PyTuple_Type,
            1,
        ));
        assert!(!args.as_ptr().is_null());
        assert!(
            GLOBAL_BRIDGE
                .managed_handle_for_pyobj(args.as_ptr())
                .is_none()
        );
        assert_eq!(
            sequences::PyTuple_SetItem(args.as_ptr(), 0, payload.into_ptr()),
            0
        );
        let prior = refcount::OwnedPyObject::from_owned(errors::molt_native_exception_new(
            &raw mut molt_cpython_abi::abi_types::PyExc_ValueError,
            args.as_ptr(),
            ptr::null_mut(),
        ));
        assert!(!prior.as_ptr().is_null());
        assert!(
            GLOBAL_BRIDGE
                .managed_handle_for_pyobj(prior.as_ptr())
                .is_none()
        );
        drop(args);
        let address = prior.as_ptr();
        errors::PyErr_SetRaisedException(prior.into_ptr());
        if runtime {
            assert!(transfer_pending_cpython_exception());
        }
        assert_eq!(
            errors::PyErr_Occurred(),
            (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast(),
        );
        OOM_PRIOR_RETIREMENT.with(|cell| cell.set((0, 0)));
        address
    }
}

fn assert_oom_prior_retired(prior: usize) {
    let expected = if prior == 0 {
        (0, 0)
    } else {
        (
            1,
            (&raw mut molt_cpython_abi::abi_types::PyExc_MemoryError).addr(),
        )
    };
    assert_eq!(OOM_PRIOR_RETIREMENT.with(std::cell::Cell::get), expected);
}

#[test]
fn semantic_raise_replaces_either_prior_channel_before_native_retirement() {
    use super::native_test_fixture::NativeType;
    use crate::builtins::exceptions::{ExceptionFieldSlot, ExceptionValue, exception_field};
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let mut payload_type = NativeType::subtype(
                &raw mut molt_cpython_abi::abi_types::PyBaseObject_Type,
                c"SemanticRaisePriorPayload",
            );
            payload_type.tp_dealloc = Some(oom_prior_payload_drop);
            assert_eq!(payload_type.ready(), 0);
            let _reset = RestoreOomObserver;
            for prior in 0..3 {
                // Exercise direct recording, a cold explicit raise, and the
                // existing-trace transport branch through their real callers.
                for publication in 0..3 {
                    let selected = ExceptionValue::adopt(py, exception_bits(py, "TypeError"));
                    let view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(selected.bits());
                    assert!(!view.is_null());
                    if publication == 2 {
                        let trace_class =
                            crate::obj_from_bits(crate::builtin_classes(py).traceback)
                                .as_ptr()
                                .unwrap();
                        let trace = crate::alloc_instance_for_class(py, trace_class);
                        let trace = refcount::OwnedPyObject::from_owned(
                            GLOBAL_BRIDGE.owned_handle_to_pyobj(trace),
                        );
                        assert!(!trace.as_ptr().is_null());
                        assert_eq!(errors::PyException_SetTraceback(view, trace.as_ptr()), 0);
                    }
                    OOM_PRIOR_ROUNDTRIP.with(|cell| cell.set(true));
                    OOM_SELECTED_INSTANCE.with(|cell| cell.set(0));
                    if prior == 2 {
                        OOM_PRIOR_RETIREMENT.with(|cell| cell.set((0, 0)));
                        crate::record_memory_error_without_allocation(py);
                    } else {
                        install_oom_prior(&raw mut *payload_type, prior == 1);
                    }
                    if publication == 0 {
                        crate::record_exception(
                            py,
                            crate::obj_from_bits(selected.bits()).as_ptr().unwrap(),
                        );
                    } else {
                        crate::molt_exception_set_last(selected.bits());
                    }
                    assert_eq!(
                        OOM_PRIOR_RETIREMENT.with(std::cell::Cell::get),
                        if prior == 2 {
                            (0, 0)
                        } else {
                            (
                                1,
                                (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).addr(),
                            )
                        },
                        "the sole prior payload retires under the new error"
                    );
                    if prior != 2 {
                        assert_eq!(
                            OOM_SELECTED_INSTANCE.with(std::cell::Cell::get),
                            view.addr()
                        );
                    }
                    assert_eq!(
                        errors::PyErr_Occurred(),
                        (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast()
                    );
                    assert_eq!(crate::exception_last_bits_noinc(py), Some(selected.bits()));
                    assert!(
                        errors::take_current_error().is_none(),
                        "native reentry completed"
                    );
                    assert_eq!(
                        exception_field(py, selected.bits(), ExceptionFieldSlot::Context)
                            .unwrap()
                            .bits(),
                        MoltObject::none().bits(),
                        "a displaced pending error is not a handled exception"
                    );
                    crate::clear_exception(py);
                }
            }
        }
    });
}

#[test]
fn numeric_allocation_failure_uses_the_declared_result_and_keeps_origins_unpublished() {
    use super::native_test_fixture::NativeType;
    use molt_cpython_abi::api::numbers;
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let mut payload_type = NativeType::subtype(
                &raw mut molt_cpython_abi::abi_types::PyBaseObject_Type,
                c"AllocationPriorPayload",
            );
            payload_type.tp_dealloc = Some(oom_prior_payload_drop);
            assert_eq!(payload_type.ready(), 0);
            let _reset = RestoreOomObserver;
            // Clean, C-prior and runtime-prior observations share real sources.
            // The prior exception is the only remaining payload owner at OOM.
            for prior in 0..3 {
                for kind in 0..4 {
                    let source = refcount::OwnedPyObject::from_owned(match kind {
                        0 => numbers::PyLong_FromLong(1000),
                        1 => numbers::PyLong_FromUnsignedLongLong(u64::MAX),
                        2 => numbers::PyFloat_FromDouble(0.0),
                        _ => numbers::PyComplex_FromDoubles(1.25, -2.5),
                    });
                    assert!(!source.as_ptr().is_null());
                    assert!(
                        GLOBAL_BRIDGE
                            .managed_handle_for_pyobj(source.as_ptr())
                            .is_none()
                    );
                    OOM_PRIOR_RETIREMENT.with(|cell| cell.set((0, 0)));
                    if prior != 0 {
                        install_oom_prior(&raw mut *payload_type, prior == 2);
                    }
                    let denied = DenyRuntimeAllocations::enter();
                    let acquired =
                        GLOBAL_BRIDGE
                            .molt_value_for_pyobj(source.as_ptr())
                            .map(|bits| {
                                molt_runtime_core::OwnedRuntimeValue::from_owned_bits(
                                    py.core_token(),
                                    bits,
                                )
                            });
                    assert!(acquired.is_none(), "prior={prior}, source={kind}");
                    assert!(
                        GLOBAL_BRIDGE
                            .managed_handle_for_pyobj(source.as_ptr())
                            .is_none()
                    );
                    assert_eq!((*source.as_ptr()).ob_refcnt, 1);
                    assert_c_emergency_is_observable_and_not_consumed(py);
                    assert_oom_prior_retired(prior);
                    errors::PyErr_Clear();
                    drop(denied);
                    match kind {
                        0 => assert_eq!(numbers::PyLong_AsLong(source.as_ptr()), 1000),
                        1 => assert_eq!(
                            numbers::PyLong_AsUnsignedLongLong(source.as_ptr()),
                            u64::MAX
                        ),
                        2 => assert_eq!(numbers::PyFloat_AsDouble(source.as_ptr()), 0.0),
                        _ => {
                            assert_eq!(numbers::PyComplex_RealAsDouble(source.as_ptr()), 1.25);
                            assert_eq!(numbers::PyComplex_ImagAsDouble(source.as_ptr()), -2.5);
                        }
                    }
                    assert!(errors::PyErr_Occurred().is_null());
                }
                for kind in 0..9 {
                    OOM_PRIOR_RETIREMENT.with(|cell| cell.set((0, 0)));
                    let prior_address = if prior == 0 {
                        ptr::null_mut()
                    } else {
                        install_oom_prior(&raw mut *payload_type, prior == 2)
                    };
                    let denied = DenyRuntimeAllocations::enter();
                    if kind == 0 {
                        // Inline zero succeeds even with either prior selected.
                        assert_eq!(hook_int_from_i64(0), MoltObject::from_int(0).bits());
                        let selected =
                            refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                        assert_eq!(selected.as_ptr(), prior_address);
                        assert_eq!(OOM_PRIOR_RETIREMENT.with(std::cell::Cell::get), (0, 0));
                        drop(selected);
                    } else if kind <= 5 {
                        let bytes = [0xffu8; 17];
                        let digits = [1u8; 129];
                        let result = match kind {
                            1 => hook_int_from_i64(i64::MAX),
                            2 => hook_int_from_u64(u64::MAX),
                            3 => hook_int_from_bytes(bytes.as_ptr(), bytes.len(), 1, 0),
                            4 => hook_int_from_digits(digits.as_ptr(), digits.len(), 2, 0),
                            _ => hook_int_from_f64_trunc(1e100),
                        };
                        let _unexpected = (result != 0).then(|| {
                            molt_runtime_core::OwnedRuntimeValue::from_owned_bits(
                                py.core_token(),
                                result,
                            )
                        });
                        assert_eq!(result, 0, "integer producer {kind}, prior={prior}");
                        assert_c_emergency_is_observable_and_not_consumed(py);
                        assert_oom_prior_retired(prior);
                    } else {
                        let result = match kind {
                            6 => hook_numeric_identity_new(MoltObject::from_int(7).bits()),
                            7 => hook_numeric_identity_new(MoltObject::from_float(0.0).bits()),
                            _ => hook_complex_from_doubles(1.25, -2.5),
                        };
                        let decoded = result.decode();
                        let _unexpected = match &decoded {
                            DecodedHandleResult::Ok(bits) => {
                                Some(molt_runtime_core::OwnedRuntimeValue::from_owned_bits(
                                    py.core_token(),
                                    *bits,
                                ))
                            }
                            _ => None,
                        };
                        assert!(matches!(decoded, DecodedHandleResult::Error));
                        assert_c_emergency_is_observable_and_not_consumed(py);
                        assert_oom_prior_retired(prior);
                    }
                    errors::PyErr_Clear();
                    drop(denied);
                    assert!(!crate::exception_pending(py));
                }
            }
        }
    });
}

#[test]
fn allocation_failure_completes_displaced_c_owner_error_transport() {
    use super::native_test_fixture::NativeType;
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let mut payload_type = NativeType::subtype(
                &raw mut molt_cpython_abi::abi_types::PyBaseObject_Type,
                c"AllocationTransportPayload",
            );
            payload_type.tp_dealloc = Some(oom_prior_payload_drop);
            assert_eq!(payload_type.ready(), 0);
            let _reset = RestoreOomObserver;
            for runtime in [false, true] {
                for tuple in [false, true] {
                    install_oom_prior(&raw mut *payload_type, runtime);
                    OOM_SELECTED_INSTANCE.with(|cell| cell.set(0));
                    OOM_PRIOR_ROUNDTRIP.with(|cell| cell.set(true));
                    // Real extent failure, with memory available for the lawful
                    // old-owner Get/Set of the selected MemoryError.
                    let result = if tuple {
                        let result =
                            crate::object::builders::alloc_tuple_uninitialized(py, usize::MAX);
                        if result.is_null() {
                            0
                        } else {
                            MoltObject::from_ptr(result).bits()
                        }
                    } else {
                        hook_alloc_bytearray(ptr::null(), usize::MAX)
                    };
                    let _unexpected = (result != 0).then(|| {
                        molt_runtime_core::OwnedRuntimeValue::from_owned_bits(
                            py.core_token(),
                            result,
                        )
                    });
                    assert_eq!(result, 0);
                    OOM_PRIOR_ROUNDTRIP.with(|cell| cell.set(false));
                    assert_oom_prior_retired(1);
                    assert!(
                        crate::exception_pending(py),
                        "native retirement must complete its error transport"
                    );
                    assert!(errors::take_current_error().is_none());
                    assert_eq!(
                        errors::PyErr_Occurred(),
                        (&raw mut molt_cpython_abi::abi_types::PyExc_MemoryError).cast()
                    );
                    let selected =
                        refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                    assert!(!selected.as_ptr().is_null());
                    assert_eq!(
                        selected.as_ptr().addr(),
                        OOM_SELECTED_INSTANCE.with(std::cell::Cell::get)
                    );
                    drop(selected);
                    assert!(!crate::exception_pending(py));
                }
            }
        }
    });
}

/// Deny one real resource reservation, then allow the displaced owner's legal
/// callback to allocate. The normal allocator, not this tracker, selects OOM.
struct DenyRuntimeAllocation {
    attempts: std::rc::Rc<std::cell::Cell<usize>>,
    denied_attempt: usize,
}

impl crate::resource::ResourceTracker for DenyRuntimeAllocation {
    fn on_allocate(&mut self, _size: usize) -> Result<(), crate::resource::ResourceError> {
        let attempts = self.attempts.get() + 1;
        self.attempts.set(attempts);
        if attempts == self.denied_attempt {
            Err(crate::resource::ResourceError::Allocation {
                count: attempts,
                limit: attempts - 1,
            })
        } else {
            Ok(())
        }
    }
    fn on_free(&mut self, _size: usize) {}
    fn on_grow(&mut self, _size: usize) -> Result<(), crate::resource::ResourceError> {
        Ok(())
    }
    fn on_shrink(&mut self, _size: usize) {}
    fn check_time(&mut self) -> Result<(), crate::resource::ResourceError> {
        Ok(())
    }
    fn check_recursion_depth(
        &mut self,
        _depth: usize,
    ) -> Result<(), crate::resource::ResourceError> {
        Ok(())
    }
    fn check_operation_size(
        &mut self,
        _operation: &crate::resource::OperationEstimate,
    ) -> Result<(), crate::resource::ResourceError> {
        Ok(())
    }
}

#[test]
fn string_allocation_status_preserves_success_and_propagates_failure() {
    use super::native_test_fixture::NativeType;
    use molt_cpython_abi::api::strings;
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let mut payload_type = NativeType::subtype(
                &raw mut molt_cpython_abi::abi_types::PyBaseObject_Type,
                c"StringAllocationPriorPayload",
            );
            payload_type.tp_dealloc = Some(oom_prior_payload_drop);
            assert_eq!(payload_type.ready(), 0);
            let _reset = RestoreOomObserver;
            for runtime in [false, true] {
                for constructor in 0..4 {
                    for failed in [false, true] {
                        let prior = install_oom_prior(&raw mut *payload_type, runtime);
                        OOM_PRIOR_ROUNDTRIP.with(|cell| cell.set(failed));
                        OOM_SELECTED_INSTANCE.with(|cell| cell.set(0));
                        let _restore_tracker = failed.then(|| {
                            set_tracker(Box::new(DenyRuntimeAllocation {
                                attempts: std::rc::Rc::new(std::cell::Cell::new(0)),
                                denied_attempt: 1,
                            }));
                            DenyRuntimeAllocations
                        });
                        let text = c"allocation failure payload";
                        let result = refcount::OwnedPyObject::from_owned(match constructor {
                            0 => strings::PyUnicode_FromString(text.as_ptr()),
                            1 => strings::PyBytes_FromString(text.as_ptr()),
                            2 => strings::PyBytes_FromStringAndSize(
                                text.as_ptr(),
                                text.to_bytes().len() as isize,
                            ),
                            _ => strings::PyByteArray_FromStringAndSize(
                                text.as_ptr(),
                                text.to_bytes().len() as isize,
                            ),
                        });
                        OOM_PRIOR_ROUNDTRIP.with(|cell| cell.set(false));
                        assert_eq!(result.as_ptr().is_null(), failed);
                        if failed {
                            assert_oom_prior_retired(1);
                        } else {
                            assert_eq!(OOM_PRIOR_RETIREMENT.with(std::cell::Cell::get), (0, 0));
                        }
                        let selected =
                            refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                        assert!(!selected.as_ptr().is_null());
                        let expected = if failed {
                            OOM_SELECTED_INSTANCE.with(std::cell::Cell::get)
                        } else {
                            prior.addr()
                        };
                        assert_eq!(selected.as_ptr().addr(), expected);
                        drop(selected);
                        assert!(!crate::exception_pending(py));
                    }
                }
            }
        }
    });
}

#[test]
fn container_backing_failure_selects_memory_error_before_partial_cleanup() {
    use super::native_test_fixture::NativeType;
    use crate::object::builders;
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let mut payload_type = NativeType::subtype(
                &raw mut molt_cpython_abi::abi_types::PyBaseObject_Type,
                c"BackingAllocationPriorPayload",
            );
            payload_type.tp_dealloc = Some(oom_prior_payload_drop);
            assert_eq!(payload_type.ready(), 0);
            let _reset = RestoreOomObserver;
            let bytearray_class = crate::builtin_classes(py).bytearray;
            for runtime in [false, true] {
                for constructor in 0..6 {
                    install_oom_prior(&raw mut *payload_type, runtime);
                    OOM_PRIOR_ROUNDTRIP.with(|cell| cell.set(true));
                    OOM_SELECTED_INSTANCE.with(|cell| cell.set(0));
                    let attempts = std::rc::Rc::new(std::cell::Cell::new(0));
                    set_tracker(Box::new(DenyRuntimeAllocation {
                        attempts: attempts.clone(),
                        denied_attempt: 2,
                    }));
                    let _restore_tracker = DenyRuntimeAllocations;
                    let elements = [MoltObject::from_int(7).bits()];
                    let result = match constructor {
                        0 => builders::alloc_list_with_capacity(py, &elements, 8),
                        1 => builders::alloc_list_filled(py, 8, MoltObject::from_int(7)),
                        2 => builders::alloc_list_with_capacity_owned(py, &elements, 8),
                        3 => builders::alloc_bytearray_with_capacity(py, b"abc", 8),
                        4 => builders::alloc_bytearray_with_len(py, 8),
                        _ => builders::alloc_native_bytearray(py, bytearray_class),
                    };
                    let _unexpected = (!result.is_null()).then(|| {
                        molt_runtime_core::OwnedRuntimeValue::from_owned_bits(
                            py.core_token(),
                            MoltObject::from_ptr(result).bits(),
                        )
                    });
                    OOM_PRIOR_ROUNDTRIP.with(|cell| cell.set(false));
                    assert!(
                        result.is_null(),
                        "backing failure constructor {constructor}"
                    );
                    assert!(
                        attempts.get() >= 2,
                        "runtime={runtime}, constructor={constructor}: failure must reach the selected allocation"
                    );
                    assert_oom_prior_retired(1);
                    let selected =
                        refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                    assert!(!selected.as_ptr().is_null());
                    assert_eq!(
                        selected.as_ptr().addr(),
                        OOM_SELECTED_INSTANCE.with(std::cell::Cell::get)
                    );
                    drop(selected);
                    assert!(!crate::exception_pending(py));
                }
            }
        }
    });
}

#[test]
#[ignore = "owns a fresh runtime lifecycle; run in the isolated runtime gate"]
fn canonical_cache_allocation_failure_reentry_and_ascii_no_retry() {
    use super::native_test_fixture::NativeType;
    use crate::object::builders;
    crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
        assert!(register_cpython_hooks());
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let mut payload_type = NativeType::subtype(
                    &raw mut molt_cpython_abi::abi_types::PyBaseObject_Type,
                    c"CanonicalAllocationPriorPayload",
                );
                payload_type.tp_dealloc = Some(oom_prior_payload_drop);
                assert_eq!(payload_type.ready(), 0);
                let _reset = RestoreOomObserver;
                for runtime in [false, true] {
                    for fixed_slot in [false, true] {
                        let outer_name = std::ffi::CString::new(format!(
                            "molt_oom_outer_identifier_{runtime}_{fixed_slot}"
                        ))
                        .unwrap();
                        let inner_name = std::ffi::CString::new(format!(
                            "molt_oom_retirement_identifier_{runtime}_{fixed_slot}"
                        ))
                        .unwrap();
                        let _iteration_observer = RestoreOomObserver;
                        install_oom_prior(&raw mut *payload_type, runtime);
                        OOM_PRIOR_ROUNDTRIP.with(|cell| cell.set(true));
                        OOM_SELECTED_INSTANCE.with(|cell| cell.set(0));
                        OOM_REENTRY_NAME.with(|cell| cell.set(inner_name.as_ptr()));
                        OOM_REENTRY_OBSERVATION.with(|cell| cell.set((false, false, false, 0)));
                        let attempts = std::rc::Rc::new(std::cell::Cell::new(0));
                        set_tracker(Box::new(DenyRuntimeAllocation {
                            attempts: attempts.clone(),
                            denied_attempt: 1,
                        }));
                        let _restore_tracker = DenyRuntimeAllocations;
                        if fixed_slot {
                            assert!(!builders::canonical_singleton_allocation_probe(py));
                        } else {
                            let value = refcount::OwnedPyObject::from_owned(
                                molt_cpython_abi::api::strings::PyUnicode_FromString(
                                    outer_name.as_ptr(),
                                ),
                            );
                            assert!(value.as_ptr().is_null());
                        }
                        OOM_REENTRY_NAME.with(|cell| cell.set(ptr::null()));
                        OOM_PRIOR_ROUNDTRIP.with(|cell| cell.set(false));
                        assert_oom_prior_retired(1);
                        assert_eq!(
                            OOM_REENTRY_OBSERVATION.with(std::cell::Cell::get),
                            (
                                true,
                                true,
                                true,
                                (&raw mut molt_cpython_abi::abi_types::PyExc_MemoryError).addr()
                            ),
                            "both canonical cache locks must be released across error-owner retirement",
                        );
                        assert!(attempts.get() > 1, "reentry must reach the real allocator");
                        let selected =
                            refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                        assert!(!selected.as_ptr().is_null());
                        assert_eq!(
                            selected.as_ptr().addr(),
                            OOM_SELECTED_INSTANCE.with(std::cell::Cell::get)
                        );
                        drop(selected);
                        assert!(!crate::exception_pending(py));
                    }
                }

                // The lifecycle fixture guarantees this is a genuinely cold
                // canonical slot, independent of earlier test execution.
                let byte = builders::uncached_ascii_character_for_test(py)
                    .expect("fresh runtime must expose a cold ASCII slot for this regression");
                for interned in [false, true] {
                    let attempts = std::rc::Rc::new(std::cell::Cell::new(0));
                    set_tracker(Box::new(DenyRuntimeAllocation {
                        attempts: attempts.clone(),
                        denied_attempt: 1,
                    }));
                    let _restore_tracker = DenyRuntimeAllocations;
                    let value = if interned {
                        builders::alloc_interned_string(py, &[byte])
                    } else {
                        builders::alloc_string(py, &[byte])
                    };
                    let _unexpected = (!value.is_null()).then(|| {
                        molt_runtime_core::OwnedRuntimeValue::from_owned_bits(
                            py.core_token(),
                            MoltObject::from_ptr(value).bits(),
                        )
                    });
                    assert!(
                        value.is_null(),
                        "eligible ASCII failure must not fall through"
                    );
                    assert_eq!(
                        attempts.get(),
                        1,
                        "each consumer must try allocation only once"
                    );
                    assert_eq!(builders::uncached_ascii_character_for_test(py), Some(byte));
                    assert_emergency_pending(py);
                    errors::PyErr_Clear();
                }
            }
        });
    });
}

unsafe extern "C" fn array_reentrant_index(_object: *mut PyObject) -> *mut PyObject {
    let array = OOM_ARRAY_HANDLE.with(std::cell::Cell::get);
    let available = crate::builtins::array_mod::array_storage_lock_available_for_test(array);
    ARRAY_INDEX_OBSERVATION.with(|cell| cell.set((cell.get().0 + 1, available)));
    if available {
        crate::molt_array_append(array, MoltObject::from_int(11).bits());
    }
    unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) }
}

#[test]
fn array_allocation_failure_releases_storage_before_prior_retirement() {
    use super::native_test_fixture::NativeType;
    use molt_runtime_core::OwnedRuntimeValue;
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let mut payload_type = NativeType::subtype(
                &raw mut molt_cpython_abi::abi_types::PyBaseObject_Type,
                c"ArrayAllocationPriorPayload",
            );
            payload_type.tp_dealloc = Some(oom_prior_payload_drop);
            assert_eq!(payload_type.ready(), 0);
            let tc = crate::alloc_string(py, b"Q");
            assert!(!tc.is_null());
            let tc = OwnedRuntimeValue::from_owned_bits(
                py.core_token(),
                MoltObject::from_ptr(tc).bits(),
            );
            let original = u64::MAX.to_ne_bytes();
            let bytes = crate::alloc_bytes(py, &original);
            assert!(!bytes.is_null());
            let bytes = OwnedRuntimeValue::from_owned_bits(
                py.core_token(),
                MoltObject::from_ptr(bytes).bits(),
            );
            let slice = crate::object::builders::alloc_slice_obj(
                py,
                MoltObject::none().bits(),
                MoltObject::none().bits(),
                MoltObject::none().bits(),
            );
            assert!(!slice.is_null());
            let slice = OwnedRuntimeValue::from_owned_bits(
                py.core_token(),
                MoltObject::from_ptr(slice).bits(),
            );
            for runtime in [false, true] {
                for operation in 0..6 {
                    let array = OwnedRuntimeValue::from_owned_bits(
                        py.core_token(),
                        crate::molt_array_new(tc.bits()),
                    );
                    assert!(!crate::obj_from_bits(array.bits()).is_none());
                    assert_eq!(
                        crate::molt_array_frombytes(array.bits(), bytes.bits()),
                        MoltObject::none().bits()
                    );
                    assert!(!crate::exception_pending(py));
                    let _reset = RestoreOomObserver;
                    install_oom_prior(&raw mut *payload_type, runtime);
                    OOM_ARRAY_HANDLE.with(|cell| cell.set(array.bits()));
                    OOM_ARRAY_OBSERVATION.with(|cell| cell.set((false, None, None, 0)));
                    OOM_PRIOR_ROUNDTRIP.with(|cell| cell.set(true));
                    OOM_SELECTED_INSTANCE.with(|cell| cell.set(0));
                    let attempts = std::rc::Rc::new(std::cell::Cell::new(0));
                    set_tracker(Box::new(DenyRuntimeAllocation {
                        attempts: attempts.clone(),
                        denied_attempt: 1,
                    }));
                    let tracker = DenyRuntimeAllocations;
                    let result = match operation {
                        0 => crate::molt_array_tobytes(array.bits()),
                        1 => crate::molt_array_tolist(array.bits()),
                        2 => {
                            crate::molt_array_getitem(array.bits(), MoltObject::from_int(0).bits())
                        }
                        3 => crate::molt_array_pop(array.bits(), MoltObject::from_int(0).bits()),
                        4 => crate::molt_array_buffer_info(array.bits()),
                        _ => crate::molt_array_getitem(array.bits(), slice.bits()),
                    };
                    let result = OwnedRuntimeValue::from_owned_bits(py.core_token(), result);
                    drop(tracker);
                    OOM_ARRAY_HANDLE.with(|cell| cell.set(0));
                    OOM_PRIOR_ROUNDTRIP.with(|cell| cell.set(false));
                    assert!(
                        crate::obj_from_bits(result.bits()).is_none(),
                        "operation {operation}"
                    );
                    assert_oom_prior_retired(1);
                    assert_eq!(
                        OOM_ARRAY_OBSERVATION.with(std::cell::Cell::get),
                        (
                            true,
                            Some(1),
                            Some(2),
                            (&raw mut molt_cpython_abi::abi_types::PyExc_MemoryError).addr(),
                        ),
                        "operation {operation}: no lock or buffer export may constrain legal retirement reentry"
                    );
                    assert_eq!(
                        attempts.get(),
                        1,
                        "operation {operation}: the prepared error and its transport allocate no managed storage"
                    );
                    assert!(crate::exception_pending(py));
                    let selected =
                        refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                    assert_eq!(
                        selected.as_ptr().addr(),
                        OOM_SELECTED_INSTANCE.with(std::cell::Cell::get)
                    );
                    assert!(!selected.as_ptr().is_null());
                    drop(selected);
                    let observed = OwnedRuntimeValue::from_owned_bits(
                        py.core_token(),
                        crate::molt_array_tobytes(array.bits()),
                    );
                    let observed_ptr = crate::obj_from_bits(observed.bits())
                        .as_ptr()
                        .expect("bytes result");
                    let expected = [original.as_slice(), 11_u64.to_ne_bytes().as_slice()].concat();
                    assert_eq!(
                        crate::bytes_like_slice(observed_ptr),
                        Some(expected.as_slice())
                    );
                    assert!(!crate::exception_pending(py));
                }
            }
        }
    });
}

#[test]
fn array_index_and_slice_coercion_reenter_before_live_storage_selection() {
    use super::native_test_fixture::NativeType;
    use molt_cpython_abi::abi_types::PyNumberMethods;
    use molt_runtime_core::OwnedRuntimeValue;
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let mut slots: PyNumberMethods = std::mem::zeroed();
            slots.nb_index = array_reentrant_index as *const () as *mut _;
            let mut kind = NativeType::subtype(
                &raw mut molt_cpython_abi::abi_types::PyBaseObject_Type,
                c"ArrayReentrantIndex",
            );
            kind.tp_as_number = (&raw mut slots).cast();
            assert_eq!(kind.ready(), 0);
            let native = refcount::OwnedPyObject::from_owned(
                molt_cpython_abi::api::typeobj::PyType_GenericAlloc(&raw mut *kind, 0),
            );
            assert!(!native.as_ptr().is_null());
            let index = OwnedRuntimeValue::from_owned_bits(
                py.core_token(),
                GLOBAL_BRIDGE
                    .molt_value_for_pyobj(native.as_ptr())
                    .expect("index owner"),
            );
            let tc = crate::alloc_string(py, b"q");
            assert!(!tc.is_null());
            let tc = OwnedRuntimeValue::from_owned_bits(
                py.core_token(),
                MoltObject::from_ptr(tc).bits(),
            );
            for operation in 0..6 {
                let array = OwnedRuntimeValue::from_owned_bits(
                    py.core_token(),
                    crate::molt_array_new(tc.bits()),
                );
                assert!(!crate::obj_from_bits(array.bits()).is_none());
                crate::molt_array_append(array.bits(), MoltObject::from_int(7).bits());
                let _reset = RestoreOomObserver;
                OOM_ARRAY_HANDLE.with(|cell| cell.set(array.bits()));
                ARRAY_INDEX_OBSERVATION.with(|cell| cell.set((0, false)));
                let result = match operation {
                    0 => crate::molt_array_getitem(array.bits(), index.bits()),
                    1 => crate::molt_array_setitem(
                        array.bits(),
                        index.bits(),
                        MoltObject::from_int(17).bits(),
                    ),
                    2 => crate::molt_array_delitem(array.bits(), index.bits()),
                    3 => crate::molt_array_pop(array.bits(), index.bits()),
                    4 => crate::molt_array_insert(
                        array.bits(),
                        index.bits(),
                        MoltObject::from_int(17).bits(),
                    ),
                    _ => {
                        let slice = crate::object::builders::alloc_slice_obj(
                            py,
                            index.bits(),
                            MoltObject::none().bits(),
                            MoltObject::none().bits(),
                        );
                        assert!(!slice.is_null());
                        let slice = OwnedRuntimeValue::from_owned_bits(
                            py.core_token(),
                            MoltObject::from_ptr(slice).bits(),
                        );
                        crate::molt_array_getitem(array.bits(), slice.bits())
                    }
                };
                let result = OwnedRuntimeValue::from_owned_bits(py.core_token(), result);
                OOM_ARRAY_HANDLE.with(|cell| cell.set(0));
                assert_eq!(
                    ARRAY_INDEX_OBSERVATION.with(std::cell::Cell::get),
                    (1, true),
                    "operation {operation}"
                );
                assert!(!crate::exception_pending(py), "operation {operation}");
                if operation == 0 || operation == 3 {
                    assert_eq!(crate::obj_from_bits(result.bits()).as_int(), Some(11));
                }
                let expected: &[i64] = match operation {
                    0 => &[7, 11],
                    1 => &[7, 17],
                    2 | 3 => &[7],
                    4 => &[7, 17, 11],
                    _ => &[11],
                };
                let inspected = if operation == 5 {
                    result.bits()
                } else {
                    array.bits()
                };
                let bytes = OwnedRuntimeValue::from_owned_bits(
                    py.core_token(),
                    crate::molt_array_tobytes(inspected),
                );
                let ptr = crate::obj_from_bits(bytes.bits())
                    .as_ptr()
                    .expect("bytes result");
                let expected = expected
                    .iter()
                    .flat_map(|value| value.to_ne_bytes())
                    .collect::<Vec<_>>();
                assert_eq!(
                    crate::bytes_like_slice(ptr),
                    Some(expected.as_slice()),
                    "operation {operation}"
                );
            }
        }
    });
}

#[derive(Clone, Copy, Default)]
struct PreparationBufferProbe {
    view: u64,
    failure: u64,
    gets: usize,
    releases: usize,
    flags: Option<i64>,
    released_original: bool,
    fallbacks: usize,
}

thread_local! {
    static PREPARATION_BUFFER: std::cell::Cell<PreparationBufferProbe> =
        std::cell::Cell::new(PreparationBufferProbe::default());
}

extern "C" fn preparation_buffer_get(_: u64, flags: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let state = PREPARATION_BUFFER.with(|probe| {
            let mut state = probe.get();
            state.gets += 1;
            state.flags = crate::to_i64(crate::obj_from_bits(flags));
            probe.set(state);
            state
        });
        if state.failure != 0 {
            crate::record_exception(py, crate::obj_from_bits(state.failure).as_ptr().unwrap());
            return MoltObject::none().bits();
        }
        inc_ref_bits(py, state.view);
        state.view
    })
}

extern "C" fn preparation_buffer_release(_: u64, view: u64) -> u64 {
    PREPARATION_BUFFER.with(|probe| {
        let mut state = probe.get();
        state.releases += 1;
        state.released_original = view == state.view;
        probe.set(state);
    });
    MoltObject::none().bits()
}

// The observer is process-global and deliberately unavailable in production.
// Its explicit test below must be the only test running in its process.
#[cfg(feature = "l7-attestation-probe")]
struct DenyPreparationCAllocations;

#[cfg(feature = "l7-attestation-probe")]
impl DenyPreparationCAllocations {
    fn enter() -> Self {
        use molt_cpython_abi::l7_attestation::c_allocations;
        c_allocations::reset();
        c_allocations::set_denial(true);
        c_allocations::set_tracking(true);
        Self
    }
}

#[cfg(feature = "l7-attestation-probe")]
impl Drop for DenyPreparationCAllocations {
    fn drop(&mut self) {
        use molt_cpython_abi::l7_attestation::c_allocations;
        c_allocations::set_tracking(false);
        c_allocations::set_denial(false);
    }
}

fn preparation_buffer_instance<'a, 'py>(
    py: &'a crate::PyToken<'py>,
    label: &[u8],
) -> (
    crate::builtins::exceptions::ExceptionValue<'a, 'py>,
    crate::builtins::exceptions::ExceptionValue<'a, 'py>,
) {
    use crate::builtins::exceptions::ExceptionValue;
    let name = ExceptionValue::adopt(py, crate::attr_name_bits_from_bytes(py, label).unwrap());
    let mut methods = Vec::new();
    for (name, target) in [
        (
            b"__buffer__".as_slice(),
            preparation_buffer_get as *const (),
        ),
        (
            b"__release_buffer__".as_slice(),
            preparation_buffer_release as *const (),
        ),
    ] {
        let key = ExceptionValue::adopt(py, crate::attr_name_bits_from_bytes(py, name).unwrap());
        let function = crate::object::builders::alloc_function_obj(
            py,
            crate::provenance::abi::expose_function_address(target),
            2,
        );
        assert!(!function.is_null());
        unsafe { crate::object::layout::function_set_call_target_ptr(function, target) };
        methods.push((
            key,
            ExceptionValue::adopt(py, MoltObject::from_ptr(function).bits()),
        ));
    }
    let pairs: Vec<_> = methods
        .iter()
        .flat_map(|(key, value)| [key.bits(), value.bits()])
        .collect();
    let namespace = crate::alloc_dict_with_pairs(py, &pairs);
    assert!(!namespace.is_null());
    let namespace = ExceptionValue::adopt(py, MoltObject::from_ptr(namespace).bits());
    let class = ExceptionValue::adopt(
        py,
        crate::builtins::types::molt_type_new(
            crate::builtin_classes(py).type_obj,
            name.bits(),
            crate::builtin_classes(py).object,
            namespace.bits(),
            MoltObject::none().bits(),
        ),
    );
    let instance = ExceptionValue::adopt(py, unsafe {
        crate::alloc_instance_for_class(py, crate::obj_from_bits(class.bits()).as_ptr().unwrap())
    });
    (class, instance)
}

fn writable_buffer_preparation_case(deny_lease: bool) {
    use crate::builtins::exceptions::ExceptionValue;
    use crate::builtins::io::{molt_bytesio_new, molt_file_readinto, molt_file_tell};
    use crate::object::buffer_exports::{BufferAccessError, ScopedWritableBuffer};
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        let (_class, instance) = preparation_buffer_instance(py, b"PreparationExporter");
        let backing = ExceptionValue::adopt(
            py,
            MoltObject::from_ptr(crate::object::builders::alloc_bytearray(py, b"---")).bits(),
        );
        let view = ExceptionValue::adopt(py, crate::molt_memoryview_new(backing.bits()));
        let initial = ExceptionValue::adopt(
            py,
            MoltObject::from_ptr(crate::object::builders::alloc_bytes(py, b"Z")).bits(),
        );
        let stream = ExceptionValue::adopt(
            py,
            molt_bytesio_new(crate::builtin_classes(py).bytes_io, initial.bits()),
        );
        let failure = ExceptionValue::adopt(py, exception_bits(py, "MemoryError"));
        assert!(!crate::exception_pending(py));
        PREPARATION_BUFFER.with(|probe| {
            probe.set(PreparationBufferProbe {
                view: view.bits(),
                ..Default::default()
            })
        });
        let pointer = crate::obj_from_bits(instance.bits()).as_ptr().unwrap();
        let has_view = || unsafe {
            (*header_from_obj_ptr(pointer)).load_synchronized_flags()
                & crate::object::HEADER_FLAG_HAS_ABI_VIEW
                != 0
        };
        assert!(
            !has_view(),
            "the first acquisition must really project a cold object"
        );
        let (result, attempts) = GLOBAL_BRIDGE
            .with_denied_publication_for_test(instance.bits(), || {
                molt_file_readinto(stream.bits(), instance.bits())
            });
        assert_eq!(result, MoltObject::none().bits());
        assert_eq!(attempts, 1);
        assert!(!has_view());
        assert_eq!(
            unsafe {
                errors::PyErr_ExceptionMatches(
                    (&raw mut molt_cpython_abi::abi_types::PyExc_MemoryError).cast(),
                )
            },
            1,
            "w* must retain preparation failure instead of normalizing it"
        );
        assert_eq!(
            PREPARATION_BUFFER.with(|probe| (probe.get().gets, probe.get().releases)),
            (0, 0)
        );
        // The selected failure is retained across the actual C error ownership transfer.
        unsafe {
            let error = refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
            assert!(!error.as_ptr().is_null());
            let identity = error.as_ptr();
            errors::PyErr_SetRaisedException(error.into_ptr());
            let error = refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
            assert_eq!(error.as_ptr(), identity);
        }
        unsafe { errors::PyErr_Clear() };
        assert_eq!(
            crate::to_i64(crate::obj_from_bits(molt_file_tell(stream.bits()))),
            Some(0)
        );

        if deny_lease {
            #[cfg(feature = "l7-attestation-probe")]
            {
                // Warm only projection. The first denied allocation is the stable
                // lease master, before any Python exporter callback can run.
                let projected = unsafe {
                    molt_cpython_abi::native_custody::project_buffer_object(instance.bits())
                }
                .unwrap();
                let denied = DenyPreparationCAllocations::enter();
                let result = molt_file_readinto(stream.bits(), instance.bits());
                drop(denied);
                let snapshot = molt_cpython_abi::l7_attestation::c_allocations::snapshot();
                assert_eq!(snapshot.attempts, 1);
                assert_eq!(snapshot.denied, 1);
                assert_eq!(result, MoltObject::none().bits());
                assert_eq!(
                    unsafe {
                        errors::PyErr_ExceptionMatches(
                            (&raw mut molt_cpython_abi::abi_types::PyExc_MemoryError).cast(),
                        )
                    },
                    1
                );
                assert_eq!(
                    PREPARATION_BUFFER.with(|probe| (probe.get().gets, probe.get().releases)),
                    (0, 0)
                );
                unsafe { errors::PyErr_Clear() };
                drop(projected);
                assert_eq!(
                    crate::to_i64(crate::obj_from_bits(molt_file_tell(stream.bits()))),
                    Some(0)
                );
            }
            #[cfg(not(feature = "l7-attestation-probe"))]
            panic!("C allocation injection requires the existing attestation feature");
        }

        PREPARATION_BUFFER.with(|probe| {
            let mut state = probe.get();
            state.failure = failure.bits();
            probe.set(state);
        });
        assert!(matches!(
            ScopedWritableBuffer::new(py, instance.bits()),
            Err(BufferAccessError::Pending)
        ));
        assert_eq!(
            crate::exception_last_bits_noinc(py),
            Some(failure.bits()),
            "the exporter really raised the retained MemoryError"
        );
        unsafe { errors::PyErr_Clear() };
        assert_eq!(
            molt_file_readinto(stream.bits(), instance.bits()),
            MoltObject::none().bits()
        );
        assert_eq!(
            unsafe {
                errors::PyErr_ExceptionMatches(
                    (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast(),
                )
            },
            1,
            "the same exception class from exporter dispatch is normalized by w*"
        );
        let normalized = crate::exception_last_bits_noinc(py).expect("normalized runtime error");
        assert_ne!(normalized, failure.bits());
        assert_eq!(
            crate::builtins::exceptions::format_exception_message(
                py,
                crate::obj_from_bits(normalized).as_ptr().unwrap()
            ),
            "readinto() argument must be read-write bytes-like object, not PreparationExporter"
        );
        assert_eq!(
            PREPARATION_BUFFER.with(|probe| (
                probe.get().gets,
                probe.get().releases,
                probe.get().flags
            )),
            (2, 0, Some(1))
        );
        unsafe { errors::PyErr_Clear() };
        assert_eq!(
            crate::to_i64(crate::obj_from_bits(molt_file_tell(stream.bits()))),
            Some(0)
        );
        assert_eq!(
            unsafe {
                crate::bytes_like_slice_raw(crate::obj_from_bits(backing.bits()).as_ptr().unwrap())
                    .unwrap()
            },
            b"---"
        );
        PREPARATION_BUFFER.with(|probe| {
            let mut state = probe.get();
            state.failure = 0;
            probe.set(state);
        });
        assert_eq!(
            crate::to_i64(crate::obj_from_bits(molt_file_readinto(
                stream.bits(),
                instance.bits()
            ))),
            Some(1)
        );
        assert!(!crate::exception_pending(py));
        assert_eq!(
            unsafe {
                crate::bytes_like_slice_raw(crate::obj_from_bits(backing.bits()).as_ptr().unwrap())
                    .unwrap()
            },
            b"Z--"
        );
        assert_eq!(
            PREPARATION_BUFFER.with(|probe| (
                probe.get().gets,
                probe.get().releases,
                probe.get().released_original
            )),
            (3, 1, true)
        );
        crate::molt_memoryview_release(view.bits());
        crate::molt_bytearray_append(backing.bits(), MoltObject::from_int(33).bits());
        assert!(
            !crate::exception_pending(py),
            "all successful and failed acquisition pins have retired"
        );
        PREPARATION_BUFFER.with(|probe| probe.set(PreparationBufferProbe::default()));
    });
}

#[test]
fn writable_buffer_cold_preparation_and_exporter_memory_error_have_distinct_policy() {
    writable_buffer_preparation_case(false);
}

#[cfg(feature = "l7-attestation-probe")]
#[test]
#[ignore = "process-global C allocation denial: run this exact test alone in a fresh process"]
fn writable_buffer_lease_preparation_failure_precedes_exporter_dispatch() {
    writable_buffer_preparation_case(true);
    bytearray_normalization_preserves_lease_preparation();
}

#[test]
#[cfg(feature = "stdlib_serial")]
fn buffer_consumers_preserve_cold_semantic_class_preparation_failure() {
    use crate::builtins::exceptions::{
        ExceptionValue, exception_matches_builtin_name, molt_exception_last_pending,
    };
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        for (consumer, source) in [
            (0, b"buffer".as_slice()),
            (1, b"QQ==".as_slice()),
            (2, b"41".as_slice()),
            (3, b"=41".as_slice()),
            (4, b"!00  \n".as_slice()),
            #[cfg(feature = "stdlib_regex")]
            (5, b"A".as_slice()),
            #[cfg(feature = "stdlib_regex")]
            (6, b"A".as_slice()),
        ] {
            let (class, instance) = preparation_buffer_instance(
                py,
                format!("ColdConsumerExporter{consumer}").as_bytes(),
            );
            let backing = ExceptionValue::adopt(
                py,
                MoltObject::from_ptr(crate::alloc_bytearray(py, source)).bits(),
            );
            let view = ExceptionValue::adopt(py, crate::molt_memoryview_new(backing.bits()));
            assert!(!crate::exception_pending(py));
            PREPARATION_BUFFER.with(|probe| {
                probe.set(PreparationBufferProbe {
                    view: view.bits(),
                    ..Default::default()
                })
            });
            let invoke = || match consumer {
                0 => crate::molt_memoryview_new(instance.bits()),
                1 => molt_runtime_serial::binascii::molt_binascii_a2b_base64(
                    instance.bits(),
                    MoltObject::from_bool(false).bits(),
                ),
                2 => molt_runtime_serial::binascii::molt_binascii_a2b_hex(instance.bits()),
                3 => molt_runtime_serial::binascii::molt_binascii_a2b_qp(instance.bits()),
                4 => molt_runtime_serial::binascii::molt_binascii_a2b_uu(instance.bits()),
                #[cfg(feature = "stdlib_regex")]
                5 => crate::molt_re_subject_snapshot(instance.bits()),
                #[cfg(feature = "stdlib_regex")]
                6 => crate::molt_re_subject_view(instance.bits()),
                _ => unreachable!(),
            };
            assert!(!GLOBAL_BRIDGE.type_has_projection(class.bits()));
            let (returned, attempts) =
                GLOBAL_BRIDGE.with_denied_publication_for_test(class.bits(), invoke);
            assert_eq!(returned, MoltObject::none().bits(), "consumer {consumer}");
            assert_eq!(
                attempts, 1,
                "the actual semantic class admission was denied"
            );
            assert!(!GLOBAL_BRIDGE.type_has_projection(class.bits()));
            assert_eq!(
                PREPARATION_BUFFER.with(|probe| (probe.get().gets, probe.get().releases)),
                (0, 0)
            );
            let selected = ExceptionValue::adopt(py, molt_exception_last_pending());
            assert!(
                exception_matches_builtin_name(py, selected.bits(), "MemoryError"),
                "preparation failure must not become converter TypeError: consumer {consumer}"
            );
            // Preserve the exact selected owner through the real C error channel.
            // No replacement exception may appear during projection/retirement.
            unsafe {
                let transported =
                    refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                assert!(!transported.as_ptr().is_null());
                let projected = ExceptionValue::adopt(
                    py,
                    GLOBAL_BRIDGE
                        .molt_value_for_pyobj(transported.as_ptr())
                        .unwrap(),
                );
                assert_eq!(projected.bits(), selected.bits());
                let identity = transported.as_ptr();
                errors::PyErr_SetRaisedException(transported.into_ptr());
                let restored =
                    refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                assert_eq!(restored.as_ptr(), identity);
            }
            assert!(!crate::exception_pending(py));
            drop(selected);

            // With that same class legitimately prepared, the real exporter is
            // called once and releases once. Denial never warms a fake success.
            let projected = unsafe {
                molt_cpython_abi::native_custody::NativeObjectOwner::from_owned(
                    GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(class.bits()),
                )
            };
            assert!(!projected.as_ptr().is_null());
            let result = ExceptionValue::adopt(py, invoke());
            assert!(!crate::exception_pending(py), "consumer {consumer}");
            if consumer == 0 || consumer == 6 {
                crate::molt_memoryview_release(result.bits());
            } else {
                assert_eq!(
                    unsafe {
                        crate::bytes_like_slice_raw(
                            crate::obj_from_bits(result.bits()).as_ptr().unwrap(),
                        )
                    },
                    Some(b"A".as_slice())
                );
            }
            assert_eq!(
                PREPARATION_BUFFER.with(|probe| (probe.get().gets, probe.get().releases)),
                (1, 1)
            );
            crate::molt_memoryview_release(view.bits());
            crate::molt_bytearray_append(backing.bits(), MoltObject::from_int(33).bits());
            assert!(
                !crate::exception_pending(py),
                "all conversion pins must retire"
            );
            PREPARATION_BUFFER.with(|probe| probe.set(PreparationBufferProbe::default()));
        }
    });
}

extern "C" fn preparation_buffer_forbidden_fallback(_: u64) -> u64 {
    PREPARATION_BUFFER.with(|probe| {
        let mut state = probe.get();
        state.fallbacks += 1;
        probe.set(state);
    });
    crate::with_gil_entry_nopanic!(py, {
        crate::raise_exception(
            py,
            "AssertionError",
            "preparation failure entered fallback protocol",
        )
    })
}

#[test]
fn buffer_eligibility_preparation_failure_never_selects_a_fallback_protocol() {
    use crate::builtins::exceptions::ExceptionValue;
    use crate::object::ops_compare::SequenceComparison;
    use molt_obj_model::sequence_compare::RichCompareOp;
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        for consumer in 0..8 {
            let (class, instance) = preparation_buffer_instance(
                py,
                format!("ColdEligibilityExporter{consumer}").as_bytes(),
            );
            // __iter__ distinguishes an incorrect constructor/slice fallback;
            // find must not interpret __index__ after failed buffer admission.
            let protocol = if consumer == 2 {
                b"__index__".as_slice()
            } else {
                b"__iter__".as_slice()
            };
            let key =
                ExceptionValue::adopt(py, crate::attr_name_bits_from_bytes(py, protocol).unwrap());
            let target = preparation_buffer_forbidden_fallback as *const ();
            let function = crate::object::builders::alloc_function_obj(
                py,
                crate::provenance::abi::expose_function_address(target),
                1,
            );
            assert!(!function.is_null());
            unsafe { crate::object::layout::function_set_call_target_ptr(function, target) };
            let function = ExceptionValue::adopt(py, MoltObject::from_ptr(function).bits());
            let assigned = ExceptionValue::adopt(
                py,
                crate::molt_set_attr_name(class.bits(), key.bits(), function.bits()),
            );
            assert!(!crate::exception_pending(py));
            drop(assigned);
            let backing = ExceptionValue::adopt(
                py,
                MoltObject::from_ptr(crate::alloc_bytearray(py, b"A")).bits(),
            );
            let view = ExceptionValue::adopt(py, crate::molt_memoryview_new(backing.bits()));
            let haystack = ExceptionValue::adopt(
                py,
                MoltObject::from_ptr(crate::alloc_bytes(py, b"A")).bits(),
            );
            let destination = ExceptionValue::adopt(
                py,
                MoltObject::from_ptr(crate::alloc_bytearray(py, b"A")).bits(),
            );
            let slice = ExceptionValue::adopt(
                py,
                crate::molt_slice_new(
                    MoltObject::none().bits(),
                    MoltObject::none().bits(),
                    MoltObject::none().bits(),
                ),
            );
            assert!(!crate::exception_pending(py));
            PREPARATION_BUFFER.with(|probe| {
                probe.set(PreparationBufferProbe {
                    view: view.bits(),
                    ..Default::default()
                })
            });
            let invoke = || match consumer {
                0 => crate::object::ops_bytes::bytes_contains_builtin(
                    py,
                    haystack.bits(),
                    instance.bits(),
                ),
                1 => crate::molt_bytes_startswith(haystack.bits(), instance.bits()),
                2 => crate::molt_bytes_find(haystack.bits(), instance.bits()),
                3 => crate::molt_bytes_from_obj(instance.bits()),
                4 => crate::object::ops_bytes::bytearray_init_from_arguments(
                    py,
                    destination.bits(),
                    [Some(instance.bits()), None, None],
                ),
                5 => crate::molt_bytearray_extend(destination.bits(), instance.bits()),
                6 => {
                    let borrowed =
                        crate::molt_store_index(destination.bits(), slice.bits(), instance.bits());
                    assert!(crate::exception_pending(py) || borrowed == destination.bits());
                    // StoreIndex has no IR result: its ABI receiver is borrowed,
                    // unlike the owned values adopted by this consumer matrix.
                    MoltObject::none().bits()
                }
                7 => SequenceComparison::Bytearray.invoke(
                    py,
                    destination.bits(),
                    instance.bits(),
                    RichCompareOp::Eq,
                ),
                _ => unreachable!(),
            };
            assert!(!GLOBAL_BRIDGE.type_has_projection(class.bits()));
            let (returned, attempts) =
                GLOBAL_BRIDGE.with_denied_publication_for_test(class.bits(), invoke);
            assert_eq!(returned, MoltObject::none().bits(), "consumer {consumer}");
            assert_eq!(attempts, 1, "cold semantic class must enter real admission");
            assert!(!GLOBAL_BRIDGE.type_has_projection(class.bits()));
            assert_eq!(
                PREPARATION_BUFFER.with(|probe| {
                    let state = probe.get();
                    (state.gets, state.releases, state.fallbacks)
                }),
                (0, 0, 0),
                "consumer {consumer}"
            );
            crate::test_support::assert_and_clear_emergency_memory_error(py);

            let projected = unsafe {
                molt_cpython_abi::native_custody::NativeObjectOwner::from_owned(
                    GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(class.bits()),
                )
            };
            assert!(!projected.as_ptr().is_null());
            let result = ExceptionValue::adopt(py, invoke());
            assert!(!crate::exception_pending(py), "warm consumer {consumer}");
            match consumer {
                0 | 1 | 7 => assert_eq!(result.bits(), MoltObject::from_bool(true).bits()),
                2 => assert_eq!(crate::obj_from_bits(result.bits()).as_int(), Some(0)),
                3 => assert_eq!(
                    unsafe {
                        crate::bytes_like_slice_raw(
                            crate::obj_from_bits(result.bits()).as_ptr().unwrap(),
                        )
                    },
                    Some(b"A".as_slice())
                ),
                4..=6 => assert_eq!(
                    unsafe {
                        crate::bytes_like_slice_raw(
                            crate::obj_from_bits(destination.bits()).as_ptr().unwrap(),
                        )
                    },
                    Some(if consumer == 5 {
                        b"AA".as_slice()
                    } else {
                        b"A".as_slice()
                    })
                ),
                _ => unreachable!(),
            }
            assert_eq!(
                PREPARATION_BUFFER.with(|probe| {
                    let state = probe.get();
                    (state.gets, state.releases, state.fallbacks)
                }),
                (1, 1, 0),
                "warm consumer {consumer}"
            );
            crate::molt_memoryview_release(view.bits());
            crate::molt_bytearray_append(backing.bits(), MoltObject::from_int(33).bits());
            assert!(
                !crate::exception_pending(py),
                "all conversion pins must retire"
            );
            PREPARATION_BUFFER.with(|probe| probe.set(PreparationBufferProbe::default()));
        }
    });
}

#[cfg(feature = "l7-attestation-probe")]
fn bytearray_normalization_preserves_lease_preparation() {
    use crate::builtins::exceptions::ExceptionValue;
    use crate::object::ops_compare::SequenceComparison;
    use molt_obj_model::sequence_compare::RichCompareOp;
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        for consumer in 0..2 {
            let (_class, instance) = preparation_buffer_instance(
                py,
                format!("LeasePreparationExporter{consumer}").as_bytes(),
            );
            let backing = ExceptionValue::adopt(
                py,
                MoltObject::from_ptr(crate::alloc_bytearray(py, b"A")).bits(),
            );
            let view = ExceptionValue::adopt(py, crate::molt_memoryview_new(backing.bits()));
            let destination = ExceptionValue::adopt(
                py,
                MoltObject::from_ptr(crate::alloc_bytearray(py, b"A")).bits(),
            );
            let projected =
                unsafe { molt_cpython_abi::native_custody::project_buffer_object(instance.bits()) }
                    .unwrap();
            PREPARATION_BUFFER.with(|probe| {
                probe.set(PreparationBufferProbe {
                    view: view.bits(),
                    ..Default::default()
                })
            });
            let invoke = || {
                if consumer == 0 {
                    crate::molt_bytearray_extend(destination.bits(), instance.bits())
                } else {
                    SequenceComparison::Bytearray.invoke(
                        py,
                        destination.bits(),
                        instance.bits(),
                        RichCompareOp::Eq,
                    )
                }
            };
            let denied = DenyPreparationCAllocations::enter();
            let returned = invoke();
            drop(denied);
            assert_eq!(returned, MoltObject::none().bits());
            let snapshot = molt_cpython_abi::l7_attestation::c_allocations::snapshot();
            assert_eq!(snapshot.attempts, 1);
            assert_eq!(snapshot.denied, 1);
            assert_eq!(
                PREPARATION_BUFFER.with(|probe| (probe.get().gets, probe.get().releases)),
                (0, 0)
            );
            crate::test_support::assert_and_clear_emergency_memory_error(py);
            assert_eq!(
                unsafe {
                    crate::bytes_like_slice_raw(
                        crate::obj_from_bits(destination.bits()).as_ptr().unwrap(),
                    )
                },
                Some(b"A".as_slice())
            );

            let result = ExceptionValue::adopt(py, invoke());
            assert!(!crate::exception_pending(py));
            if consumer == 1 {
                assert_eq!(result.bits(), MoltObject::from_bool(true).bits());
            }
            assert_eq!(
                PREPARATION_BUFFER.with(|probe| (probe.get().gets, probe.get().releases)),
                (1, 1)
            );
            crate::molt_memoryview_release(view.bits());
            crate::molt_bytearray_append(backing.bits(), MoltObject::from_int(33).bits());
            assert!(!crate::exception_pending(py));
            PREPARATION_BUFFER.with(|probe| probe.set(PreparationBufferProbe::default()));
            drop(projected);
        }
    });
}
