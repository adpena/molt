//! Matching must consume actual managed/native class identity, including native
//! class objects whose metaclass is a subtype of type. These fixtures allocate
//! native exception storage directly; a normal C call can return a managed view.

use super::*;
use molt_cpython_abi::abi_types::*;
use molt_cpython_abi::api::{errors, memory, object, refcount, sequences};
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
use std::ffi::CStr;
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(feature = "stdlib_http")]
mod http_exception_boundary_tests {
    use super::*;
    use molt_runtime_http::{bridge as http, functions_http as server};
    use std::cell::RefCell;

    fn assert_active(py: &PyToken<'_>, expected: u64) {
        // sys.exception() obtains this intrinsic; the C handled query and
        // sys.exc_info() must observe the same original instance.
        let active = ExceptionValue::adopt(py, exception_state_abi::molt_exception_active());
        assert_eq!(active.bits(), expected);
        unsafe {
            let active = errors::PyErr_GetHandledException();
            let active_owner = refcount::OwnedPyObject::from_owned(active);
            assert_eq!(active, GLOBAL_BRIDGE.handle_to_borrowed_pyobj(expected));
            drop(active_owner);
        }
    }

    #[test]
    fn http_leaf_pending_match_and_scopes_preserve_distinct_outer_handler() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        assert!(crate::cpython_abi_hooks::register_cpython_hooks());
        crate::concurrency::gil::with_gil(|py| unsafe {
            let py = &py;
            molt_runtime_core::with_core_gil!(core_py, {
                let outer = ExceptionValue::adopt(py, managed_exception(py, "KeyError"));
                let _outer_handler = ExceptionStackScope::push(py);
                exception_context_set(py, outer.bits());
                let entry_depth = exception_stack_depth();
                for (base, name, timeout, os_error, ordinary, import_error) in [
                    (
                        &raw mut PyExc_TimeoutError,
                        c"KeyboardInterrupt",
                        true,
                        true,
                        true,
                        false,
                    ),
                    (
                        &raw mut PyExc_BaseException,
                        c"TimeoutError",
                        false,
                        false,
                        false,
                        false,
                    ),
                    (
                        &raw mut PyExc_ModuleNotFoundError,
                        c"OtherError",
                        false,
                        false,
                        true,
                        true,
                    ),
                ] {
                    let mut class = native_subclass(base, name);
                    let native = native_exception(&raw mut *class);
                    let raised = ExceptionValue::adopt(py, foreign_bits(native));
                    record_exception(py, obj_from_bits(raised.bits()).as_ptr().unwrap());
                    let refs = (*native).ob_refcnt;
                    let wrapper_refs =
                        (*header_from_obj_ptr(obj_from_bits(raised.bits()).as_ptr().unwrap()))
                            .ref_count_snapshot();
                    for (target, expected) in [
                        ("TimeoutError", timeout),
                        ("OSError", os_error),
                        ("Exception", ordinary),
                        ("ImportError", import_error),
                    ] {
                        assert_eq!(
                            http::pending_exception_matches_builtin(core_py, target),
                            expected
                        );
                        assert_eq!(exception_last_bits_noinc(py), Some(raised.bits()));
                        assert_active(py, outer.bits());
                        assert!(exception_pending(py));
                        assert_eq!((*native).ob_refcnt, refs);
                    }
                    assert_eq!(
                        http::with_saved_exception(core_py, || {
                            assert!(!exception_pending(py));
                            assert_active(py, raised.bits());
                            Ok(17)
                        }),
                        Ok(17)
                    );
                    assert_eq!(exception_last_bits_noinc(py), Some(raised.bits()));
                    assert_eq!(exception_stack_depth(), entry_depth);
                    assert_active(py, outer.bits());
                    assert_eq!(
                        (*header_from_obj_ptr(obj_from_bits(raised.bits()).as_ptr().unwrap()))
                            .ref_count_snapshot(),
                        wrapper_refs
                    );

                    let replacement =
                        ExceptionValue::adopt(py, managed_exception(py, "LookupError"));
                    assert!(
                        http::with_saved_exception::<()>(core_py, || {
                            assert_active(py, raised.bits());
                            record_exception(
                                py,
                                obj_from_bits(replacement.bits()).as_ptr().unwrap(),
                            );
                            Err(MoltObject::none().bits())
                        })
                        .is_err()
                    );
                    assert_eq!(exception_last_bits_noinc(py), Some(replacement.bits()));
                    assert_eq!(
                        exception_field(py, replacement.bits(), ExceptionFieldSlot::Context)
                            .unwrap()
                            .bits(),
                        raised.bits()
                    );
                    assert_active(py, outer.bits());
                    clear_exception(py);

                    record_exception(py, obj_from_bits(raised.bits()).as_ptr().unwrap());
                    let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        http::with_saved_exception::<()>(core_py, || {
                            record_exception(
                                py,
                                obj_from_bits(replacement.bits()).as_ptr().unwrap(),
                            );
                            panic!("HTTP callback unwind");
                        })
                    }));
                    assert!(unwind.is_err());
                    assert_eq!(exception_last_bits_noinc(py), Some(raised.bits()));
                    assert_eq!(exception_stack_depth(), entry_depth);
                    assert_active(py, outer.bits());
                    assert!(
                        http::with_handled_exception(core_py, || {
                            assert!(!exception_pending(py));
                            assert_active(py, raised.bits());
                            Ok(())
                        })
                        .is_ok()
                    );
                    assert!(!exception_pending(py));
                    assert_active(py, outer.bits());
                    drop(replacement);
                    drop(raised);
                    refcount::Py_DECREF(native);
                }
                assert!(errors::PyErr_Occurred().is_null());
            });
        });
    }

    #[derive(Default)]
    struct Callbacks {
        tuple: u64,
        raised: u64,
        handler_failure: u64,
        cleanup_failure: u64,
        probe_failure: u64,
        cleanup_active: u64,
        outer: u64,
        handled: usize,
        closed: usize,
        serviced: usize,
    }

    thread_local! {
        static CALLBACKS: RefCell<Callbacks> = RefCell::new(Callbacks::default());
    }

    fn raise_fixture(py: &PyToken<'_>, bits: u64) -> u64 {
        if bits != 0 {
            record_exception(py, obj_from_bits(bits).as_ptr().unwrap());
        }
        MoltObject::none().bits()
    }

    extern "C" fn get_request(_: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            let tuple = CALLBACKS.with(|state| state.borrow().tuple);
            inc_ref_bits(py, tuple);
            tuple
        })
    }

    extern "C" fn process_request(_: u64, _: u64, _: u64) -> u64 {
        serve_request(MoltObject::none().bits())
    }

    extern "C" fn serve_request(_: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            let raised = CALLBACKS.with(|state| state.borrow().raised);
            raise_fixture(py, raised)
        })
    }

    extern "C" fn handle_error(_: u64, _: u64, _: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            let (raised, failure) = CALLBACKS.with(|state| {
                let mut state = state.borrow_mut();
                state.handled += 1;
                (state.raised, state.handler_failure)
            });
            assert!(!exception_pending(py));
            assert_active(py, raised);
            raise_fixture(py, failure)
        })
    }

    extern "C" fn close_request(_: u64, _: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            let (active, failure) = CALLBACKS.with(|state| {
                let mut state = state.borrow_mut();
                state.closed += 1;
                (state.cleanup_active, state.cleanup_failure)
            });
            assert!(!exception_pending(py));
            assert_active(py, active);
            raise_fixture(py, failure)
        })
    }

    extern "C" fn response_bytes(_: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            MoltObject::from_ptr(alloc_bytes(py, b"HTTP/1.0 200 OK\r\n\r\n")).bits()
        })
    }

    extern "C" fn probe_failure(_: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            let (raised, failure) = CALLBACKS.with(|state| {
                let state = state.borrow();
                (state.raised, state.probe_failure)
            });
            assert!(!exception_pending(py));
            assert_active(py, raised);
            raise_fixture(py, failure)
        })
    }

    extern "C" fn missing_attribute(value: u64, _: u64) -> u64 {
        probe_failure(value)
    }

    fn set_attr(py: &PyToken<'_>, instance: u64, name: &[u8], value: u64) {
        let name = ExceptionValue::adopt(py, attr_name_bits_from_bytes(py, name).unwrap());
        crate::molt_object_setattr(instance, name.bits(), value);
        assert!(!exception_pending(py));
    }

    extern "C" fn service_actions(instance: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            let outer = CALLBACKS.with(|state| {
                let mut state = state.borrow_mut();
                state.serviced += 1;
                state.outer
            });
            assert_active(py, outer);
            set_attr(
                py,
                instance,
                b"_molt_shutdown_request",
                MoltObject::from_bool(true).bits(),
            );
            MoltObject::none().bits()
        })
    }

    fn instance_with_methods<'a, 'py>(
        py: &'a PyToken<'py>,
        methods: &[(&[u8], *const (), u64)],
    ) -> ExceptionValue<'a, 'py> {
        let name = ExceptionValue::adopt(
            py,
            attr_name_bits_from_bytes(py, b"HttpBoundaryFixture").unwrap(),
        );
        let class = ExceptionValue::adopt(py, crate::molt_class_new(name.bits()));
        crate::molt_class_set_base(class.bits(), builtin_classes(py).object);
        for &(name, callback, arity) in methods {
            let name = ExceptionValue::adopt(py, attr_name_bits_from_bytes(py, name).unwrap());
            let function = ExceptionValue::adopt(
                py,
                MoltObject::from_ptr(crate::builtins::functions::alloc_runtime_function_obj(
                    py,
                    crate::provenance::abi::expose_function_address(callback),
                    arity,
                ))
                .bits(),
            );
            crate::molt_set_attr_name(class.bits(), name.bits(), function.bits());
        }
        unsafe {
            crate::object::class_finish_definition(
                py,
                obj_from_bits(class.bits()).as_ptr().unwrap(),
            )
            .unwrap();
        }
        let instance =
            ExceptionValue::adopt(py, unsafe { crate::call_callable0(py, class.bits()) });
        assert!(!exception_pending(py));
        instance
    }

    #[test]
    fn http_handle_request_scopes_handler_cleanup_and_response_early_returns() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        assert!(crate::cpython_abi_hooks::register_cpython_hooks());
        crate::concurrency::gil::with_gil(|py| unsafe {
            let py = &py;
            let outer = ExceptionValue::adopt(py, managed_exception(py, "KeyError"));
            let _outer_handler = ExceptionStackScope::push(py);
            exception_context_set(py, outer.bits());
            let entry_depth = exception_stack_depth();
            let server = instance_with_methods(
                py,
                &[
                    (b"get_request", get_request as *const (), 1),
                    (b"process_request", process_request as *const (), 3),
                    (b"handle_error", handle_error as *const (), 3),
                    (b"close_request", close_request as *const (), 2),
                ],
            );
            let request =
                instance_with_methods(py, &[(b"response_bytes", response_bytes as *const (), 1)]);
            for (ordinary, handler_fails, cleanup_fails, request_id) in [
                (true, false, false, -1),
                (false, false, false, -1),
                (false, false, false, 1_234_567),
                (true, true, false, 1_234_567),
                (false, false, true, -1),
                (true, true, true, -1),
            ] {
                let native = native_exception(if ordinary {
                    &raw mut PyExc_ValueError
                } else {
                    &raw mut PyExc_BaseException
                });
                let raised = ExceptionValue::adopt(py, foreign_bits(native));
                refcount::Py_DECREF(native);
                let handler_failure =
                    ExceptionValue::adopt(py, managed_exception(py, "LookupError"));
                let cleanup_failure = ExceptionValue::adopt(py, managed_exception(py, "OSError"));
                let tuple = ExceptionValue::adopt(
                    py,
                    MoltObject::from_ptr(alloc_tuple(
                        py,
                        &[
                            request.bits(),
                            MoltObject::none().bits(),
                            MoltObject::from_int(request_id).bits(),
                        ],
                    ))
                    .bits(),
                );
                CALLBACKS.with(|state| {
                    *state.borrow_mut() = Callbacks {
                        tuple: tuple.bits(),
                        raised: raised.bits(),
                        handler_failure: if handler_fails {
                            handler_failure.bits()
                        } else {
                            0
                        },
                        cleanup_failure: if cleanup_fails {
                            cleanup_failure.bits()
                        } else {
                            0
                        },
                        cleanup_active: if handler_fails {
                            handler_failure.bits()
                        } else {
                            raised.bits()
                        },
                        outer: outer.bits(),
                        ..Callbacks::default()
                    }
                });
                server::molt_socketserver_handle_request(server.bits());
                let expected = if cleanup_fails {
                    Some(cleanup_failure.bits())
                } else if handler_fails {
                    Some(handler_failure.bits())
                } else if ordinary {
                    None
                } else {
                    Some(raised.bits())
                };
                assert_eq!(exception_last_bits_noinc(py), expected);
                assert_eq!(exception_pending(py), expected.is_some());
                assert_active(py, outer.bits());
                assert_eq!(exception_stack_depth(), entry_depth);
                CALLBACKS.with(|state| {
                    assert_eq!(state.borrow().handled, usize::from(ordinary));
                    assert_eq!(state.borrow().closed, 1);
                });
                if handler_fails {
                    assert_eq!(
                        exception_field(py, handler_failure.bits(), ExceptionFieldSlot::Context)
                            .unwrap()
                            .bits(),
                        raised.bits()
                    );
                }
                if cleanup_fails {
                    assert_eq!(
                        exception_field(py, cleanup_failure.bits(), ExceptionFieldSlot::Context)
                            .unwrap()
                            .bits(),
                        if handler_fails {
                            handler_failure.bits()
                        } else {
                            raised.bits()
                        }
                    );
                }
                clear_exception(py);
                CALLBACKS.with(|state| *state.borrow_mut() = Callbacks::default());
                drop(cleanup_failure);
                drop(handler_failure);
                assert_eq!(
                    (*header_from_obj_ptr(obj_from_bits(raised.bits()).as_ptr().unwrap()))
                        .ref_count_snapshot(),
                    1
                );
            }
        });
    }

    #[test]
    fn http_serve_oserror_probes_prioritize_lookup_and_truthiness_failures() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        assert!(crate::cpython_abi_hooks::register_cpython_hooks());
        crate::concurrency::gil::with_gil(|py| unsafe {
            let py = &py;
            let outer = ExceptionValue::adopt(py, managed_exception(py, "KeyError"));
            let _outer_handler = ExceptionStackScope::push(py);
            exception_context_set(py, outer.bits());
            for probe in ["success", "lookup", "truthiness"] {
                let native = native_exception(&raw mut PyExc_FileNotFoundError);
                let raised = ExceptionValue::adopt(py, foreign_bits(native));
                refcount::Py_DECREF(native);
                let failure = ExceptionValue::adopt(py, managed_exception(py, "LookupError"));
                let mut methods: Vec<(&[u8], *const (), u64)> = vec![
                    (b"handle_request", serve_request as *const (), 1),
                    (b"handle_error", handle_error as *const (), 3),
                    (b"service_actions", service_actions as *const (), 1),
                ];
                if probe == "lookup" {
                    methods.push((b"__getattr__", missing_attribute as *const (), 2));
                }
                let server = instance_with_methods(py, &methods);
                set_attr(
                    py,
                    server.bits(),
                    b"_molt_shutdown_request",
                    MoltObject::from_bool(false).bits(),
                );
                if probe == "truthiness" {
                    let closed =
                        instance_with_methods(py, &[(b"__bool__", probe_failure as *const (), 1)]);
                    set_attr(py, server.bits(), b"_closed", closed.bits());
                } else if probe == "success" {
                    set_attr(
                        py,
                        server.bits(),
                        b"_closed",
                        MoltObject::from_bool(false).bits(),
                    );
                }
                CALLBACKS.with(|state| {
                    *state.borrow_mut() = Callbacks {
                        raised: raised.bits(),
                        probe_failure: failure.bits(),
                        outer: outer.bits(),
                        ..Callbacks::default()
                    }
                });
                server::molt_socketserver_serve_forever(
                    server.bits(),
                    MoltObject::from_float(0.0).bits(),
                );
                assert_active(py, outer.bits());
                if probe == "success" {
                    assert!(!exception_pending(py));
                    CALLBACKS.with(|state| assert_eq!(state.borrow().handled, 1));
                } else {
                    assert_eq!(exception_last_bits_noinc(py), Some(failure.bits()));
                    assert_eq!(
                        exception_field(py, failure.bits(), ExceptionFieldSlot::Context)
                            .unwrap()
                            .bits(),
                        raised.bits()
                    );
                    CALLBACKS.with(|state| assert_eq!(state.borrow().handled, 0));
                }
                clear_exception(py);
                CALLBACKS.with(|state| *state.borrow_mut() = Callbacks::default());
            }
        });
    }
}

