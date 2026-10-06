//! Consumer proofs use C-owned exception allocation, never a managed
//! constructor result exported as a C view.

use super::native_test_fixture::NativeType;
use crate::builtins::exceptions::{
    ExceptionFieldSlot, ExceptionValue, exception_replace_field_bits,
};
use crate::{MoltObject, PyToken};
use molt_cpython_abi::abi_types::*;
use molt_cpython_abi::api::refcount::OwnedPyObject;
use molt_cpython_abi::api::{errors, numbers, object, sequences, strings, typeobj};
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
use std::ptr;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

struct CallbackState;

impl CallbackState {
    fn new() -> Self {
        Self::reset();
        Self
    }

    fn reset() {
        CHECK_MODE.store(0, Ordering::SeqCst);
        CHECK_CALLS.store(0, Ordering::SeqCst);
        CHECK_FAILURE.store(ptr::null_mut(), Ordering::SeqCst);
        EXIT_EXCEPTION.store(ptr::null_mut(), Ordering::SeqCst);
        EXIT_TRACEBACK.store(ptr::null_mut(), Ordering::SeqCst);
        EXIT_CALLS.store(0, Ordering::SeqCst);
    }
}

impl Drop for CallbackState {
    fn drop(&mut self) {
        Self::reset();
    }
}

unsafe fn native_exception<'a, 'py>(
    py: &'a PyToken<'py>,
    class: *mut PyTypeObject,
    message: &'static std::ffi::CStr,
) -> ExceptionValue<'a, 'py> {
    unsafe {
        assert_eq!(typeobj::PyType_Ready(class), 0);
        let args = OwnedPyObject::from_owned(sequences::PyTuple_New(1));
        let text = OwnedPyObject::from_owned(strings::PyUnicode_FromString(message.as_ptr()));
        assert!(!args.as_ptr().is_null() && !text.as_ptr().is_null());
        assert_eq!(
            sequences::PyTuple_SetItem(args.as_ptr(), 0, text.into_ptr()),
            0
        );
        let native = OwnedPyObject::from_owned(errors::molt_native_exception_new(
            class,
            args.as_ptr(),
            ptr::null_mut(),
        ));
        drop(args);
        assert!(!native.as_ptr().is_null());
        let value = ExceptionValue::adopt(
            py,
            GLOBAL_BRIDGE.molt_value_for_pyobj(native.as_ptr()).unwrap(),
        );
        assert_eq!(
            crate::object_type_id(crate::obj_from_bits(value.bits()).as_ptr().unwrap()),
            crate::TYPE_ID_FOREIGN
        );
        assert_eq!(
            GLOBAL_BRIDGE.handle_to_borrowed_pyobj(value.bits()),
            native.as_ptr()
        );
        drop(native);
        value
    }
}

fn public_render(py: &PyToken<'_>, value: u64) -> String {
    let none = MoltObject::none().bits();
    let rendered = ExceptionValue::adopt(
        py,
        crate::molt_traceback_format_exception(
            none,
            value,
            none,
            none,
            MoltObject::from_bool(true).bits(),
        ),
    );
    assert!(!crate::exception_pending(py));
    let lines = unsafe {
        crate::object::seq_access::snapshot(
            py,
            crate::obj_from_bits(rendered.bits()).as_ptr().unwrap(),
            "render test lines",
        )
    }
    .unwrap();
    lines
        .iter()
        .map(|bits| crate::string_obj_to_owned(crate::obj_from_bits(*bits)).unwrap())
        .collect()
}

