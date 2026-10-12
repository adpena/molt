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
            for mode in [
                0,
                #[cfg(feature = "stdlib_http")]
                1,
                2,
            ] {
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
                    #[cfg(feature = "stdlib_http")]
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

// These observations are independent of the implementation's branch structure:
// pinned typeobject.c/abstract.c specify which error is visible at each last
// native reference. No fixture reference keeps a returned callable/value alive.
#[derive(Clone, Copy)]
struct ProtocolObservation {
    callable: *mut PyTypeObject,
    result: *mut PyTypeObject,
    integer: *mut PyTypeObject,
    index_source: *mut PyTypeObject,
    index_result: *mut PyTypeObject,
    failure: *mut PyObject,
    mode: u8,
    index_negative: bool,
    index_fails: bool,
    index_huge: bool,
    index_entry: (usize, usize),
    index_returned: usize,
    events: [(u8, usize); 8],
    tracebacks: [usize; 8],
    event_count: usize,
    bindings: usize,
    calls: usize,
    indices: usize,
    descriptors: usize,
}
impl ProtocolObservation {
    const EMPTY: Self = Self {
        callable: ptr::null_mut(),
        result: ptr::null_mut(),
        integer: ptr::null_mut(),
        index_source: ptr::null_mut(),
        index_result: ptr::null_mut(),
        failure: ptr::null_mut(),
        mode: 0,
        index_negative: false,
        index_fails: false,
        index_huge: true,
        index_entry: (0, 0),
        index_returned: 0,
        events: [(0, 0); 8],
        tracebacks: [0; 8],
        event_count: 0,
        bindings: 0,
        calls: 0,
        indices: 0,
        descriptors: 0,
    };
}
thread_local! {
    static PROTOCOL: std::cell::Cell<ProtocolObservation> = const {
        std::cell::Cell::new(ProtocolObservation::EMPTY)
    };
}
fn protocol_update(update: impl FnOnce(&mut ProtocolObservation)) {
    PROTOCOL.with(|slot| {
        let mut state = slot.get();
        update(&mut state);
        slot.set(state);
    });
}
struct ProtocolReset;
impl Drop for ProtocolReset {
    fn drop(&mut self) {
        PROTOCOL.with(|slot| slot.set(ProtocolObservation::EMPTY));
        crate::molt_warnings_resetwarnings();
    }
}
unsafe fn protocol_drop(value: *mut PyObject, kind: u8) {
    let error = unsafe { errors::PyErr_GetRaisedException() };
    let traceback = if error.is_null() {
        ptr::null_mut()
    } else {
        unsafe { errors::PyException_GetTraceback(error) }
    };
    protocol_update(|state| {
        if state.event_count < state.events.len() {
            state.events[state.event_count] = (kind, error.addr());
            state.tracebacks[state.event_count] = traceback.addr();
        }
        state.event_count += 1;
    });
    unsafe {
        refcount::Py_XDECREF(traceback);
        errors::PyErr_SetRaisedException(error);
        molt_cpython_abi::api::memory::PyObject_Free(value.cast());
    }
}
unsafe extern "C" fn protocol_callable_drop(value: *mut PyObject) {
    unsafe { protocol_drop(value, 1) };
}
unsafe extern "C" fn protocol_result_drop(value: *mut PyObject) {
    unsafe { protocol_drop(value, 2) };
}
unsafe extern "C" fn protocol_index_drop(value: *mut PyObject) {
    unsafe { protocol_drop(value, 3) };
}
unsafe extern "C" fn protocol_descriptor_drop(value: *mut PyObject) {
    protocol_update(|state| state.descriptors += 1);
    unsafe { molt_cpython_abi::api::memory::PyObject_Free(value.cast()) };
}
unsafe fn protocol_long(class: *mut PyTypeObject, huge: bool, negative: bool) -> *mut PyObject {
    use molt_cpython_abi::{abi_types::PyLongObject, api::typeobj};
    let count = if huge { 6 } else { 1 };
    let value = unsafe { typeobj::PyType_GenericAlloc(class, count) };
    if !value.is_null() {
        let integer = value.cast::<PyLongObject>();
        unsafe {
            (*integer).long_value.lv_tag = ((count as usize) << 3) | (usize::from(negative) * 2);
            let digits = (&raw mut (*integer).long_value.ob_digit).cast::<u32>();
            for index in 0..count as usize {
                digits
                    .add(index)
                    .write(u32::from(index + 1 == count as usize));
            }
        }
    }
    value
}
unsafe extern "C" fn protocol_index(_value: *mut PyObject) -> *mut PyObject {
    use molt_cpython_abi::api::object;
    let incoming = unsafe { errors::PyErr_GetRaisedException() };
    let traceback = if incoming.is_null() {
        ptr::null_mut()
    } else {
        unsafe { errors::PyException_GetTraceback(incoming) }
    };
    protocol_update(|state| {
        state.indices += 1;
        state.index_entry = (incoming.addr(), traceback.addr());
    });
    unsafe {
        refcount::Py_XDECREF(traceback);
        errors::PyErr_SetRaisedException(incoming);
    }
    let state = PROTOCOL.with(|slot| slot.get());
    if state.index_fails {
        unsafe { errors::PyErr_SetRaisedException(object::Py_NewRef(state.failure)) };
        ptr::null_mut()
    } else {
        let result =
            unsafe { protocol_long(state.index_result, state.index_huge, state.index_negative) };
        protocol_update(|state| state.index_returned = result.addr());
        result
    }
}
unsafe extern "C" fn protocol_call(
    _callable: *mut PyObject,
    _args: *mut PyObject,
    _kwargs: *mut PyObject,
) -> *mut PyObject {
    use molt_cpython_abi::{
        abi_types::{Py_NotImplementedSentinel, Py_True},
        api::{object, typeobj},
    };
    protocol_update(|state| state.calls += 1);
    let state = PROTOCOL.with(|slot| slot.get());
    unsafe {
        match state.mode {
            0 => typeobj::PyType_GenericAlloc(state.result, 0),
            1 => protocol_long(state.integer, false, true),
            2 => protocol_long(state.integer, true, false),
            3 => protocol_long(state.integer, true, true),
            4 => typeobj::PyType_GenericAlloc(state.index_source, 0),
            5 => object::Py_NewRef((&raw mut Py_True).cast()),
            7 => object::Py_NewRef(&raw mut Py_NotImplementedSentinel),
            _ => {
                errors::PyErr_SetRaisedException(object::Py_NewRef(state.failure));
                ptr::null_mut()
            }
        }
    }
}
unsafe extern "C" fn protocol_bind(
    _descriptor: *mut PyObject,
    _instance: *mut PyObject,
    _class: *mut PyObject,
) -> *mut PyObject {
    protocol_update(|state| state.bindings += 1);
    unsafe {
        molt_cpython_abi::api::typeobj::PyType_GenericAlloc(
            PROTOCOL.with(|slot| slot.get().callable),
            0,
        )
    }
}

// Clear the managed class and its projected namespace while every local native
// declaration still exists. Native static instances do not retain ob_type.
struct ProtocolClass<'a, 'py> {
    view: refcount::ProtectedPyObject,
    class: ProtectedRuntimeValue<'a>,
    py: &'a PyToken<'py>,
}
impl Drop for ProtocolClass<'_, '_> {
    fn drop(&mut self) {
        errors::with_preserved_error(|| unsafe {
            if let Some(class) = obj_from_bits(self.class.bits()).as_ptr() {
                // The public GC clear and C type clear share this owner. It also
                // handles setup failure before a C type projection exists.
                crate::object::heap_lifecycle::try_clear_cycle_edges(self.py, class);
            }
        });
    }
}
unsafe fn protocol_receiver<'a, 'py>(
    py: &'a PyToken<'py>,
    name: &[u8],
    descriptor: u64,
) -> (ProtocolClass<'a, 'py>, ProtectedRuntimeValue<'a>) {
    let class_name = crate::attr_name_bits_from_bytes(py, b"ManagedTruthLength").unwrap();
    let class_name = unsafe { ProtectedRuntimeValue::from_owned_bits(py.core_token(), class_name) };
    let class = unsafe {
        ProtectedRuntimeValue::from_owned_bits(
            py.core_token(),
            crate::molt_class_new(class_name.bits()),
        )
    };
    // Arm before publishing the first native edge, including every unwind path.
    let mut class = ProtocolClass {
        view: unsafe { refcount::ProtectedPyObject::from_owned(ptr::null_mut()) },
        class,
        py,
    };
    assert!(!obj_from_bits(class.class.bits()).is_none());
    dec_ref_bits(
        py,
        crate::molt_class_set_base(class.class.bits(), crate::builtin_classes(py).object),
    );
    let name = unsafe {
        ProtectedRuntimeValue::from_owned_bits(
            py.core_token(),
            crate::attr_name_bits_from_bytes(py, name).unwrap(),
        )
    };
    dec_ref_bits(
        py,
        crate::molt_set_attr_name(class.class.bits(), name.bits(), descriptor),
    );
    assert!(!exception_pending(py));
    unsafe {
        crate::object::class_finish_definition(
            py,
            obj_from_bits(class.class.bits()).as_ptr().unwrap(),
        )
    }
    .unwrap();
    class.view = unsafe {
        refcount::ProtectedPyObject::from_owned(
            GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(class.class.bits()),
        )
    };
    assert!(!class.view.as_ptr().is_null());
    let instance = unsafe {
        crate::alloc_instance_for_class(py, obj_from_bits(class.class.bits()).as_ptr().unwrap())
    };
    let instance = unsafe { ProtectedRuntimeValue::from_owned_bits(py.core_token(), instance) };
    assert!(!exception_pending(py));
    assert!(!obj_from_bits(instance.bits()).is_none());
    (class, instance)
}
unsafe fn protocol_value<'py>(
    py: &'py PyToken<'_>,
    class: *mut PyTypeObject,
) -> ProtectedRuntimeValue<'py> {
    let native = unsafe {
        refcount::ProtectedPyObject::from_owned(
            molt_cpython_abi::api::typeobj::PyType_GenericAlloc(class, 0),
        )
    };
    assert!(!native.as_ptr().is_null());
    let bits = unsafe { GLOBAL_BRIDGE.molt_value_for_pyobj(native.as_ptr()) }
        .expect("native protocol fixture owner");
    unsafe { ProtectedRuntimeValue::from_owned_bits(py.core_token(), bits) }
}
struct ProtocolNativeClass(refcount::ProtectedPyObject);
impl Drop for ProtocolNativeClass {
    fn drop(&mut self) {
        errors::with_preserved_error(|| unsafe {
            molt_cpython_abi::api::typeobj::molt_type_clear(self.0.as_ptr());
        });
    }
}
unsafe fn protocol_native_class() -> ProtocolNativeClass {
    use molt_cpython_abi::{abi_types::*, api::typeobj, type_slots};
    let mut slots = [
        PyType_Slot {
            slot: type_slots::Py_tp_new,
            pfunc: typeobj::PyType_GenericNew as *const () as *mut std::ffi::c_void,
        },
        PyType_Slot {
            slot: 0,
            pfunc: ptr::null_mut(),
        },
    ];
    let mut spec = PyType_Spec {
        name: c"retirement.NativeBool".as_ptr(),
        basicsize: std::mem::size_of::<PyObject>() as i32,
        itemsize: 0,
        flags: PyType_Spec::flags_from_tp_flags(Py_TPFLAGS_DEFAULT | Py_TPFLAGS_BASETYPE),
        slots: slots.as_mut_ptr(),
    };
    let class = ProtocolNativeClass(unsafe {
        refcount::ProtectedPyObject::from_owned(typeobj::PyType_FromSpec(&raw mut spec))
    });
    assert!(!class.0.as_ptr().is_null());
    class
}
fn protocol_start(mode: u8) {
    protocol_update(|state| {
        state.mode = mode;
        state.index_negative = false;
        state.index_fails = false;
        state.index_huge = true;
        state.index_entry = (0, 0);
        state.index_returned = 0;
        state.tracebacks = [0; 8];
        state.events = [(0, 0); 8];
        state.event_count = 0;
        state.bindings = 0;
        state.calls = 0;
        state.indices = 0;
    });
}
fn protocol_warning_policy(py: &PyToken<'_>, action: &[u8]) {
    let action = unsafe {
        ProtectedRuntimeValue::from_owned_bits(
            py.core_token(),
            crate::attr_name_bits_from_bytes(py, action).unwrap(),
        )
    };
    dec_ref_bits(
        py,
        crate::molt_warnings_simplefilter(
            action.bits(),
            crate::builtins::exceptions::exception_type_bits_from_name(py, "DeprecationWarning"),
            MoltObject::from_int(0).bits(),
            MoltObject::from_bool(false).bits(),
        ),
    );
    assert!(!exception_pending(py));
}
unsafe fn protocol_error(kind: *mut PyTypeObject, message: &str) -> refcount::ProtectedPyObject {
    use molt_cpython_abi::api::{strings, typeobj};
    assert_eq!(unsafe { errors::PyErr_ExceptionMatches(kind.cast()) }, 1);
    let error =
        unsafe { refcount::ProtectedPyObject::from_owned(errors::PyErr_GetRaisedException()) };
    assert!(!error.as_ptr().is_null());
    let text =
        unsafe { refcount::ProtectedPyObject::from_owned(typeobj::PyObject_Str(error.as_ptr())) };
    assert!(!text.as_ptr().is_null());
    let mut size = 0;
    let bytes = unsafe { strings::PyUnicode_AsUTF8AndSize(text.as_ptr(), &raw mut size) };
    assert!(!bytes.is_null());
    assert_eq!(
        unsafe { std::slice::from_raw_parts(bytes.cast::<u8>(), size as usize) },
        message.as_bytes()
    );
    error
}
fn protocol_events(expected: &[(u8, usize)]) {
    let state = PROTOCOL.with(|slot| slot.get());
    assert_eq!(state.event_count, expected.len());
    assert_eq!(&state.events[..expected.len()], expected);
}