#[test]
fn native_error_roundtrip_retains_identity_and_owned_metadata() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(crate::cpython_abi_hooks::register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            for class in [&raw mut PyExc_ValueError, &raw mut PyExc_StopIteration] {
                let native = native_exception(class);
                let bits = foreign_bits(native);
                let context =
                    MoltObject::from_ptr(alloc_exception(py, "LookupError", "context")).bits();
                exception_replace_field_bits(py, bits, ExceptionFieldSlot::Context, context)
                    .unwrap();
                errors::PyErr_SetObject(class.cast(), native);
                assert!(crate::cpython_abi_hooks::transfer_pending_cpython_exception());
                assert_eq!(exception_last_bits_noinc(py), Some(bits));
                assert_eq!(
                    errors::PyErr_Occurred(),
                    class.cast(),
                    "pending class query must retain native identity"
                );
                assert_eq!(
                    exception_field(py, bits, ExceptionFieldSlot::Context)
                        .unwrap()
                        .bits(),
                    context
                );
                let mut exc = ptr::null_mut();
                let mut value = ptr::null_mut();
                let mut traceback = ptr::null_mut();
                errors::PyErr_Fetch(&raw mut exc, &raw mut value, &raw mut traceback);
                let exc_owner = refcount::OwnedPyObject::from_owned(exc);
                let value_owner = refcount::OwnedPyObject::from_owned(value);
                let traceback_owner = refcount::OwnedPyObject::from_owned(traceback);
                assert_eq!(exc, class.cast());
                assert_eq!(
                    value, native,
                    "C-to-runtime-to-C must not reconstruct the instance"
                );
                assert!(!exception_pending(py));
                errors::PyErr_Restore(
                    exc_owner.into_ptr(),
                    value_owner.into_ptr(),
                    traceback_owner.into_ptr(),
                );
                assert!(crate::cpython_abi_hooks::transfer_pending_cpython_exception());
                assert_eq!(exception_last_bits_noinc(py), Some(bits));
                exception_stack_push();
                let handled = exception_state_abi::molt_exception_enter_handler(bits);
                assert_eq!(handled, bits);
                assert_eq!(exception_context_active_bits(), Some(bits));
                assert!(!exception_pending(py));
                exception_stack_pop(py);
                dec_ref_bits(py, handled);
                molt_raise(bits);
                assert_eq!(exception_last_bits_noinc(py), Some(bits));
                let mut saved = Some(take_raised(py));
                assert!(!exception_pending(py));
                resolve_raised(py, &mut saved);
                assert_eq!(exception_last_bits_noinc(py), Some(bits));
                clear_exception(py);
                exception_replace_field_bits(
                    py,
                    bits,
                    ExceptionFieldSlot::Context,
                    MoltObject::none().bits(),
                )
                .unwrap();
                dec_ref_bits(py, context);
                dec_ref_bits(py, bits);
                refcount::Py_DECREF(native);
                assert!(errors::PyErr_Occurred().is_null());
            }
        }
    });
}