#[test]
fn native_and_managed_chain_renderers_share_cause_suppression_and_cycle_policy() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let _callbacks = CallbackState::new();
    assert!(super::register_cpython_hooks());
    let _execution = crate::concurrency::RuntimeExecutionGuard::enter();
    crate::concurrency::gil::with_gil(|py| unsafe {
        let py = &py;
        let native = native_exception(py, &raw mut PyExc_ValueError, c"native-root");
        let managed = ExceptionValue::adopt(
            py,
            MoltObject::from_ptr(crate::builtins::exceptions::alloc_exception(
                py,
                "TypeError",
                "managed-cause",
            ))
            .bits(),
        );
        exception_replace_field_bits(py, native.bits(), ExceptionFieldSlot::Cause, managed.bits())
            .unwrap();
        crate::builtins::exceptions::exception_replace_suppress_context(py, native.bits(), true)
            .unwrap();
        exception_replace_field_bits(
            py,
            managed.bits(),
            ExceptionFieldSlot::Context,
            native.bits(),
        )
        .unwrap();
        let rendered = public_render(py, native.bits());
        let diagnostic = crate::builtins::exceptions::format_exception_with_traceback(
            py,
            crate::obj_from_bits(native.bits()).as_ptr().unwrap(),
        );
        for text in [&rendered, &diagnostic] {
            assert_eq!(text.matches("native-root").count(), 1);
            assert_eq!(text.matches("managed-cause").count(), 1);
            assert_eq!(text.matches("direct cause").count(), 1);
            assert!(text.find("managed-cause").unwrap() < text.find("native-root").unwrap());
        }
        let graph = ExceptionValue::adopt(
            py,
            crate::object::ops_sys::traceback_exception_chain_payload_bits(py, native.bits(), None)
                .unwrap(),
        );
        let nodes = crate::object::seq_access::snapshot(
            py,
            crate::obj_from_bits(graph.bits()).as_ptr().unwrap(),
            "chain graph",
        )
        .unwrap();
        assert_eq!(
            nodes.len(),
            2,
            "mixed cycles retain each physical exception exactly once"
        );

        crate::molt_exception_set_last(native.bits());
        let formatted = ExceptionValue::adopt(
            py,
            crate::molt_traceback_format_exc(MoltObject::none().bits()),
        );
        assert!(
            crate::exception_pending(py),
            "successful rendering restores the original raised owner"
        );
        assert_eq!(crate::exception_last_bits_noinc(py), Some(native.bits()));
        assert!(
            crate::string_obj_to_owned(crate::obj_from_bits(formatted.bits()))
                .unwrap()
                .contains("native-root")
        );
        crate::clear_exception(py);
        for (value, field) in [
            (native.bits(), ExceptionFieldSlot::Cause),
            (managed.bits(), ExceptionFieldSlot::Context),
        ] {
            exception_replace_field_bits(py, value, field, MoltObject::none().bits()).unwrap();
        }
    });
}

static CHECK_MODE: AtomicUsize = AtomicUsize::new(0);
static CHECK_CALLS: AtomicUsize = AtomicUsize::new(0);
static CHECK_FAILURE: AtomicPtr<PyObject> = AtomicPtr::new(ptr::null_mut());

unsafe extern "C" fn class_check(_self: *mut PyObject, _candidate: *mut PyObject) -> *mut PyObject {
    CHECK_CALLS.fetch_add(1, Ordering::SeqCst);
    unsafe {
        if CHECK_MODE.load(Ordering::SeqCst) == 2 {
            errors::PyErr_SetObject(
                (&raw mut PyExc_LookupError).cast(),
                CHECK_FAILURE.load(Ordering::SeqCst),
            );
            ptr::null_mut()
        } else {
            object::Py_NewRef(if CHECK_MODE.load(Ordering::SeqCst) == 0 {
                (&raw mut Py_True).cast()
            } else {
                (&raw mut Py_False).cast()
            })
        }
    }
}

