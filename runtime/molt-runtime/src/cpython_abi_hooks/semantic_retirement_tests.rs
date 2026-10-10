//! Ordinary semantic retirement must expose, then complete, native error reentry.
use super::native_test_fixture::NativeType;
use super::*;
use crate::{PyToken, exception_pending, obj_from_bits};
use molt_cpython_abi::api::{errors, refcount};
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
use molt_runtime_core::{OwnedRuntimeValue, ProtectedRuntimeValue};

thread_local! {
    static OBSERVED: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
    static RETURN_VALUE: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

// The callback transfers its one result from this slot. Unwind before the
// callback still retires it, so a failed fixture cannot leave a hidden owner.
struct StoredResult<'a>(std::marker::PhantomData<&'a molt_runtime_core::PyToken>);
impl<'a> StoredResult<'a> {
    fn new(value: ProtectedRuntimeValue<'a>) -> Self {
        RETURN_VALUE.with(|slot| {
            assert_eq!(slot.get(), 0);
            slot.set(value.into_bits());
        });
        Self(std::marker::PhantomData)
    }
}
impl Drop for StoredResult<'_> {
    fn drop(&mut self) {
        let bits = RETURN_VALUE.with(|slot| slot.replace(0));
        unsafe { molt_runtime_core::ffi::__molt_runtime_release_protected_value(bits) };
    }
}

unsafe extern "C" fn observing_drop(object: *mut PyObject) {
    // This is a legal deallocator: fetch and restore exactly the active error.
    // The observation has no ownership and is checked while the error is live.
    let error = unsafe { errors::PyErr_GetRaisedException() };
    OBSERVED.with(|seen| seen.set((seen.get().0 + 1, error as usize)));
    unsafe { errors::PyErr_SetRaisedException(error) };
    drop(unsafe { Box::from_raw(object) });
}

unsafe extern "C" fn native_iter(object: *mut PyObject) -> *mut PyObject {
    unsafe { refcount::Py_INCREF(object) };
    object
}
unsafe extern "C" fn native_next(_object: *mut PyObject) -> *mut PyObject {
    std::ptr::null_mut()
}
unsafe extern "C" fn return_native() -> u64 {
    RETURN_VALUE.with(|value| value.replace(0))
}

unsafe fn native_value<'a>(
    py: &'a PyToken<'_>,
    kind: &mut PyTypeObject,
) -> ProtectedRuntimeValue<'a> {
    let raw = Box::into_raw(Box::new(PyObject {
        ob_refcnt: 1,
        ob_type: kind,
    }));
    let native = unsafe { refcount::ProtectedPyObject::from_owned(raw) };
    let bits =
        unsafe { GLOBAL_BRIDGE.molt_value_for_pyobj(native.as_ptr()) }.expect("foreign owner");
    let owned = unsafe { ProtectedRuntimeValue::from_owned_bits(py.core_token(), bits) };
    drop(native);
    owned
}

fn pending_error<'a>(py: &'a PyToken<'_>) -> ProtectedRuntimeValue<'a> {
    let ptr = crate::builtins::exceptions::alloc_exception(py, "ValueError", "");
    assert!(!ptr.is_null());
    unsafe {
        ProtectedRuntimeValue::from_owned_bits(py.core_token(), MoltObject::from_ptr(ptr).bits())
    }
}

fn assert_completed(py: &PyToken<'_>, expected: Option<u64>) {
    assert!(
        exception_pending(py),
        "native restore must return to the runtime channel"
    );
    if let Some(bits) = expected {
        assert_eq!(crate::exception_last_bits_noinc(py), Some(bits));
    }
    let error = unsafe { errors::PyErr_GetRaisedException() };
    let error = unsafe { refcount::ProtectedPyObject::from_owned(error) };
    assert!(!error.as_ptr().is_null());
    assert_eq!(
        OBSERVED.with(|seen| seen.get()),
        (1, error.as_ptr() as usize)
    );
    assert!(!exception_pending(py));
}