#[test]
fn native_pending_class_borrows_custom_type_without_creating_a_wrapper() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(crate::cpython_abi_hooks::register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let mut class = native_subclass(&raw mut PyExc_ValueError, c"NativeRaisedError");
            let native = native_exception(&raw mut *class);
            let bits = foreign_bits(native);
            record_exception(py, obj_from_bits(bits).as_ptr().unwrap());
            let refs = class.ob_base.ob_base.ob_refcnt;
            for _ in 0..5 {
                assert_eq!(errors::PyErr_Occurred(), (&raw mut *class).cast());
                assert_eq!(class.ob_base.ob_base.ob_refcnt, refs);
                assert_eq!(exception_last_bits_noinc(py), Some(bits));
            }
            let actual = exception_class(py, bits).unwrap();
            assert_eq!(
                GLOBAL_BRIDGE.handle_to_borrowed_pyobj(actual.bits()),
                (&raw mut *class).cast()
            );
            drop(actual);
            clear_exception(py);
            dec_ref_bits(py, bits);
            refcount::Py_DECREF(native);
            assert_eq!(class.ob_base.ob_base.ob_refcnt, 1);
        }
    });
}

static IDENTITY_LOOKUPS: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static TYPED_UPDATE_RECEIVER: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static TYPED_INPUT_DROPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static TYPED_DROPS_DURING_INDEX: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

extern "C" fn typed_input_finalize(_: u64) -> u64 {
    TYPED_INPUT_DROPS.with(|count| count.set(count.get() + 1));
    MoltObject::none().bits()
}

extern "C" fn reentrant_typed_index(_: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if let Err(message) = exception_typed_field_replace_internal(
            py,
            TYPED_UPDATE_RECEIVER.with(std::cell::Cell::get),
            ExceptionTypedField::OSErrorFilename,
            MoltObject::none().bits(),
        ) {
            return raise_exception::<u64>(py, "SystemError", message);
        }
        TYPED_DROPS_DURING_INDEX
            .with(|count| count.set(TYPED_INPUT_DROPS.with(std::cell::Cell::get)));
        MoltObject::from_int(4).bits()
    })
}

fn typed_callback_class(py: &PyToken<'_>, name: &[u8], callback: *const ()) -> u64 {
    let class_name = attr_name_bits_from_bytes(py, b"TypedUpdateCallback").unwrap();
    let class = crate::molt_class_new(class_name);
    dec_ref_bits(py, class_name);
    crate::molt_class_set_base(class, builtin_classes(py).object);
    let name = attr_name_bits_from_bytes(py, name).unwrap();
    let function = MoltObject::from_ptr(crate::builtins::functions::alloc_runtime_function_obj(
        py,
        crate::provenance::abi::expose_function_address(callback),
        1,
    ))
    .bits();
    crate::molt_set_attr_name(class, name, function);
    dec_ref_bits(py, name);
    dec_ref_bits(py, function);
    unsafe {
        crate::object::class_finish_definition(py, obj_from_bits(class).as_ptr().unwrap()).unwrap();
    }
    assert!(!exception_pending(py));
    class
}