#[test]
fn native_classinfo_preserves_hooks_tuple_order_and_throw_failure_identity() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let _callbacks = CallbackState::new();
    assert!(super::register_cpython_hooks());
    let _execution = crate::concurrency::RuntimeExecutionGuard::enter();
    crate::concurrency::gil::with_gil(|py| unsafe {
        let py = &py;
        let failure = native_exception(py, &raw mut PyExc_LookupError, c"class hook failed");
        let input = native_exception(py, &raw mut PyExc_IndexError, c"class input");
        CHECK_FAILURE.store(
            GLOBAL_BRIDGE.handle_to_borrowed_pyobj(failure.bits()),
            Ordering::SeqCst,
        );
        let mut methods = [
            PyMethodDef {
                ml_name: c"__subclasscheck__".as_ptr(),
                ml_meth: Some(class_check),
                ml_flags: METH_O,
                ml_doc: ptr::null(),
            },
            PyMethodDef {
                ml_name: c"__instancecheck__".as_ptr(),
                ml_meth: Some(class_check),
                ml_flags: METH_O,
                ml_doc: ptr::null(),
            },
            std::mem::zeroed(),
        ];
        let mut meta = NativeType::subtype(&raw mut PyType_Type, c"NativeConsumerCheckMeta");
        meta.tp_methods = methods.as_mut_ptr();
        assert_eq!(meta.ready(), 0);
        let mut class = NativeType::subtype(&raw mut PyExc_ValueError, c"NativeConsumerException");
        class.ob_base.ob_base.ob_type = &raw mut *meta;
        assert_eq!(class.ready(), 0);
        {
            let class = ExceptionValue::adopt(
                py,
                GLOBAL_BRIDGE
                    .molt_value_for_pyobj((&raw mut *class).cast())
                    .unwrap(),
            );
            let actual = crate::builtins::exceptions::exception_class(py, input.bits()).unwrap();
            let lookup = crate::exception_type_bits_from_name(py, "LookupError");
            assert!(crate::isinstance_runtime(py, input.bits(), lookup));
            assert!(crate::issubclass_runtime(py, actual.bits(), lookup));
            CHECK_MODE.store(0, Ordering::SeqCst);
            CHECK_CALLS.store(0, Ordering::SeqCst);
            assert!(crate::isinstance_runtime(py, input.bits(), class.bits()));
            assert!(crate::issubclass_runtime(py, actual.bits(), class.bits()));
            assert_eq!(CHECK_CALLS.load(Ordering::SeqCst), 2);
            let tuple = ExceptionValue::adopt(
                py,
                MoltObject::from_ptr(crate::alloc_tuple(
                    py,
                    &[class.bits(), MoltObject::from_int(3).bits()],
                ))
                .bits(),
            );
            assert!(crate::isinstance_runtime(py, input.bits(), tuple.bits()));
            assert!(crate::issubclass_runtime(py, actual.bits(), tuple.bits()));
            assert!(
                !crate::exception_pending(py),
                "successful first member skips invalid later classinfo"
            );
            let invalid_first = ExceptionValue::adopt(
                py,
                MoltObject::from_ptr(crate::alloc_tuple(
                    py,
                    &[MoltObject::from_int(3).bits(), class.bits()],
                ))
                .bits(),
            );
            CHECK_CALLS.store(0, Ordering::SeqCst);
            assert!(!crate::isinstance_runtime(
                py,
                input.bits(),
                invalid_first.bits()
            ));
            assert!(crate::exception_pending(py));
            assert_eq!(CHECK_CALLS.load(Ordering::SeqCst), 0);
            crate::clear_exception(py);

            CHECK_MODE.store(2, Ordering::SeqCst);
            let carrier = ExceptionValue::adopt(
                py,
                MoltObject::from_ptr(crate::alloc_tuple(py, &[class.bits(), input.bits()])).bits(),
            );
            let normalized = ExceptionValue::adopt(
                py,
                crate::async_rt::throw_protocol::normalize_throw_argument(py, carrier.bits())
                    .unwrap(),
            );
            assert_eq!(
                normalized.bits(),
                failure.bits(),
                "throw injects the exact subclass-hook failure"
            );
            assert!(!crate::exception_pending(py));
            crate::builtins::contextlib::molt_contextlib_suppress_match(
                actual.bits(),
                tuple.bits(),
            );
            assert!(crate::exception_pending(py));
            assert_eq!(crate::exception_last_bits_noinc(py), Some(failure.bits()));
            crate::clear_exception(py);
        }
        CHECK_MODE.store(0, Ordering::SeqCst);
        CHECK_FAILURE.store(ptr::null_mut(), Ordering::SeqCst);
    });
}

static EXIT_EXCEPTION: AtomicPtr<PyObject> = AtomicPtr::new(ptr::null_mut());
static EXIT_TRACEBACK: AtomicPtr<PyObject> = AtomicPtr::new(ptr::null_mut());
static EXIT_CALLS: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn enter_context(_self: *mut PyObject, _args: *mut PyObject) -> *mut PyObject {
    unsafe { object::Py_NewRef(&raw mut Py_None) }
}

unsafe extern "C" fn exit_context(_self: *mut PyObject, args: *mut PyObject) -> *mut PyObject {
    unsafe {
        let exception = EXIT_EXCEPTION.load(Ordering::SeqCst);
        let traceback = EXIT_TRACEBACK.load(Ordering::SeqCst);
        assert_eq!(sequences::PyTuple_Size(args), 3);
        assert_eq!(
            sequences::PyTuple_GetItem(args, 0),
            (*exception).ob_type.cast()
        );
        assert_eq!(sequences::PyTuple_GetItem(args, 1), exception);
        assert_eq!(sequences::PyTuple_GetItem(args, 2), traceback);
        let refs = (*traceback).ob_refcnt;
        assert!(
            refs >= 2,
            "the exception field and exit argument both own the traceback"
        );
        let old = OwnedPyObject::from_owned(std::mem::replace(
            &mut (*exception.cast::<PyBaseExceptionObject>()).traceback,
            ptr::null_mut(),
        ));
        assert_eq!(old.as_ptr(), traceback);
        drop(old);
        assert_eq!(
            (*traceback).ob_refcnt,
            refs - 1,
            "exit's traceback argument owns its replaced native field"
        );
        let line = OwnedPyObject::from_owned(object::PyObject_GetAttrString(
            traceback,
            c"tb_lineno".as_ptr(),
        ));
        assert!(!line.as_ptr().is_null());
        assert_eq!(numbers::PyLong_AsLong(line.as_ptr()), 7);
        EXIT_CALLS.fetch_add(1, Ordering::SeqCst);
        object::Py_NewRef((&raw mut Py_False).cast())
    }
}