#[test]
fn ordinary_runtime_and_http_owners_expose_and_complete_native_error() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let mut kind = NativeType::subtype(
                &raw mut molt_cpython_abi::abi_types::PyBaseObject_Type,
                c"OrdinaryRetirement",
            );
            kind.tp_dealloc = Some(observing_drop);
            kind.tp_iter = Some(native_iter);
            kind.tp_iternext = Some(native_next);
            assert_eq!(kind.ready(), 0);
            for mode in 0..3 {
                let value = native_value(py, &mut kind);
                let iterator = (mode == 2).then(|| {
                    crate::object::iterable::OwnedIterator::new(py, value.bits()).unwrap()
                });
                let error = pending_error(py);
                OBSERVED.with(|seen| seen.set((0, 0)));
                let _stored = if mode == 2 {
                    drop(value);
                    None
                } else {
                    Some(StoredResult::new(value))
                };
                crate::record_exception(py, obj_from_bits(error.bits()).as_ptr().unwrap());
                match mode {
                    0 => drop(OwnedRuntimeValue::from_owned_bits(
                        py.core_token(),
                        RETURN_VALUE.with(|slot| slot.replace(0)),
                    )),
                    1 => crate::http_bridge::__molt_http_dec_ref_bits(
                        RETURN_VALUE.with(|slot| slot.replace(0)),
                    ),
                    _ => drop(iterator),
                }
                assert_completed(py, Some(error.bits()));
            }
        }
    });
}

#[test]
fn semantic_sequence_and_argument_snapshots_expose_last_native_owner() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let mut kind = NativeType::subtype(
                &raw mut molt_cpython_abi::abi_types::PyBaseObject_Type,
                c"SnapshotRetirement",
            );
            kind.tp_dealloc = Some(observing_drop);
            assert_eq!(kind.ready(), 0);
            for mode in 0..4 {
                let value = native_value(py, &mut kind);
                let value_bits = value.bits();
                let tuple = crate::alloc_tuple(
                    py,
                    if mode == 3 {
                        &[]
                    } else {
                        std::slice::from_ref(&value_bits)
                    },
                );
                assert!(!tuple.is_null());
                let tuple_owner = ProtectedRuntimeValue::from_owned_bits(
                    py.core_token(),
                    MoltObject::from_ptr(tuple).bits(),
                );
                let pin =
                    (mode == 0).then(|| crate::object::seq_access::pin_tuple(py, tuple).unwrap());
                let item =
                    (mode == 1).then(|| crate::object::seq_access::pin_item(py, tuple, 0).unwrap());
                let snapshot = (mode == 2).then(|| {
                    crate::object::seq_access::snapshot(py, tuple, "snapshot test").unwrap()
                });
                let name = crate::attr_name_bits_from_bytes(py, b"value").unwrap();
                let name = ProtectedRuntimeValue::from_owned_bits(py.core_token(), name);
                let dict = crate::alloc_dict_with_pairs(py, &[name.bits(), value.bits()]);
                assert!(!dict.is_null());
                let dict_owner = ProtectedRuntimeValue::from_owned_bits(
                    py.core_token(),
                    MoltObject::from_ptr(dict).bits(),
                );
                let args = (mode == 3).then(|| {
                    crate::builtins::native_arguments::NativeArguments::read(
                        py,
                        "snapshot",
                        tuple_owner.bits(),
                        dict_owner.bits(),
                    )
                    .unwrap()
                });
                crate::object::ops::dict_clear_in_place(py, dict);
                drop(dict_owner);
                drop(tuple_owner);
                drop(value);
                OBSERVED.with(|seen| seen.set((0, 0)));
                let error = pending_error(py);
                crate::record_exception(py, obj_from_bits(error.bits()).as_ptr().unwrap());
                drop(pin);
                drop(item);
                drop(snapshot);
                drop(args);
                assert_completed(py, Some(error.bits()));
            }
        }
    });
}

#[test]
fn invalid_format_and_mapping_results_expose_their_type_error_to_native_retirement() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let mut kind = NativeType::subtype(
                &raw mut molt_cpython_abi::abi_types::PyBaseObject_Type,
                c"InvalidSemanticResult",
            );
            kind.tp_dealloc = Some(observing_drop);
            assert_eq!(kind.ready(), 0);
            for mapping in [false, true] {
                let value = native_value(py, &mut kind);
                OBSERVED.with(|seen| seen.set((0, 0)));
                if mapping {
                    assert!(
                        crate::object::mapping_merge::output_as_list(
                            py,
                            MoltObject::none().bits(),
                            b"keys",
                            value.into_bits()
                        )
                        .is_none()
                    );
                } else {
                    let method = crate::alloc_runtime_function_obj(
                        py,
                        crate::builtins::functions::runtime_fn_addr(
                            "retirement_return_native",
                            return_native as *const (),
                        ),
                        0,
                    );
                    assert!(!method.is_null());
                    let method = ProtectedRuntimeValue::from_owned_bits(
                        py.core_token(),
                        MoltObject::from_ptr(method).bits(),
                    );
                    let _stored = StoredResult::new(value);
                    let result = crate::object::ops_format::invoke_format_method(
                        py,
                        method.into_bits(),
                        "__str__",
                        None,
                    );
                    assert_eq!(result, MoltObject::none().bits());
                }
                let bits = crate::exception_last_bits_noinc(py).expect("runtime TypeError");
                assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                    py,
                    bits,
                    "TypeError"
                ));
                assert_completed(py, Some(bits));
            }
        }
    });
}