#[test]
fn typed_batches_pin_later_inputs_and_retire_values_after_reentrant_conversion() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    assert!(crate::cpython_abi_hooks::register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let tracked_class =
                typed_callback_class(py, b"__del__", typed_input_finalize as *const ());
            let index_class =
                typed_callback_class(py, b"__index__", reentrant_typed_index as *const ());
            let indexer = crate::call_callable0(py, index_class);
            let native = native_exception(&raw mut PyExc_OSError);
            let native_bits = foreign_bits(native);
            let managed = managed_exception(py, "OSError");
            for receiver in [managed, native_bits] {
                TYPED_UPDATE_RECEIVER.with(|slot| slot.set(receiver));
                for later_input in [true, false] {
                    TYPED_INPUT_DROPS.with(|count| count.set(0));
                    let tracked = crate::call_callable0(py, tracked_class);
                    exception_typed_field_replace_internal(
                        py,
                        receiver,
                        ExceptionTypedField::OSErrorFilename,
                        tracked,
                    )
                    .unwrap();
                    dec_ref_bits(py, tracked); // the receiver is now its sole owner
                    let updates = if later_input {
                        [
                            (ExceptionTypedField::OSErrorCharactersWritten, indexer),
                            (ExceptionTypedField::OSErrorFilename, tracked),
                        ]
                    } else {
                        [
                            (
                                ExceptionTypedField::OSErrorFilename,
                                MoltObject::none().bits(),
                            ),
                            (ExceptionTypedField::OSErrorCharactersWritten, indexer),
                        ]
                    };
                    exception_typed_fields_replace_internal(py, receiver, &updates).unwrap();
                    assert!(!exception_pending(py));
                    assert_eq!(
                        TYPED_DROPS_DURING_INDEX.with(std::cell::Cell::get),
                        usize::from(!later_input)
                    );
                    if later_input {
                        let stored = ExceptionStorage::for_exception(py, receiver)
                            .unwrap()
                            .typed_field(py, ExceptionTypedField::OSErrorFilename)
                            .unwrap();
                        assert_eq!(stored.bits(), tracked);
                    }
                    exception_typed_field_replace_internal(
                        py,
                        receiver,
                        ExceptionTypedField::OSErrorFilename,
                        MoltObject::none().bits(),
                    )
                    .unwrap();
                    assert_eq!(TYPED_INPUT_DROPS.with(std::cell::Cell::get), 1);
                }
            }
            TYPED_UPDATE_RECEIVER.with(|slot| slot.set(0));
            for bits in [managed, native_bits, indexer] {
                dec_ref_bits(py, bits);
            }
            refcount::Py_DECREF(native);
            for class in [tracked_class, index_class] {
                // Published user classes retire through the ordinary RC/GC
                // lifecycle owned by this transaction, with their identity intact
                // while callbacks and namespace contents are released.
                dec_ref_bits(py, class);
            }
            assert!(!exception_pending(py));
        }
    });
}

#[test]
fn native_typed_fields_preserve_defaults_atomic_updates_and_pending_identity() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(crate::cpython_abi_hooks::register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let exit = native_exception(&raw mut PyExc_SystemExit);
            let exit_bits = foreign_bits(exit);
            assert_eq!(
                system_exit_code(py, obj_from_bits(exit_bits).as_ptr().unwrap()),
                0
            );
            assert!(!exception_pending(py));

            let stop = native_exception(&raw mut PyExc_StopIteration);
            let stop_bits = foreign_bits(stop);
            let pending = managed_exception(py, "LookupError");
            record_exception(py, obj_from_bits(pending).as_ptr().unwrap());
            molt_exception_set_value(stop_bits, MoltObject::from_int(42).bits());
            assert_eq!(exception_last_bits_noinc(py), Some(pending));
            let stop_storage = ExceptionStorage::for_exception(py, stop_bits).unwrap();
            assert_eq!(
                stop_storage
                    .typed_field(py, ExceptionTypedField::StopIterationValue)
                    .unwrap()
                    .bits(),
                MoltObject::from_int(42).bits()
            );
            let args = errors::PyException_GetArgs(stop);
            assert_eq!(
                sequences::PyTuple_Size(args),
                0,
                "value assignment leaves args unchanged"
            );
            refcount::Py_DECREF(args);
            clear_exception(py);

            let os_error = native_exception(&raw mut PyExc_OSError);
            let os_bits = foreign_bits(os_error);
            let storage = ExceptionStorage::for_exception(py, os_bits).unwrap();
            assert!(
                storage
                    .typed_field(py, ExceptionTypedField::OSErrorCharactersWritten)
                    .is_none()
            );
            assert!(exception_matches_builtin_name(
                py,
                exception_last_bits_noinc(py).unwrap(),
                "AttributeError"
            ));
            clear_exception(py);
            let text = MoltObject::from_ptr(alloc_string(py, b"file")).bits();
            let result = exception_typed_fields_replace_internal(
                py,
                os_bits,
                &[
                    (ExceptionTypedField::OSErrorFilename, text),
                    (ExceptionTypedField::OSErrorCharactersWritten, text),
                ],
            );
            assert!(result.is_err());
            assert!(exception_matches_builtin_name(
                py,
                exception_last_bits_noinc(py).unwrap(),
                "TypeError"
            ));
            clear_exception(py);
            assert!(
                obj_from_bits(
                    storage
                        .typed_field(py, ExceptionTypedField::OSErrorFilename)
                        .unwrap()
                        .bits()
                )
                .is_none(),
                "failed conversion cannot publish an earlier field"
            );
            exception_typed_fields_replace_internal(
                py,
                os_bits,
                &[
                    (ExceptionTypedField::OSErrorFilename, text),
                    (
                        ExceptionTypedField::OSErrorCharactersWritten,
                        MoltObject::from_int(4).bits(),
                    ),
                ],
            )
            .unwrap();
            assert_eq!(
                storage
                    .typed_field(py, ExceptionTypedField::OSErrorFilename)
                    .unwrap()
                    .bits(),
                text
            );
            assert_eq!(
                storage
                    .typed_field(py, ExceptionTypedField::OSErrorCharactersWritten)
                    .unwrap()
                    .bits(),
                MoltObject::from_int(4).bits()
            );
            for bits in [exit_bits, stop_bits, os_bits, pending, text] {
                dec_ref_bits(py, bits);
            }
            for object in [exit, stop, os_error] {
                refcount::Py_DECREF(object);
            }
            assert!(!exception_pending(py));
            assert!(errors::PyErr_Occurred().is_null());
        }
    });
}

unsafe extern "C" fn spoof_identity(_: *mut PyObject, _: *mut PyObject) -> *mut PyObject {
    IDENTITY_LOOKUPS.fetch_add(1, Ordering::SeqCst);
    unsafe { object::Py_NewRef((&raw mut PyExc_KeyError).cast()) }
}

/// A live C static subtype with the real physical base/slot edges. It has no
/// managed class registration, no copied tp_dict/tp_mro owners, and stays alive
/// until every native instance and foreign wrapper in the fixture is released.
unsafe fn native_subclass(base: *mut PyTypeObject, name: &'static CStr) -> Box<PyTypeObject> {
    let mut class: Box<PyTypeObject> = Box::new(unsafe { std::mem::zeroed() });
    unsafe {
        class.ob_base.ob_base.ob_refcnt = 1;
        class.ob_base.ob_base.ob_type = &raw mut PyType_Type;
        class.tp_name = name.as_ptr();
        class.tp_base = base;
        class.tp_basicsize = (*base).tp_basicsize;
        class.tp_itemsize = (*base).tp_itemsize;
        class.tp_flags = (*base).tp_flags & !(Py_TPFLAGS_HEAPTYPE | Py_TPFLAGS_IMMUTABLETYPE);
        class.tp_alloc = (*base).tp_alloc;
        class.tp_free = (*base).tp_free;
        class.tp_dealloc = (*base).tp_dealloc;
        class.tp_traverse = (*base).tp_traverse;
        class.tp_clear = (*base).tp_clear;
        class.tp_is_gc = (*base).tp_is_gc;
        class.tp_getattro = (*base).tp_getattro;
        class.tp_setattro = (*base).tp_setattro;
        class.tp_dictoffset = (*base).tp_dictoffset;
    }
    class
}

unsafe fn native_exception(class: *mut PyTypeObject) -> *mut PyObject {
    let value =
        unsafe { errors::molt_native_exception_new(class, ptr::null_mut(), ptr::null_mut()) };
    assert!(!value.is_null());
    assert!(GLOBAL_BRIDGE.molt_handle_for_pyobj(value).is_none());
    assert!(unsafe { errors::native_exception_instance(value) });
    value
}

