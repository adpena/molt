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
            crate::record_exception(py, crate::obj_from_bits(runtime).as_ptr().unwrap());
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
            crate::record_exception(py, crate::obj_from_bits(runtime).as_ptr().unwrap());
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
    assert!(matches!(raised, Some(RaisedSnapshot::Emergency(_))));
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
            crate::builtins::exceptions::molt_exception_set_last(runtime);
            let result = errors::with_preserved_error(|| {
                assert!(errors::PyErr_Occurred().is_null());
                assert!(!crate::exception_pending(py));
                assert_eq!(
                    crate::builtins::exceptions::exception_context_active_bits(),
                    Some(handled)
                );
                set_c_error(cleanup_view);
                crate::builtins::exceptions::molt_exception_set_last(handled);
                errors::with_preserved_error(|| {
                    assert!(errors::PyErr_Occurred().is_null());
                    assert!(!crate::exception_pending(py));
                    set_c_error(original_view);
                    crate::builtins::exceptions::molt_exception_set_last(runtime);
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
        crate::record_exception(
            py,
            crate::obj_from_bits(input.runtime_error).as_ptr().unwrap(),
        );
    });
}

#[test]
fn sequence_owners_preserve_both_errors_through_terminal_retirement_and_item_transfer() {
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
                crate::record_exception(py, crate::obj_from_bits(runtime).as_ptr().unwrap());
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
                    drop(item_owner);
                }
                drop(tuple_owner);
                drop(snapshot_owner);
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
            crate::builtins::exceptions::molt_exception_set_last(runtime);
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
        unsafe { set_c_error(original_view) };
        crate::record_memory_error_without_allocation(py);
        let denied = DenyRuntimeAllocations::enter();
        errors::with_preserved_error(|| {
            assert!(errors::take_current_error().is_none());
            assert!(!crate::exception_pending(py));
            crate::record_memory_error_without_allocation(py);
        });
        assert_emergency_pending(py);
        let pending = take_native_pending_snapshot();
        assert!(pending.has_error());
        assert!(matches!(
            pending.runtime_error,
            Some(RaisedSnapshot::Emergency(_))
        ));
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
    assert_eq!(
        unsafe { errors::PyErr_Occurred() },
        (&raw mut PyExc_MemoryError).cast()
    );
    assert_eq!(
        unsafe { errors::PyErr_ExceptionMatches((&raw mut PyExc_Exception).cast()) },
        1
    );
    let (mut kind, mut value, mut traceback) = (ptr::null_mut(), ptr::null_mut(), ptr::null_mut());
    unsafe { errors::PyErr_Fetch(&raw mut kind, &raw mut value, &raw mut traceback) };
    let kind_owner = unsafe { refcount::OwnedPyObject::from_owned(kind) };
    let value_owner = unsafe { refcount::OwnedPyObject::from_owned(value) };
    let traceback_owner = unsafe { refcount::OwnedPyObject::from_owned(traceback) };
    assert!(kind.is_null() && value.is_null() && traceback.is_null());
    drop(kind_owner);
    drop(value_owner);
    drop(traceback_owner);
    assert_emergency_pending(py);
    let raised = unsafe { refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException()) };
    assert!(raised.as_ptr().is_null());
    drop(raised);
    assert_emergency_pending(py);
    assert!(errors::take_current_error().is_none());
    assert_eq!(
        unsafe { errors::PyErr_Occurred() },
        (&raw mut PyExc_MemoryError).cast()
    );
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
        // The same classification applies to native status callbacks.
        for succeed in [false, true] {
            call.succeed = succeed;
            let denied = DenyRuntimeAllocations::enter();
            assert_eq!(
                unsafe { object::PyObject_SetAttr(&raw mut call.object, name, value) },
                -1
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
fn core_owned_iterator_value_preserves_both_errors_during_native_retirement() {
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
                let owner =
                    molt_runtime_core::OwnedRuntimeValue::from_owned_bits(py.core_token(), bits);
                set_c_error(original_view);
                crate::record_exception(py, crate::obj_from_bits(runtime).as_ptr().unwrap());
                if transfer {
                    let bits = owner.into_bits();
                    assert_eq!(SEQUENCE_INPUT_DROPS.with(|count| count.get()), 0);
                    molt_runtime_core::ffi::__molt_runtime_release_owned_value(bits);
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