#[test]
fn context_exit_exposes_error_to_the_last_native_value_owner() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            use crate::builtins::contextvars as context;
            let mut kind = NativeType::subtype(
                &raw mut molt_cpython_abi::abi_types::PyBaseObject_Type,
                c"ContextRetirement",
            );
            kind.tp_dealloc = Some(observing_drop);
            assert_eq!(kind.ready(), 0);
            let value = native_value(py, &mut kind);
            let error = pending_error(py);
            let name = crate::attr_name_bits_from_bytes(py, b"retirement-value").unwrap();
            let name = ProtectedRuntimeValue::from_owned_bits(py.core_token(), name);
            let variable = context::new_variable(py, name.bits(), None).unwrap();
            let variable = ProtectedRuntimeValue::from_owned_bits(py.core_token(), variable);
            let ctx = context::new_context(py).unwrap();
            let ctx = ProtectedRuntimeValue::from_owned_bits(py.core_token(), ctx);
            let entered = context::EnteredContext::enter(py, ctx.bits()).unwrap();
            let token = context::set_variable(py, variable.bits(), value.bits()).unwrap();
            drop(ProtectedRuntimeValue::from_owned_bits(
                py.core_token(),
                token,
            ));
            drop(ctx);
            drop(value);
            OBSERVED.with(|seen| seen.set((0, 0)));
            crate::record_exception(py, obj_from_bits(error.bits()).as_ptr().unwrap());
            drop(entered);
            assert_completed(py, Some(error.bits()));
        }
    });
}

#[test]
fn renderer_string_subclass_owners_expose_error_to_native_fields() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            use crate::object::ops_format::{FormatError, FormatOutput, FormatWriter};
            let mut kind = NativeType::subtype(
                &raw mut molt_cpython_abi::abi_types::PyBaseObject_Type,
                c"RendererFieldRetirement",
            );
            kind.tp_dealloc = Some(observing_drop);
            assert_eq!(kind.ready(), 0);
            let name = crate::attr_name_bits_from_bytes(py, b"RetirementString").unwrap();
            let name = ProtectedRuntimeValue::from_owned_bits(py.core_token(), name);
            let class = ProtectedRuntimeValue::from_owned_bits(
                py.core_token(),
                crate::molt_class_new(name.bits()),
            );
            dec_ref_bits(
                py,
                crate::molt_class_set_base(class.bits(), crate::builtin_classes(py).str),
            );
            crate::object::class_finish_definition(
                py,
                obj_from_bits(class.bits()).as_ptr().unwrap(),
            )
            .unwrap();
            let text = crate::alloc_string(py, b"value");
            assert!(!text.is_null());
            let text = ProtectedRuntimeValue::from_owned_bits(
                py.core_token(),
                MoltObject::from_ptr(text).bits(),
            );
            let field = crate::attr_name_bits_from_bytes(py, b"native_owner").unwrap();
            let field = ProtectedRuntimeValue::from_owned_bits(py.core_token(), field);
            for mode in 0..3 {
                let value = native_value(py, &mut kind);
                let instance = crate::call::bind::call_bind_borrowed(
                    py,
                    class.bits(),
                    None,
                    &[text.bits()],
                    &[],
                    &[],
                );
                let instance = ProtectedRuntimeValue::from_owned_bits(py.core_token(), instance);
                assert!(!exception_pending(py));
                assert_eq!(crate::type_of_bits(py, instance.bits()), class.bits());
                dec_ref_bits(
                    py,
                    crate::molt_set_attr_name(instance.bits(), field.bits(), value.bits()),
                );
                assert!(!exception_pending(py));
                drop(value);
                let error = pending_error(py);
                OBSERVED.with(|seen| seen.set((0, 0)));
                if mode == 2 {
                    let mut writer = FormatWriter::new(py);
                    writer
                        .append_output(FormatOutput::OwnedString(instance.into_bits()), true)
                        .unwrap();
                    crate::record_exception(py, obj_from_bits(error.bits()).as_ptr().unwrap());
                    drop(writer);
                } else {
                    crate::record_exception(py, obj_from_bits(error.bits()).as_ptr().unwrap());
                    let output = FormatOutput::OwnedString(instance.into_bits());
                    if mode == 0 {
                        assert_eq!(output.into_bits(py), MoltObject::none().bits());
                    } else {
                        assert!(matches!(
                            output.with_bytes(py, |_| Ok(())),
                            Err(FormatError::Pending)
                        ));
                    }
                }
                assert_completed(py, Some(error.bits()));
            }
        }
    });
}