unsafe fn foreign_bits(value: *mut PyObject) -> u64 {
    let bits = unsafe { GLOBAL_BRIDGE.molt_value_for_pyobj(value) }.expect("foreign wrapper");
    assert_eq!(
        unsafe { object_type_id(obj_from_bits(bits).as_ptr().unwrap()) },
        crate::TYPE_ID_FOREIGN,
        "the fixture must exercise native storage"
    );
    bits
}

/// Explicit native tuple allocation bypasses the runtime-backed PyTuple_New
/// path, so Python handler admission exercises its foreign tuple branch.
unsafe fn native_tuple(items: &[*mut PyObject]) -> *mut PyObject {
    unsafe {
        let tuple = memory::_PyObject_GC_NewVar(&raw mut PyTuple_Type, items.len() as Py_ssize_t)
            .cast::<PyObject>();
        assert!(!tuple.is_null());
        assert!(GLOBAL_BRIDGE.molt_handle_for_pyobj(tuple).is_none());
        for (index, &item) in items.iter().enumerate() {
            refcount::Py_INCREF(item);
            assert_eq!(
                sequences::PyTuple_SetItem(tuple, index as Py_ssize_t, item),
                0
            );
        }
        tuple
    }
}

fn managed_exception(py: &PyToken<'_>, name: &str) -> u64 {
    let value = alloc_exception(py, name, "identity");
    assert!(!value.is_null());
    MoltObject::from_ptr(value).bits()
}

#[test]
fn real_native_exception_subclasses_and_metaclasses_match_without_identity_hooks() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(crate::cpython_abi_hooks::register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            IDENTITY_LOOKUPS.store(0, Ordering::SeqCst);
            let mut meta = native_subclass(&raw mut PyType_Type, c"IdentityMeta");
            meta.tp_getattro = Some(spoof_identity);
            let mut base = native_subclass(&raw mut PyExc_ValueError, c"NativeValueBase");
            base.ob_base.ob_base.ob_type = &raw mut *meta;
            let mut child = native_subclass(&raw mut *base, c"NativeValueChild");
            child.ob_base.ob_base.ob_type = &raw mut *meta;
            child.tp_getattro = Some(spoof_identity);
            let native = native_exception(&raw mut *child);
            let native_bits = foreign_bits(native);
            let class_bits = foreign_bits((&raw mut *base).cast());
            let value_class = exception_type_bits_from_name(py, "ValueError");
            let key_class = exception_type_bits_from_name(py, "KeyError");
            let exception_class = builtin_classes(py).exception;

            assert!(exception_is_class(py, class_bits));
            assert!(exception_class_is_subtype(py, class_bits, value_class));
            assert!(exception_class_is_subtype(py, class_bits, exception_class));
            assert!(!exception_class_is_subtype(py, value_class, class_bits));
            assert!(exception_matches_type(py, native_bits, class_bits));
            assert!(exception_matches_type(py, native_bits, value_class));
            assert!(exception_matches_type(py, native_bits, exception_class));
            assert!(exception_matches_type(
                py,
                native_bits,
                builtin_classes(py).base_exception
            ));
            assert!(!exception_matches_type(py, native_bits, key_class));
            assert_eq!(
                errors::PyErr_GivenExceptionMatches(native, (&raw mut *base).cast()),
                1
            );
            assert_eq!(
                errors::PyErr_GivenExceptionMatches(native, (&raw mut PyExc_Exception).cast()),
                1
            );
            assert_eq!(
                errors::PyErr_GivenExceptionMatches(
                    (&raw mut *child).cast(),
                    (&raw mut *base).cast()
                ),
                1
            );

            let managed = managed_exception(py, "ValueError");
            let managed_view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(managed);
            assert_eq!(
                errors::PyErr_GivenExceptionMatches(
                    managed_view,
                    (&raw mut PyExc_Exception).cast()
                ),
                1
            );
            assert_eq!(
                errors::PyErr_GivenExceptionMatches(managed_view, (&raw mut *base).cast()),
                0
            );
            assert!(!exception_matches_type(py, managed, class_bits));
            assert_eq!(IDENTITY_LOOKUPS.load(Ordering::SeqCst), 0);
            assert!(errors::PyErr_Occurred().is_null());

            dec_ref_bits(py, managed);
            dec_ref_bits(py, class_bits);
            dec_ref_bits(py, native_bits);
            refcount::Py_DECREF(native);
            assert_eq!(base.ob_base.ob_base.ob_refcnt, 1);
        }
    });
}

#[test]
fn foreign_handler_tuples_validate_the_complete_flat_handler() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(crate::cpython_abi_hooks::register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let mut class = native_subclass(&raw mut PyExc_ValueError, c"NativeHandlerValue");
            let native = native_exception(&raw mut *class);
            let value = foreign_bits(native);
            let class_ptr = (&raw mut *class).cast::<PyObject>();
            let class_bits = foreign_bits(class_ptr);
            let tuple = native_tuple(&[(&raw mut PyExc_KeyError).cast(), class_ptr]);
            let tuple_bits = foreign_bits(tuple);
            assert_eq!(
                molt_exception_match_handler(value, class_bits),
                MoltObject::from_bool(true).bits()
            );
            assert_eq!(
                molt_exception_match_handler(value, tuple_bits),
                MoltObject::from_bool(true).bits()
            );

            let nested = native_tuple(&[tuple]);
            assert_eq!(errors::PyErr_GivenExceptionMatches(native, nested), 1);
            let nested_bits = foreign_bits(nested);
            let invalid = native_tuple(&[class_ptr, &raw mut Py_None]);
            let invalid_bits = foreign_bits(invalid);
            let managed_invalid =
                MoltObject::from_ptr(alloc_tuple(py, &[class_bits, MoltObject::none().bits()]))
                    .bits();
            for handler in [invalid_bits, nested_bits, managed_invalid] {
                assert!(obj_from_bits(molt_exception_match_handler(value, handler)).is_none());
                let pending = crate::exception_last_bits_noinc(py).expect("invalid handler raises");
                assert!(exception_matches_builtin_name(py, pending, "TypeError"));
                crate::clear_exception(py);
            }
            assert!(errors::PyErr_Occurred().is_null());
            for bits in [
                managed_invalid,
                invalid_bits,
                nested_bits,
                tuple_bits,
                class_bits,
                value,
            ] {
                dec_ref_bits(py, bits);
            }
            for pointer in [invalid, nested, tuple, native] {
                refcount::Py_DECREF(pointer);
            }
            assert_eq!(class.ob_base.ob_base.ob_refcnt, 1);
        }
    });
}

#[test]
fn spoofed_native_identity_cannot_enter_exception_matching_or_descriptors() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(crate::cpython_abi_hooks::register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            IDENTITY_LOOKUPS.store(0, Ordering::SeqCst);
            let mut plain = native_subclass(&raw mut PyBaseObject_Type, c"PretendException");
            plain.tp_getattro = Some(spoof_identity);
            let mut value = PyObject {
                ob_refcnt: 1,
                ob_type: &raw mut *plain,
            };
            let value_ptr = &raw mut value;
            let bits = foreign_bits(value_ptr);
            assert!(!exception_is_instance(py, bits));
            assert!(!exception_is_class(py, bits));
            assert!(!exception_matches_type(
                py,
                bits,
                builtin_classes(py).base_exception
            ));
            assert_eq!(
                errors::PyErr_GivenExceptionMatches(value_ptr, (&raw mut PyExc_KeyError).cast()),
                0
            );
            assert_eq!(errors::PyErr_GivenExceptionMatches(value_ptr, value_ptr), 1);
            assert_eq!(
                errors::PyErr_GivenExceptionMatches(
                    (&raw mut PyBool_Type).cast(),
                    (&raw mut PyLong_Type).cast()
                ),
                0
            );
            let method = exception_method_bits(py, "add_note").unwrap();
            inc_ref_bits(py, method);
            assert!(
                !crate::builtins::functions::native_callable::admit_native_call(
                    py,
                    obj_from_bits(method).as_ptr().unwrap(),
                    Some(bits),
                )
            );
            assert!(crate::exception_pending(py));
            crate::clear_exception(py);
            assert_eq!(IDENTITY_LOOKUPS.load(Ordering::SeqCst), 0);
            dec_ref_bits(py, method);
            dec_ref_bits(py, bits);
            assert_eq!(value.ob_refcnt, 1);
        }
    });
}