#[test]
fn managed_truth_length_and_hint_retire_sole_native_owners_at_cpython_boundaries() {
    use molt_cpython_abi::{
        abi_types::*,
        api::{object, typeobj},
    };
    let transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let mut callable =
                NativeType::subtype(&raw mut PyBaseObject_Type, c"RetirementCallable");
            callable.tp_call = Some(protocol_call);
            callable.tp_dealloc = Some(protocol_callable_drop);
            assert_eq!(callable.ready(), 0);
            let mut descriptor =
                NativeType::subtype(&raw mut PyBaseObject_Type, c"RetirementDescriptor");
            descriptor.tp_descr_get = Some(protocol_bind);
            descriptor.tp_dealloc = Some(protocol_descriptor_drop);
            assert_eq!(descriptor.ready(), 0);
            let mut result = NativeType::subtype(&raw mut PyBaseObject_Type, c"RetirementResult");
            result.tp_dealloc = Some(protocol_result_drop);
            assert_eq!(result.ready(), 0);
            let mut integer = NativeType::subtype(&raw mut PyLong_Type, c"RetirementInteger");
            integer.tp_dealloc = Some(protocol_result_drop);
            assert_eq!(integer.ready(), 0);
            let mut index_slots: PyNumberMethods = std::mem::zeroed();
            index_slots.nb_index = protocol_index as *const () as *mut _;
            let mut indexed = NativeType::subtype(&raw mut PyBaseObject_Type, c"RetirementIndexed");
            indexed.tp_as_number = (&raw mut index_slots).cast();
            indexed.tp_dealloc = Some(protocol_result_drop);
            assert_eq!(indexed.ready(), 0);
            let mut index_result =
                NativeType::subtype(&raw mut PyLong_Type, c"RetirementIndexResult");
            index_result.tp_dealloc = Some(protocol_index_drop);
            assert_eq!(index_result.ready(), 0);
            let failure_bits = crate::builtins::exceptions::alloc_exception(
                py,
                "LookupError",
                "protocol callback",
            );
            assert!(!failure_bits.is_null());
            let failure_owner = ProtectedRuntimeValue::from_owned_bits(
                py.core_token(),
                MoltObject::from_ptr(failure_bits).bits(),
            );
            let failure = refcount::ProtectedPyObject::from_owned(
                GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(failure_owner.bits()),
            );
            assert!(!failure.as_ptr().is_null());
            let traceback_bits = crate::alloc_instance_for_class(
                py,
                obj_from_bits(crate::builtin_classes(py).traceback)
                    .as_ptr()
                    .unwrap(),
            );
            let traceback = refcount::ProtectedPyObject::from_owned(
                GLOBAL_BRIDGE.owned_handle_to_pyobj(traceback_bits),
            );
            assert!(!traceback.as_ptr().is_null());
            assert_eq!(
                errors::PyException_SetTraceback(failure.as_ptr(), traceback.as_ptr()),
                0
            );
            let _reset = ProtocolReset;
            protocol_update(|state| {
                state.callable = &raw mut *callable;
                state.result = &raw mut *result;
                state.integer = &raw mut *integer;
                state.index_source = &raw mut *indexed;
                state.index_result = &raw mut *index_result;
                state.failure = failure.as_ptr();
            });
            protocol_warning_policy(py, b"ignore");
            for minor in [12, 13, 14] {
                transaction.with_target_python_minor(py, minor, || {
                    for (method, route) in [
                        (b"__bool__".as_slice(), 0),
                        (b"__len__", 1),
                        (b"__length_hint__", 2),
                    ] {
                        let declaration = refcount::ProtectedPyObject::from_owned(
                            typeobj::PyType_GenericAlloc(&raw mut *descriptor, 0),
                        );
                        assert!(!declaration.as_ptr().is_null());
                        let declaration_bits = GLOBAL_BRIDGE
                            .molt_value_for_pyobj(declaration.as_ptr())
                            .unwrap();
                        let declaration_bits = ProtectedRuntimeValue::from_owned_bits(
                            py.core_token(),
                            declaration_bits,
                        );
                        let (class, receiver) =
                            protocol_receiver(py, method, declaration_bits.bits());
                        drop(declaration_bits);
                        drop(declaration);
                        let view = refcount::ProtectedPyObject::from_owned(
                            GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(receiver.bits()),
                        );
                        assert!(!view.as_ptr().is_null());
                        for mode in 0..=6 {
                            if route != 0 && mode == 5 {
                                continue;
                            }
                            protocol_start(mode);
                            let status = match route {
                                0 => object::PyObject_IsTrue(view.as_ptr()),
                                1 => object::PyObject_Size(view.as_ptr()) as i32,
                                _ => object::PyObject_LengthHint(view.as_ptr(), 7) as i32,
                            };
                            assert_eq!(PROTOCOL.with(|slot| slot.get().calls), 1);
                            if mode == 5 {
                                assert_eq!(status, 1);
                                protocol_events(&[(1, 0)]);
                                continue;
                            }
                            assert_eq!(status, -1, "minor={minor} route={route} mode={mode}");
                            if mode == 6 {
                                let selected = refcount::ProtectedPyObject::from_owned(
                                    errors::PyErr_GetRaisedException(),
                                );
                                assert_eq!(selected.as_ptr(), failure.as_ptr());
                                let tb = refcount::ProtectedPyObject::from_owned(
                                    errors::PyException_GetTraceback(selected.as_ptr()),
                                );
                                assert_eq!(tb.as_ptr(), traceback.as_ptr());
                                protocol_events(&[(1, failure.as_ptr().addr())]);
                                assert_eq!(
                                    PROTOCOL.with(|slot| slot.get().tracebacks[0]),
                                    traceback.as_ptr().addr()
                                );
                                continue;
                            }
                            let (kind, text) = if route == 0 {
                                (
                                    &raw mut PyExc_TypeError,
                                    format!(
                                        "__bool__ should return bool, returned {}",
                                        if mode == 0 {
                                            "RetirementResult"
                                        } else if mode == 4 {
                                            "RetirementIndexed"
                                        } else {
                                            "RetirementInteger"
                                        }
                                    ),
                                )
                            } else if route == 2 && (mode == 0 || mode == 4) {
                                (
                                    &raw mut PyExc_TypeError,
                                    format!(
                                        "__length_hint__ must be an integer, not {}",
                                        if mode == 0 {
                                            "RetirementResult"
                                        } else {
                                            "RetirementIndexed"
                                        }
                                    ),
                                )
                            } else if route == 1 && mode == 0 {
                                (
                                    &raw mut PyExc_TypeError,
                                    "'RetirementResult' object cannot be interpreted as an integer"
                                        .to_string(),
                                )
                            } else if mode == 1 || (route == 1 && mode == 3) {
                                (
                                    &raw mut PyExc_ValueError,
                                    format!(
                                        "{}() should return >= 0",
                                        if route == 1 {
                                            "__len__"
                                        } else {
                                            "__length_hint__"
                                        }
                                    ),
                                )
                            } else if route == 2 {
                                (
                                    &raw mut PyExc_OverflowError,
                                    "Python int too large to convert to C ssize_t".to_string(),
                                )
                            } else {
                                (
                                    &raw mut PyExc_OverflowError,
                                    format!(
                                        "cannot fit '{}' into an index-sized integer",
                                        if mode == 4 {
                                            "RetirementIndexResult"
                                        } else {
                                            "RetirementInteger"
                                        }
                                    ),
                                )
                            };
                            let error = protocol_error(kind, &text);
                            let selected = error.as_ptr().addr();
                            if route == 0 {
                                if minor < 14 {
                                    protocol_events(&[(2, selected), (1, selected)]);
                                } else {
                                    protocol_events(&[(1, 0), (2, selected)]);
                                }
                            } else if route == 1 && mode == 4 {
                                assert_eq!(PROTOCOL.with(|slot| slot.get().indices), 1);
                                protocol_events(&[(1, 0), (2, 0), (3, selected)]);
                            } else {
                                let negative = mode == 1 || (route == 1 && mode == 3);
                                protocol_events(&[
                                    (1, 0),
                                    (2, if negative { 0 } else { selected }),
                                ]);
                                if route == 2 && mode == 4 {
                                    assert_eq!(PROTOCOL.with(|slot| slot.get().indices), 0);
                                }
                            }
                            assert!(!exception_pending(py));
                        }
                        if route == 1 {
                            protocol_start(4);
                            protocol_update(|state| state.index_fails = true);
                            assert_eq!(object::PyObject_Size(view.as_ptr()), -1);
                            let selected = refcount::ProtectedPyObject::from_owned(
                                errors::PyErr_GetRaisedException(),
                            );
                            assert_eq!(selected.as_ptr(), failure.as_ptr());
                            protocol_events(&[(1, 0), (2, failure.as_ptr().addr())]);
                            assert_eq!(
                                PROTOCOL.with(|slot| slot.get().tracebacks[1]),
                                traceback.as_ptr().addr()
                            );
                            assert_eq!(PROTOCOL.with(|slot| slot.get().indices), 1);
                            protocol_warning_policy(py, b"error");
                            protocol_start(4);
                            assert_eq!(object::PyObject_Size(view.as_ptr()), -1);
                            let warning = protocol_error(&raw mut PyExc_DeprecationWarning, "__index__ returned non-int (type RetirementIndexResult).  The ability to return an instance of a strict subclass of int is deprecated, and may be removed in a future version of Python.");
                            let warning_traceback = refcount::ProtectedPyObject::from_owned(errors::PyException_GetTraceback(warning.as_ptr()));
                            protocol_events(&[(1, 0), (3, warning.as_ptr().addr()), (2, warning.as_ptr().addr())]);
                            assert_eq!(PROTOCOL.with(|slot| slot.get().tracebacks[1]), warning_traceback.as_ptr().addr());
                            assert_eq!(PROTOCOL.with(|slot| slot.get().tracebacks[2]), warning_traceback.as_ptr().addr());
                            protocol_warning_policy(py, b"ignore");
                        }
                        // Default conversion must fail before target lookup. A
                        // native private-index result remains owned through overflow.
                        if route == 2 {
                            protocol_start(0);
                            let default = protocol_value(py, &raw mut *indexed);
                            let result = crate::builtins::operator::molt_operator_length_hint(
                                receiver.bits(),
                                default.bits(),
                            );
                            let result =
                                ProtectedRuntimeValue::from_owned_bits(py.core_token(), result);
                            assert!(obj_from_bits(result.bits()).is_none());
                            let error = protocol_error(
                                &raw mut PyExc_OverflowError,
                                "Python int too large to convert to C ssize_t",
                            );
                            assert_eq!(PROTOCOL.with(|slot| slot.get().bindings), 0);
                            assert_eq!(PROTOCOL.with(|slot| slot.get().indices), 1);
                            protocol_events(&[(3, error.as_ptr().addr())]);
                            protocol_start(0);
                            protocol_update(|state| state.index_fails = true);
                            let failed = ProtectedRuntimeValue::from_owned_bits(
                                py.core_token(),
                                crate::builtins::operator::molt_operator_length_hint(
                                    receiver.bits(),
                                    default.bits(),
                                ),
                            );
                            assert!(obj_from_bits(failed.bits()).is_none());
                            let selected = refcount::ProtectedPyObject::from_owned(
                                errors::PyErr_GetRaisedException(),
                            );
                            assert_eq!(selected.as_ptr(), failure.as_ptr());
                            assert_eq!(PROTOCOL.with(|slot| slot.get().bindings), 0);
                            assert_eq!(PROTOCOL.with(|slot| slot.get().indices), 1);
                            protocol_events(&[]);
                            protocol_start(7);
                            protocol_update(|state| {
                                state.index_huge = false;
                                state.index_negative = true;
                            });
                            let negative_default = ProtectedRuntimeValue::from_owned_bits(
                                py.core_token(),
                                crate::builtins::operator::molt_operator_length_hint(
                                    receiver.bits(),
                                    default.bits(),
                                ),
                            );
                            assert_eq!(
                                crate::to_i64(obj_from_bits(negative_default.bits())),
                                Some(-1)
                            );
                            assert!(!exception_pending(py));
                            assert_eq!(PROTOCOL.with(|slot| slot.get().bindings), 1);
                            protocol_events(&[(3, 0), (1, 0)]);
                            drop(default);
                        }
                        drop(view);
                        drop(receiver);
                        let retired = PROTOCOL.with(|slot| slot.get().descriptors);
                        drop(class);
                        assert_eq!(
                            PROTOCOL.with(|slot| slot.get().descriptors),
                            retired + 1,
                            "class must release its native descriptor before the declaration dies"
                        );
                        assert!(errors::PyErr_Occurred().is_null());
                    }
                    // The 3.14 no-args helper rejects a resolved None without
                    // invoking it; earlier targets retain normal call semantics.
                    let (none_class, none_receiver) =
                        protocol_receiver(py, b"__bool__", MoltObject::none().bits());
                    let len_declaration = protocol_value(py, &raw mut *descriptor);
                    let len_name = ProtectedRuntimeValue::from_owned_bits(py.core_token(), crate::attr_name_bits_from_bytes(py, b"__len__").unwrap());
                    dec_ref_bits(py, crate::molt_set_attr_name(none_class.class.bits(), len_name.bits(), len_declaration.bits()));
                    assert!(!exception_pending(py));
                    drop(len_declaration);
                    protocol_start(5);
                    assert_eq!(
                        crate::object::ops::truth_value(py, obj_from_bits(none_receiver.bits())),
                        Err(())
                    );
                    let _error = protocol_error(
                        &raw mut PyExc_TypeError,
                        if minor < 14 {
                            "'NoneType' object is not callable"
                        } else {
                            "'ManagedTruthLength' cannot be interpreted as a boolean"
                        },
                    );
                    assert_eq!(PROTOCOL.with(|slot| slot.get().calls), 0);
                    assert_eq!(PROTOCOL.with(|slot| slot.get().bindings), 0);
                    drop(none_receiver);
                    let retired = PROTOCOL.with(|slot| slot.get().descriptors);
                    drop(none_class);
                    assert_eq!(PROTOCOL.with(|slot| slot.get().descriptors), retired + 1);

                    // A real native heap type reaches the generated C bool
                    // sibling, rather than the managed truth hook above.
                    let native_class = protocol_native_class();
                    let declaration = refcount::ProtectedPyObject::from_owned(
                        typeobj::PyType_GenericAlloc(&raw mut *descriptor, 0),
                    );
                    assert!(!declaration.as_ptr().is_null());
                    assert_eq!(
                        object::PyObject_SetAttrString(
                            native_class.0.as_ptr(),
                            c"__bool__".as_ptr(),
                            declaration.as_ptr()
                        ),
                        0
                    );
                    drop(declaration);
                    let native_receiver = refcount::ProtectedPyObject::from_owned(
                        typeobj::PyType_GenericAlloc(native_class.0.as_ptr().cast(), 0),
                    );
                    assert!(!native_receiver.as_ptr().is_null());
                    protocol_start(0);
                    assert_eq!(object::PyObject_IsTrue(native_receiver.as_ptr()), -1);
                    let error = protocol_error(
                        &raw mut PyExc_TypeError,
                        "__bool__ should return bool, returned RetirementResult",
                    );
                    if minor < 14 {
                        protocol_events(&[(2, error.as_ptr().addr()), (1, error.as_ptr().addr())]);
                    } else {
                        protocol_events(&[(1, 0), (2, error.as_ptr().addr())]);
                    }
                    let retired = PROTOCOL.with(|slot| slot.get().descriptors);
                    assert_eq!(
                        object::PyObject_SetAttrString(
                            native_class.0.as_ptr(),
                            c"__bool__".as_ptr(),
                            &raw mut Py_None
                        ),
                        0
                    );
                    assert_eq!(PROTOCOL.with(|slot| slot.get().descriptors), retired + 1);
                    let len_declaration = refcount::ProtectedPyObject::from_owned(typeobj::PyType_GenericAlloc(&raw mut *descriptor, 0));
                    assert!(!len_declaration.as_ptr().is_null());
                    assert_eq!(object::PyObject_SetAttrString(native_class.0.as_ptr(), c"__len__".as_ptr(), len_declaration.as_ptr()), 0);
                    drop(len_declaration);
                    protocol_start(5);
                    assert_eq!(object::PyObject_IsTrue(native_receiver.as_ptr()), -1);
                    let _error = protocol_error(
                        &raw mut PyExc_TypeError,
                        if minor < 14 {
                            "'NoneType' object is not callable"
                        } else {
                            "'retirement.NativeBool' cannot be interpreted as a boolean"
                        },
                    );
                    assert_eq!(PROTOCOL.with(|slot| slot.get().calls), 0);
                    assert_eq!(PROTOCOL.with(|slot| slot.get().bindings), 0);
                    protocol_events(&[]);
                    drop(native_receiver);
                    let retired = PROTOCOL.with(|slot| slot.get().descriptors);
                    drop(native_class);
                    assert_eq!(PROTOCOL.with(|slot| slot.get().descriptors), retired + 1);

                    // Legacy int truncation consumes the same private native
                    // index producer, while 3.14 no longer admits __trunc__.
                    let declaration = refcount::ProtectedPyObject::from_owned(
                        typeobj::PyType_GenericAlloc(&raw mut *descriptor, 0),
                    );
                    assert!(!declaration.as_ptr().is_null());
                    let declaration_bits = ProtectedRuntimeValue::from_owned_bits(
                        py.core_token(),
                        GLOBAL_BRIDGE
                            .molt_value_for_pyobj(declaration.as_ptr())
                            .unwrap(),
                    );
                    let (trunc_class, trunc_receiver) =
                        protocol_receiver(py, b"__trunc__", declaration_bits.bits());
                    drop(declaration_bits);
                    drop(declaration);
                    protocol_start(4);
                    protocol_update(|state| state.index_huge = false);
                    let converted = crate::builtins::numbers::int_from_number_protocol(
                        py,
                        trunc_receiver.bits(),
                    );
                    if minor < 14 {
                        let converted = ProtectedRuntimeValue::from_owned_bits(
                            py.core_token(),
                            converted.expect("legacy truncation result"),
                        );
                        assert_eq!(
                            crate::builtins::numbers::integer_storage_i64(py, converted.bits()),
                            Some(1)
                        );
                        assert_eq!(PROTOCOL.with(|slot| slot.get().indices), 1);
                    } else {
                        assert_eq!(converted, None);
                        assert_eq!(PROTOCOL.with(|slot| slot.get().calls), 0);
                    }
                    assert!(errors::PyErr_Occurred().is_null());
                    drop(trunc_receiver);
                    let retired = PROTOCOL.with(|slot| slot.get().descriptors);
                    drop(trunc_class);
                    assert_eq!(PROTOCOL.with(|slot| slot.get().descriptors), retired + 1);
                });
            }
            // Slot-only FOREIGN inputs use private nb_index once. Both real
            // pending channels remain visible to that C callback; returning an
            // exact integer with a prior indicator is a direct-slot success.
            let source = protocol_value(py, &raw mut *indexed);
            for channel in [false, true] {
                for fails in [false, true] {
                    protocol_start(0);
                    protocol_update(|state| {
                        state.index_result = &raw mut PyLong_Type;
                        state.index_huge = false;
                        state.index_fails = fails;
                    });
                    if channel {
                        crate::record_exception(py, failure_bits);
                    } else {
                        errors::PyErr_SetRaisedException(object::Py_NewRef(failure.as_ptr()));
                    }
                    let converted =
                        crate::builtins::numbers::index_from_number_protocol(py, source.bits());
                    if fails {
                        assert_eq!(converted, None);
                    } else {
                        let converted = ProtectedRuntimeValue::from_owned_bits(
                            py.core_token(),
                            converted.expect("native index success with a prior error"),
                        );
                        let view = refcount::ProtectedPyObject::from_owned(
                            GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(converted.bits()),
                        );
                        assert_eq!(
                            view.as_ptr().addr(),
                            PROTOCOL.with(|slot| slot.get().index_returned)
                        );
                    }
                    let selected =
                        refcount::ProtectedPyObject::from_owned(errors::PyErr_GetRaisedException());
                    assert_eq!(selected.as_ptr(), failure.as_ptr());
                    assert_eq!(
                        PROTOCOL.with(|slot| slot.get().index_entry),
                        (failure.as_ptr().addr(), traceback.as_ptr().addr())
                    );
                    assert_eq!(PROTOCOL.with(|slot| slot.get().indices), 1);
                }
            }
            // The same producer feeds required index, int's index fallback,
            // and float's index fallback without changing each public policy.
            for consumer in 0..3 {
                protocol_start(0);
                protocol_update(|state| {
                    state.index_result = &raw mut *index_result;
                    state.index_huge = false;
                });
                let value = match consumer {
                    0 => crate::builtins::numbers::index_from_object(py, source.bits()),
                    1 => crate::builtins::numbers::int_from_number_protocol(py, source.bits())
                        .expect("native index int fallback"),
                    _ => crate::builtins::numbers::float_from_number_protocol(py, source.bits())
                        .expect("native index float fallback")
                        .into_bits(py),
                };
                let value = ProtectedRuntimeValue::from_owned_bits(py.core_token(), value);
                assert!(!exception_pending(py));
                assert_eq!(PROTOCOL.with(|slot| slot.get().indices), 1);
                if consumer == 2 {
                    assert_eq!(
                        crate::builtins::numbers::float_storage_value(py, value.bits()),
                        Ok(Some(1.0))
                    );
                } else {
                    assert_eq!(
                        crate::builtins::numbers::integer_storage_i64(py, value.bits()),
                        Some(1)
                    );
                }
            }
            drop(source);
        }
    });
}