/// Materialize a real traceback from the production frame snapshot authority.
/// The returned C owner is transferred to the native exception before __exit__.
unsafe fn context_traceback(py: &PyToken<'_>) -> OwnedPyObject {
    unsafe {
        let filename = ExceptionValue::adopt(
            py,
            MoltObject::from_ptr(crate::alloc_string(py, b"<native-context-exit>")).bits(),
        );
        let name = ExceptionValue::adopt(
            py,
            MoltObject::from_ptr(crate::alloc_string(py, b"native_context_exit")).bits(),
        );
        let empty =
            ExceptionValue::adopt(py, MoltObject::from_ptr(crate::alloc_tuple(py, &[])).bits());
        let code = crate::alloc_code_obj(
            py,
            filename.bits(),
            name.bits(),
            7,
            MoltObject::none().bits(),
            empty.bits(),
            empty.bits(),
            0,
            0,
            0,
        );
        assert!(!code.is_null());
        crate::builtins::frames::frame_stack_push_owned(
            py,
            MoltObject::from_ptr(code).bits(),
            0,
            0,
            0,
        );
        let payload = crate::builtins::frames::frame_stack_trace_payload_bits(py, None, false);
        crate::builtins::frames::frame_stack_pop(py);
        let payload = ExceptionValue::adopt(py, payload.expect("captured context frame"));
        let traceback = ExceptionValue::adopt(
            py,
            crate::builtins::frames::traceback_payload_to_traceback_bits(py, payload.bits()),
        );
        assert!(!MoltObject::from_bits(traceback.bits()).is_none());
        let traceback =
            OwnedPyObject::from_owned(GLOBAL_BRIDGE.owned_handle_to_pyobj(traceback.into_bits()));
        assert!(!traceback.as_ptr().is_null());
        assert_eq!(
            object::Py_TYPE(traceback.as_ptr()),
            &raw mut PyTraceBack_Type
        );
        traceback
    }
}

#[test]
fn native_context_exit_owns_actual_exception_class_and_mutated_traceback() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let _callbacks = CallbackState::new();
    assert!(super::register_cpython_hooks());
    let _execution = crate::concurrency::RuntimeExecutionGuard::enter();
    crate::concurrency::gil::with_gil(|py| unsafe {
        let py = &py;
        let exception = native_exception(py, &raw mut PyExc_ValueError, c"context input");
        let native = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(exception.bits());
        let trace_owner = context_traceback(py);
        let traceback = trace_owner.as_ptr();
        assert_eq!(errors::PyException_SetTraceback(native, traceback), 0);
        drop(trace_owner);
        EXIT_EXCEPTION.store(native, Ordering::SeqCst);
        EXIT_TRACEBACK.store(traceback, Ordering::SeqCst);
        let mut methods = [
            PyMethodDef {
                ml_name: c"__enter__".as_ptr(),
                ml_meth: Some(enter_context),
                ml_flags: METH_NOARGS,
                ml_doc: ptr::null(),
            },
            PyMethodDef {
                ml_name: c"__exit__".as_ptr(),
                ml_meth: Some(exit_context),
                ml_flags: METH_VARARGS,
                ml_doc: ptr::null(),
            },
            std::mem::zeroed(),
        ];
        let mut cm_type = NativeType::subtype(&raw mut PyBaseObject_Type, c"NativeConsumerContext");
        cm_type.tp_methods = methods.as_mut_ptr();
        assert_eq!(cm_type.ready(), 0);
        assert!(
            cm_type.tp_dealloc.is_some(),
            "object subtype inherits its production deallocator"
        );
        {
            let cm = OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(&raw mut *cm_type, 0));
            assert!(!cm.as_ptr().is_null());
            let cm_bits =
                ExceptionValue::adopt(py, GLOBAL_BRIDGE.molt_value_for_pyobj(cm.as_ptr()).unwrap());
            drop(cm);
            let entered = ExceptionValue::adopt(py, crate::molt_context_enter(cm_bits.bits()));
            assert!(!crate::exception_pending(py));
            drop(entered);
            let result = ExceptionValue::adopt(
                py,
                crate::molt_context_exit(cm_bits.bits(), exception.bits()),
            );
            assert!(!crate::exception_pending(py));
            assert!(!crate::is_truthy(py, crate::obj_from_bits(result.bits())));
        }
        assert_eq!(EXIT_CALLS.load(Ordering::SeqCst), 1);
        assert!(
            (*native.cast::<PyBaseExceptionObject>())
                .traceback
                .is_null()
        );
        assert!(
            GLOBAL_BRIDGE.molt_handle_for_pyobj(traceback).is_none(),
            "the field and call have released the traceback's final C view owner"
        );
        EXIT_EXCEPTION.store(ptr::null_mut(), Ordering::SeqCst);
        EXIT_TRACEBACK.store(ptr::null_mut(), Ordering::SeqCst);
    });
}