#[test]
fn native_classmethod_binding_and_public_type_follow_live_native_identity() {
    use crate::builtins::functions::native_callable::{
        native_callable_repr, native_descriptor_receiver,
    };
    use crate::builtins::methods::object_method_bits;
    use crate::{bound_method_self_bits, exception_pending};
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(crate::cpython_abi_hooks::register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            IDENTITY_LOOKUPS.store(0, Ordering::SeqCst);
            let mut first = native_subclass(&raw mut PyBaseObject_Type, c"NativeTypeFirst");
            let mut second = native_subclass(&raw mut PyBaseObject_Type, c"NativeTypeSecond");
            first.tp_getattro = Some(spoof_identity);
            second.tp_getattro = Some(spoof_identity);
            let mut value = PyObject {
                ob_refcnt: 1,
                ob_type: &raw mut *first,
            };
            let bits = foreign_bits(&raw mut value);
            let class = crate::molt_type_of(bits);
            assert_eq!(
                GLOBAL_BRIDGE.handle_to_borrowed_pyobj(class),
                (&raw mut *first).cast::<PyObject>(),
            );

            let descriptor = object_method_bits(py, "__init_subclass__").unwrap();
            inc_ref_bits(py, descriptor);
            let receiver = native_descriptor_receiver(
                py,
                obj_from_bits(descriptor).as_ptr().unwrap(),
                NativeCallableKind::ClassMethodDescriptor,
                crate::builtins::functions::native_callable::NativeDescriptorContext::Binding,
                None,
                Some(bits),
            )
            .ok()
            .flatten()
            .expect("native classmethod receiver");
            assert_eq!(receiver.bits(), class);
            drop(receiver);
            let bound = crate::builtins::attr::descriptor_bind(py, descriptor, None, Some(bits))
                .expect("materialized native classmethod");
            assert_eq!(
                bound_method_self_bits(obj_from_bits(bound).as_ptr().unwrap()),
                class
            );
            dec_ref_bits(py, class);
            assert_eq!(
                first.ob_base.ob_base.ob_refcnt, 2,
                "the bound receiver owns its native type wrapper"
            );

            // The equal-layout static classes need no heap-type custody edge.
            // A subsequent type query must observe a native class reassignment;
            // an already-bound classmethod retains its original receiver.
            value.ob_type = &raw mut *second;
            let current = crate::molt_type_of(bits);
            assert_eq!(
                GLOBAL_BRIDGE.handle_to_borrowed_pyobj(current),
                (&raw mut *second).cast::<PyObject>(),
            );
            assert_ne!(current, class);
            assert_eq!(
                bound_method_self_bits(obj_from_bits(bound).as_ptr().unwrap()),
                class
            );

            let repr_descriptor = object_method_bits(py, "__repr__").unwrap();
            inc_ref_bits(py, repr_descriptor);
            let repr_bound =
                crate::builtins::attr::descriptor_bind(py, repr_descriptor, None, Some(bits))
                    .expect("native method wrapper");
            let rendered = native_callable_repr(py, obj_from_bits(repr_bound).as_ptr().unwrap())
                .expect("native method repr");
            assert!(
                String::from_utf8(rendered)
                    .unwrap()
                    .contains("of NativeTypeSecond object at")
            );
            assert_eq!(IDENTITY_LOOKUPS.load(Ordering::SeqCst), 0);
            assert!(!exception_pending(py));
            assert!(errors::PyErr_Occurred().is_null());

            for owned in [
                repr_bound,
                repr_descriptor,
                current,
                bound,
                descriptor,
                bits,
            ] {
                dec_ref_bits(py, owned);
            }
            assert_eq!(value.ob_refcnt, 1);
            assert_eq!(first.ob_base.ob_base.ob_refcnt, 1);
            assert_eq!(second.ob_base.ob_base.ob_refcnt, 1);
        }
    });
}

#[test]
fn native_exception_public_methods_use_physical_receiver_admission() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(crate::cpython_abi_hooks::register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let native = native_exception(&raw mut PyExc_ValueError);
            let bits = foreign_bits(native);
            let note = MoltObject::from_ptr(alloc_string(py, b"native note")).bits();
            let method = exception_method_bits(py, "add_note").unwrap();
            inc_ref_bits(py, method);
            let function = obj_from_bits(method).as_ptr().unwrap();
            assert_eq!(
                crate::builtins::functions::native_callable::native_descriptor_receiver(
                    py,
                    function,
                    NativeCallableKind::MethodDescriptor,
                    crate::builtins::functions::native_callable::NativeDescriptorContext::Binding,
                    None,
                    Some(bits),
                )
                .map(|receiver| receiver.map(|receiver| receiver.bits())),
                Ok(Some(bits))
            );
            let result = crate::call_callable2(py, method, bits, note);
            assert!(obj_from_bits(result).is_none());
            assert!(!crate::exception_pending(py));
            let notes = object::PyObject_GetAttrString(native, c"__notes__".as_ptr());
            assert!(!notes.is_null());
            assert_eq!(sequences::PyList_Size(notes), 1);
            assert_eq!(
                sequences::PyList_GetItem(notes, 0),
                GLOBAL_BRIDGE.handle_to_borrowed_pyobj(note)
            );
            refcount::Py_DECREF(notes);

            let traceback_method = exception_method_bits(py, "with_traceback").unwrap();
            inc_ref_bits(py, traceback_method);
            let result =
                crate::call_callable2(py, traceback_method, bits, MoltObject::none().bits());
            assert_eq!(result, bits);
            dec_ref_bits(py, result);
            assert!(!crate::exception_pending(py));
            let result = crate::call_callable2(py, traceback_method, bits, note);
            assert!(obj_from_bits(result).is_none());
            let pending = crate::exception_last_bits_noinc(py).expect("invalid traceback raises");
            assert!(exception_matches_builtin_name(py, pending, "TypeError"));
            assert_eq!(
                format_exception_message(py, obj_from_bits(pending).as_ptr().unwrap()),
                "__traceback__ must be a traceback or None"
            );
            crate::clear_exception(py);
            assert!(errors::PyErr_Occurred().is_null());
            for value in [traceback_method, method, note, bits] {
                dec_ref_bits(py, value);
            }
            refcount::Py_DECREF(native);
        }
    });
}

#[test]
fn native_matching_preserves_both_exact_pending_error_channels() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(crate::cpython_abi_hooks::register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let mut class = native_subclass(&raw mut PyExc_ValueError, c"PendingMatchValue");
            let native = native_exception(&raw mut *class);
            let bits = foreign_bits(native);
            let class_ptr = (&raw mut *class).cast::<PyObject>();
            let class_bits = foreign_bits(class_ptr);
            let tuple = native_tuple(&[class_ptr]);
            let tuple_bits = foreign_bits(tuple);
            let c_error = native_exception(&raw mut PyExc_KeyError);
            refcount::Py_INCREF(c_error);
            refcount::Py_INCREF((&raw mut PyExc_KeyError).cast());
            errors::restore_current_error_exact(errors::OwnedCError {
                exc_type: (&raw mut PyExc_KeyError).cast(),
                value: c_error,
                traceback: ptr::null_mut(),
            });
            let runtime_error = managed_exception(py, "RuntimeError");
            crate::record_exception(py, obj_from_bits(runtime_error).as_ptr().unwrap());
            let native_refs = (*native).ob_refcnt;
            let class_refs = class.ob_base.ob_base.ob_refcnt;

            assert!(exception_is_class(py, class_bits));
            assert!(exception_class_is_subtype(
                py,
                class_bits,
                builtin_classes(py).exception
            ));
            assert!(exception_matches_type(
                py,
                bits,
                builtin_classes(py).exception
            ));
            assert_eq!(
                molt_exception_match_handler(bits, tuple_bits),
                MoltObject::from_bool(true).bits()
            );
            assert_eq!(errors::PyErr_GivenExceptionMatches(native, tuple), 1);
            assert_eq!(crate::exception_last_bits_noinc(py), Some(runtime_error));
            let error = errors::take_current_error().expect("C error survives matching");
            assert_eq!(error.exc_type, (&raw mut PyExc_KeyError).cast());
            assert_eq!(error.value, c_error);
            assert!(error.traceback.is_null());
            assert_eq!((*native).ob_refcnt, native_refs);
            assert_eq!(class.ob_base.ob_base.ob_refcnt, class_refs);
            drop(error);
            crate::clear_exception(py);
            for value in [runtime_error, tuple_bits, class_bits, bits] {
                dec_ref_bits(py, value);
            }
            for pointer in [c_error, tuple, native] {
                refcount::Py_DECREF(pointer);
            }
            assert!(errors::PyErr_Occurred().is_null());
        }
    });
}

/// Allocate real C storage through FromSpec/GenericNew, independently of the
/// runtime exception allocator whose admission this regression exercises.
unsafe fn init_storage_native_type(base: *mut PyTypeObject) -> refcount::OwnedPyObject {
    use molt_cpython_abi::api::typeobj;
    let mut slots = [
        PyType_Slot {
            slot: molt_cpython_abi::type_slots::Py_tp_new,
            pfunc: typeobj::PyType_GenericNew as *const () as *mut std::ffi::c_void,
        },
        PyType_Slot {
            slot: 0,
            pfunc: ptr::null_mut(),
        },
    ];
    let mut spec = PyType_Spec {
        name: c"storage.NativeException".as_ptr(),
        basicsize: unsafe { (*base).tp_basicsize } as i32,
        itemsize: 0,
        flags: PyType_Spec::flags_from_tp_flags(Py_TPFLAGS_DEFAULT | Py_TPFLAGS_BASETYPE),
        slots: slots.as_mut_ptr(),
    };
    let class = unsafe {
        refcount::OwnedPyObject::from_owned(typeobj::PyType_FromSpecWithBases(
            &raw mut spec,
            base.cast(),
        ))
    };
    assert!(
        !class.as_ptr().is_null(),
        "native exception type declaration"
    );
    class
}

fn init_storage_call(
    py: &PyToken<'_>,
    root: ExceptionLayoutRoot,
    receiver: u64,
    positional: &[u64],
    names: &[u64],
    values: &[u64],
) {
    let owner = exception_type_bits_from_name(py, root.owner_name());
    let callable = exception_method_bits_for_owner(py, owner, "__init__").unwrap();
    let mut arguments = vec![receiver];
    arguments.extend_from_slice(positional);
    let result = unsafe {
        crate::call::bind::call_bind_borrowed(py, callable, None, &arguments, names, values)
    };
    dec_ref_bits(py, result);
}

fn init_storage_arg(py: &PyToken<'_>, receiver: u64) -> u64 {
    let args = exception_field(py, receiver, ExceptionFieldSlot::Args).unwrap();
    // A real C constructor can own a native tuple. Observe its Python protocol,
    // rather than requiring the runtime's immutable physical tuple layout.
    assert_eq!(
        obj_from_bits(crate::molt_len(args.bits())).as_int(),
        Some(1)
    );
    let value =
        crate::object::ops::molt_getitem_builtin(args.bits(), MoltObject::from_int(0).bits());
    assert!(!exception_pending(py));
    value
}

#[test]
fn runtime_exception_initializers_dispatch_real_native_storage_without_wrapper_writes() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    assert!(crate::cpython_abi_hooks::register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let class = init_storage_native_type(&raw mut PyExc_Exception);
            let native = refcount::OwnedPyObject::from_owned(
                molt_cpython_abi::api::typeobj::PyType_GenericNew(
                    class.as_ptr().cast(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                ),
            );
            assert!(!native.as_ptr().is_null());
            let receiver = ExceptionValue::adopt(py, foreign_bits(native.as_ptr()));
            let wrapper = obj_from_bits(receiver.bits()).as_ptr().unwrap();
            let identity = crate::object::foreign::foreign_ptr_from_obj(wrapper);
            assert_eq!(identity, native.as_ptr().addr());
            let message = ExceptionValue::adopt(
                py,
                MoltObject::from_ptr(alloc_string(py, b"native message")).bits(),
            );
            init_storage_call(
                py,
                ExceptionLayoutRoot::Base,
                receiver.bits(),
                &[message.bits()],
                &[],
                &[],
            );
            assert!(!exception_pending(py));
            assert_eq!(
                crate::object::foreign::foreign_ptr_from_obj(wrapper),
                identity
            );
            assert_eq!(object_type_id(wrapper), crate::TYPE_ID_FOREIGN);
            let observed = ExceptionValue::adopt(py, init_storage_arg(py, receiver.bits()));
            assert_eq!(observed.bits(), message.bits());
            let raw_args = errors::PyException_GetArgs(native.as_ptr());
            assert_eq!(sequences::PyTuple_Size(raw_args), 1);
            assert_eq!(
                sequences::PyTuple_GetItem(raw_args, 0),
                GLOBAL_BRIDGE.handle_to_borrowed_pyobj(message.bits())
            );
            refcount::Py_DECREF(raw_args);
            for rendered in [
                crate::molt_str_from_obj(receiver.bits()),
                molt_exception_message(receiver.bits()),
            ] {
                assert!(!exception_pending(py));
                assert_eq!(
                    string_obj_to_owned(obj_from_bits(rendered)).as_deref(),
                    Some("native message")
                );
                dec_ref_bits(py, rendered);
            }
            let represented = crate::molt_repr_from_obj(receiver.bits());
            assert!(!exception_pending(py));
            assert!(
                string_obj_to_owned(obj_from_bits(represented))
                    .unwrap()
                    .contains("native message")
            );
            dec_ref_bits(py, represented);
            let args_name =
                ExceptionValue::adopt(py, attr_name_bits_from_bytes(py, b"args").unwrap());
            let public_args = ExceptionValue::adopt(
                py,
                crate::molt_get_attr_name(receiver.bits(), args_name.bits()),
            );
            assert!(!exception_pending(py));
            assert_eq!(
                public_args.bits(),
                exception_field(py, receiver.bits(), ExceptionFieldSlot::Args)
                    .unwrap()
                    .bits()
            );

            // Failed semantic admission cannot touch native fields or the wrapper.
            let before_args = (*native.as_ptr().cast::<PyBaseExceptionObject>()).args;
            let packed = MoltObject::from_ptr(alloc_tuple(py, &[message.bits()])).bits();
            let names = MoltObject::from_ptr(alloc_tuple(py, &[])).bits();
            let values = MoltObject::from_ptr(alloc_tuple(py, &[])).bits();
            molt_exception_init_owned(
                MoltObject::from_int(ExceptionLayoutRoot::SyntaxError as u8 as i64).bits(),
                receiver.bits(),
                packed,
                names,
                values,
            );
            assert!(exception_pending(py));
            assert!(exception_matches_builtin_name(
                py,
                exception_last_bits_noinc(py).unwrap(),
                "TypeError"
            ));
            clear_exception(py);
            assert_eq!(
                (*native.as_ptr().cast::<PyBaseExceptionObject>()).args,
                before_args
            );
            assert_eq!(
                crate::object::foreign::foreign_ptr_from_obj(wrapper),
                identity
            );

            // Consumed argument cleanup preserves an already active C error.
            let packed = MoltObject::from_ptr(alloc_tuple(py, &[])).bits();
            let names = MoltObject::from_ptr(alloc_tuple(py, &[])).bits();
            let values = MoltObject::from_ptr(alloc_tuple(py, &[])).bits();
            errors::PyErr_SetObject((&raw mut PyExc_Exception).cast(), native.as_ptr());
            molt_exception_init_owned(
                MoltObject::from_int(-1).bits(),
                receiver.bits(),
                packed,
                names,
                values,
            );
            let mut error_type = ptr::null_mut();
            let mut error_value = ptr::null_mut();
            let mut traceback = ptr::null_mut();
            errors::PyErr_Fetch(
                &raw mut error_type,
                &raw mut error_value,
                &raw mut traceback,
            );
            let error_type_owner = refcount::OwnedPyObject::from_owned(error_type);
            let error_value_owner = refcount::OwnedPyObject::from_owned(error_value);
            let traceback_owner = refcount::OwnedPyObject::from_owned(traceback);
            assert_eq!(error_value, native.as_ptr());
            drop(error_type_owner);
            drop(error_value_owner);
            drop(traceback_owner);
            assert!(!exception_pending(py));
            assert_eq!(
                (*native.as_ptr().cast::<PyBaseExceptionObject>()).args,
                before_args
            );
            assert_eq!(
                crate::object::foreign::foreign_ptr_from_obj(wrapper),
                identity
            );
        }
    });
}

#[test]
fn declaring_base_exception_init_preserves_typed_fields_across_storage() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    assert!(crate::cpython_abi_hooks::register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            for (native_class, name, root, field, public_name) in [
                (
                    &raw mut PyExc_StopIteration,
                    "StopIteration",
                    ExceptionLayoutRoot::StopIteration,
                    ExceptionTypedField::StopIterationValue,
                    b"value".as_slice(),
                ),
                (
                    &raw mut PyExc_SyntaxError,
                    "SyntaxError",
                    ExceptionLayoutRoot::SyntaxError,
                    ExceptionTypedField::SyntaxMessage,
                    b"msg".as_slice(),
                ),
                (
                    &raw mut PyExc_ImportError,
                    "ImportError",
                    ExceptionLayoutRoot::ImportError,
                    ExceptionTypedField::ImportMessage,
                    b"msg".as_slice(),
                ),
            ] {
                let native = refcount::OwnedPyObject::from_owned(native_exception(native_class));
                let foreign = ExceptionValue::adopt(py, foreign_bits(native.as_ptr()));
                let managed = ExceptionValue::adopt(py, managed_exception(py, name));
                for receiver in [managed.bits(), foreign.bits()] {
                    init_storage_call(
                        py,
                        root,
                        receiver,
                        &[MoltObject::from_int(7).bits()],
                        &[],
                        &[],
                    );
                    assert!(!exception_pending(py));
                    init_storage_call(
                        py,
                        ExceptionLayoutRoot::Base,
                        receiver,
                        &[MoltObject::from_int(9).bits()],
                        &[],
                        &[],
                    );
                    assert!(!exception_pending(py));
                    let storage = ExceptionStorage::for_exception(py, receiver).unwrap();
                    assert_eq!(
                        storage.typed_field(py, field).unwrap().bits(),
                        MoltObject::from_int(7).bits()
                    );
                    let observed = ExceptionValue::adopt(py, init_storage_arg(py, receiver));
                    assert_eq!(observed.bits(), MoltObject::from_int(9).bits());
                    let value_name = ExceptionValue::adopt(
                        py,
                        attr_name_bits_from_bytes(py, public_name).unwrap(),
                    );
                    let public_value = ExceptionValue::adopt(
                        py,
                        crate::molt_get_attr_name(receiver, value_name.bits()),
                    );
                    assert!(!exception_pending(py));
                    assert_eq!(public_value.bits(), MoltObject::from_int(7).bits());
                    crate::molt_set_attr_name(
                        receiver,
                        value_name.bits(),
                        MoltObject::from_int(13).bits(),
                    );
                    assert!(!exception_pending(py));
                    assert_eq!(
                        storage.typed_field(py, field).unwrap().bits(),
                        MoltObject::from_int(13).bits()
                    );
                    let arguments =
                        MoltObject::from_ptr(alloc_tuple(py, &[MoltObject::from_int(11).bits()]))
                            .bits();
                    molt_exception_init(receiver, arguments);
                    assert!(!exception_pending(py));
                    assert_eq!(
                        storage.typed_field(py, field).unwrap().bits(),
                        MoltObject::from_int(11).bits()
                    );
                }
                assert_eq!(
                    crate::object::foreign::foreign_ptr_from_obj(
                        obj_from_bits(foreign.bits()).as_ptr().unwrap()
                    ),
                    native.as_ptr().addr()
                );
            }

            // Keyword constructors use the existing native typed initializer.
            let attribute = refcount::OwnedPyObject::from_owned(native_exception(
                &raw mut PyExc_AttributeError,
            ));
            let attribute_bits = ExceptionValue::adopt(py, foreign_bits(attribute.as_ptr()));
            let name = ExceptionValue::adopt(py, attr_name_bits_from_bytes(py, b"name").unwrap());
            let field =
                ExceptionValue::adopt(py, attr_name_bits_from_bytes(py, b"missing_field").unwrap());
            init_storage_call(
                py,
                ExceptionLayoutRoot::AttributeError,
                attribute_bits.bits(),
                &[field.bits()],
                &[name.bits()],
                &[field.bits()],
            );
            assert!(!exception_pending(py));
            assert_eq!(
                ExceptionStorage::for_exception(py, attribute_bits.bits())
                    .unwrap()
                    .typed_field(py, ExceptionTypedField::AttributeErrorName)
                    .unwrap()
                    .bits(),
                field.bits()
            );
        }
    });
}

#[test]
fn native_child_of_managed_exception_initializes_real_physical_payload() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    assert!(crate::cpython_abi_hooks::register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let name = ExceptionValue::adopt(
                py,
                attr_name_bits_from_bytes(py, b"ManagedAppError").unwrap(),
            );
            let parent = ExceptionValue::adopt(py, crate::molt_class_new(name.bits()));
            crate::molt_class_set_base(
                parent.bits(),
                exception_type_bits_from_name(py, "Exception"),
            );
            crate::object::class_finish_definition(
                py,
                obj_from_bits(parent.bits()).as_ptr().unwrap(),
            )
            .expect("seal exception subclass");
            let view = refcount::OwnedPyObject::from_owned(
                GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(parent.bits()),
            );
            assert!(!view.as_ptr().is_null());
            let class = init_storage_native_type(view.as_ptr().cast());
            let message = ExceptionValue::adopt(
                py,
                attr_name_bits_from_bytes(py, b"constructed native").unwrap(),
            );
            let c_message = refcount::OwnedPyObject::from_owned(
                GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(message.bits()),
            );
            let native = refcount::OwnedPyObject::from_owned(object::PyObject_CallOneArg(
                class.as_ptr(),
                c_message.as_ptr(),
            ));
            assert!(
                !native.as_ptr().is_null(),
                "native constructor inherited from managed Exception subtype"
            );
            let receiver = ExceptionValue::adopt(py, foreign_bits(native.as_ptr()));
            assert!(matches!(
                ExceptionStorage::for_exception(py, receiver.bits()),
                Some(ExceptionStorage::Native(_))
            ));
            assert_eq!(
                crate::object::foreign::foreign_ptr_from_obj(
                    obj_from_bits(receiver.bits()).as_ptr().unwrap()
                ),
                native.as_ptr().addr()
            );
            let observed = ExceptionValue::adopt(py, init_storage_arg(py, receiver.bits()));
            assert_eq!(observed.bits(), message.bits());
            assert!(!exception_pending(py));
        }
    });
}
