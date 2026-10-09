//! C origin, runtime origin and failed adoption are distinct ownership contracts.
//! CPython 3.12.13/3.13.11/3.14.3 longobject/floatobject constructors produce
//! separate noncached objects; C getters return the retained original object.
mod support;
use molt_cpython_abi::hooks::{OwnedHandleResult, STUB_HOOKS};
use molt_cpython_abi::{
    abi_types::*,
    api::{errors, numbers, refcount},
    bridge::{self, GLOBAL_BRIDGE},
};
use molt_lang_obj_model::MoltObject;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
static REJECT_NEW: AtomicBool = AtomicBool::new(false);
static REJECT_MARK: AtomicBool = AtomicBool::new(false);
static NEW_CALLS: AtomicUsize = AtomicUsize::new(0);
static COMPLEX_CALLS: AtomicUsize = AtomicUsize::new(0);
static WORD_CALLS: AtomicUsize = AtomicUsize::new(0);
static BYTE_CALLS: AtomicUsize = AtomicUsize::new(0);
static ERROR_AFTER_BYTES: AtomicBool = AtomicBool::new(false);
static REENTER_SOURCE: AtomicUsize = AtomicUsize::new(0);
static REENTER_WINNER: AtomicU64 = AtomicU64::new(0);
static REJECT_AFTER_REENTRY: AtomicBool = AtomicBool::new(false);
unsafe extern "C" fn unexpected_i64(_: i64) -> u64 {
    WORD_CALLS.fetch_add(1, Ordering::SeqCst);
    0
}
unsafe extern "C" fn unexpected_u64(_: u64) -> u64 {
    WORD_CALLS.fetch_add(1, Ordering::SeqCst);
    0
}
unsafe extern "C" fn from_bytes(data: *const u8, len: usize, little: i32, signed: i32) -> u64 {
    BYTE_CALLS.fetch_add(1, Ordering::SeqCst);
    let bits = unsafe { support::fake_runtime::int_from_bytes(data, len, little, signed) };
    if ERROR_AFTER_BYTES.load(Ordering::SeqCst) {
        unsafe { errors::PyErr_SetNone((&raw mut PyExc_ValueError).cast()) };
    }
    bits
}
unsafe extern "C" fn complex_new(real: f64, imag: f64) -> OwnedHandleResult {
    COMPLEX_CALLS.fetch_add(1, Ordering::SeqCst);
    if REJECT_NEW.load(Ordering::SeqCst) {
        OwnedHandleResult::error()
    } else {
        unsafe { support::fake_complex::from_doubles(real, imag) }
    }
}
unsafe extern "C" fn numeric_new(bits: u64) -> OwnedHandleResult {
    NEW_CALLS.fetch_add(1, Ordering::SeqCst);
    let source = REENTER_SOURCE.swap(0, Ordering::SeqCst);
    if source != 0 {
        let winner = unsafe {
            bridge::molt_capi_pyobj_to_handle(std::ptr::with_exposed_provenance_mut(source))
        };
        REENTER_WINNER.store(winner, Ordering::SeqCst);
        if winner == 0 || REJECT_AFTER_REENTRY.load(Ordering::SeqCst) {
            return OwnedHandleResult::error();
        }
    }
    if REJECT_NEW.load(Ordering::SeqCst) {
        OwnedHandleResult::error()
    } else {
        unsafe { support::fake_runtime::numeric_identity_new(bits) }
    }
}
unsafe extern "C" fn mark(bits: u64, present: i32) -> i32 {
    let tag = unsafe { support::fake_runtime::classify_heap(bits) };
    i32::from(
        unsafe { support::fake_runtime::try_mark_abi_view(bits, present) } != 0
            && !(present != 0
                && REJECT_MARK.load(Ordering::SeqCst)
                && (tag == MoltTypeTag::Int as u8 || tag == MoltTypeTag::Float as u8)),
    )
}
#[test]
fn numeric_identity_admission_preserves_origins_and_failed_sources() {
    bridge::molt_cpython_abi_init();
    unsafe {
        // Real pre-registration physical allocations, not manufactured map rows.
        let old_int = numbers::PyLong_FromLong(1000);
        let old_float = numbers::PyFloat_FromDouble(1.25);
        assert!(!old_int.is_null() && !old_float.is_null());
        bridge::molt_capi_any_incref(old_int);
        assert_eq!((*old_int).ob_refcnt, 2);
        assert_eq!(numbers::PyLong_AsLong(old_int), 1000);
        assert_eq!(numbers::PyFloat_AsDouble(old_float), 1.25);
        let original_bits = GLOBAL_BRIDGE.molt_handle_for_pyobj(old_int).unwrap().bits();
        let mut hooks = STUB_HOOKS;
        support::fake_runtime::wire(&mut hooks);
        hooks.numeric_identity_new = Some(numeric_new);
        hooks.complex_from_doubles = complex_new;
        hooks.int_from_i64 = unexpected_i64;
        hooks.int_from_u64 = unexpected_u64;
        hooks.int_from_bytes = from_bytes;
        hooks.try_mark_abi_view = mark;
        let _abi_test = support::enter_runtime_class_abi_test(hooks);
        let initial_live = support::fake_runtime::live_numeric_count();

        let pending_source = numbers::PyLong_FromLong(1000);
        let calls = NEW_CALLS.load(Ordering::SeqCst);
        errors::PyErr_SetNone((&raw mut PyExc_ValueError).cast());
        let original_error = errors::PyErr_Occurred();
        assert!(!original_error.is_null());
        assert_eq!(bridge::molt_capi_pyobj_to_handle(pending_source), 0);
        assert_eq!(
            NEW_CALLS.load(Ordering::SeqCst),
            calls,
            "incoming C error refuses before identity allocation"
        );
        assert_eq!(errors::PyErr_Occurred(), original_error);
        assert_eq!((*pending_source).ob_refcnt, 1);
        assert_eq!(numbers::PyLong_AsLong(pending_source), 1000);
        refcount::Py_DECREF(pending_source);
        errors::PyErr_Clear();

        REJECT_NEW.store(true, Ordering::SeqCst);
        let calls = NEW_CALLS.load(Ordering::SeqCst);
        let c_only = numbers::PyLong_FromLong(1000);
        assert!(!c_only.is_null());
        assert_eq!(numbers::PyLong_AsLong(c_only), 1000);
        assert_eq!(numbers::PyLong_Check(c_only), 1);
        assert_eq!(numbers::PyLong_CheckExact(c_only), 1);
        assert_eq!(numbers::PyBool_Check(c_only), 0);
        assert_eq!(numbers::PyFloat_AsDouble(c_only), 1000.0);
        assert_eq!(numbers::PyComplex_ImagAsDouble(c_only), 0.0);
        assert_eq!(
            bridge::molt_capi_semantic_type(c_only),
            &raw mut PyLong_Type
        );
        bridge::molt_capi_any_incref(c_only);
        bridge::molt_capi_any_decref(c_only);
        assert_eq!(
            NEW_CALLS.load(Ordering::SeqCst),
            calls,
            "C-only operations do not allocate runtime identity"
        );
        assert_eq!(support::fake_runtime::live_numeric_count(), initial_live);
        assert_eq!(
            bridge::molt_capi_pyobj_to_handle(c_only),
            0,
            "producer refusal cannot become a foreign or inline identity"
        );
        assert_eq!(NEW_CALLS.load(Ordering::SeqCst), calls + 1);
        assert_eq!((*c_only).ob_refcnt, 1);
        assert_eq!(numbers::PyLong_AsLong(c_only), 1000);
        assert!(!errors::PyErr_Occurred().is_null());
        refcount::Py_DECREF(c_only);
        errors::PyErr_Clear();
        assert_eq!(bridge::molt_capi_pyobj_to_handle(old_int), 0);
        let error = errors::PyErr_Occurred();
        assert!(!error.is_null());
        assert_eq!(
            GLOBAL_BRIDGE.molt_handle_for_pyobj(old_int).unwrap().bits(),
            original_bits
        );
        let calls = NEW_CALLS.load(Ordering::SeqCst);
        bridge::molt_capi_any_incref(old_int);
        assert_eq!((*old_int).ob_refcnt, 3);
        bridge::molt_capi_any_decref(old_int);
        assert_eq!((*old_int).ob_refcnt, 2);
        assert_eq!(
            NEW_CALLS.load(Ordering::SeqCst),
            calls,
            "reference ownership never adopts"
        );
        assert_eq!(errors::PyErr_Occurred(), error);
        assert_eq!(
            numbers::PyLong_AsLong(old_int),
            1000,
            "physical extraction needs no identity allocation"
        );
        assert_eq!(NEW_CALLS.load(Ordering::SeqCst), calls);
        errors::PyErr_Clear();
        assert_eq!(support::fake_runtime::live_numeric_count(), initial_live);

        REJECT_NEW.store(false, Ordering::SeqCst);
        REJECT_MARK.store(true, Ordering::SeqCst);
        assert_eq!(bridge::molt_capi_pyobj_to_handle(old_float), 0);
        assert_eq!((*old_float).ob_refcnt, 1);
        assert_eq!(numbers::PyFloat_AsDouble(old_float), 1.25);
        assert!(GLOBAL_BRIDGE.managed_handle_for_pyobj(old_float).is_none());
        assert_eq!(
            support::fake_runtime::live_numeric_count(),
            initial_live,
            "rejected staged hold retired"
        );
        errors::PyErr_Clear();
        REJECT_MARK.store(false, Ordering::SeqCst);

        let calls = NEW_CALLS.load(Ordering::SeqCst);
        let int_bits = bridge::molt_capi_pyobj_to_handle(old_int);
        assert_eq!(NEW_CALLS.load(Ordering::SeqCst), calls + 1);
        assert_eq!(bridge::molt_capi_pyobj_to_handle(old_int), int_bits);
        assert_eq!(
            NEW_CALLS.load(Ordering::SeqCst),
            calls + 1,
            "one identity per origin"
        );
        let float_bits = bridge::molt_capi_pyobj_to_handle(old_float);
        assert!(MoltObject::from_bits(int_bits).is_ptr());
        assert!(MoltObject::from_bits(float_bits).is_ptr());
        assert_eq!((*old_int).ob_refcnt, 2, "adoption preserves external N");
        support::fake_runtime::inc_ref(int_bits);
        bridge::molt_capi_any_decref(old_int);
        bridge::molt_capi_any_decref(old_int);
        assert_eq!(
            GLOBAL_BRIDGE.handle_to_borrowed_pyobj(int_bits),
            old_int,
            "runtime hold retains original C origin"
        );
        let returned = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(int_bits);
        assert_eq!(returned, old_int);
        support::fake_runtime::dec_ref(int_bits);
        refcount::Py_DECREF(returned);
        assert!(!support::fake_runtime::contains(int_bits));
        assert!(GLOBAL_BRIDGE.managed_handle_for_pyobj(old_int).is_none());
        refcount::Py_DECREF(old_float);
        assert!(!support::fake_runtime::contains(float_bits));

        for value in [i64::MIN, -(1i64 << 48), (1i64 << 48), i64::MAX] {
            let before_new = NEW_CALLS.load(Ordering::SeqCst);
            let before_bytes = BYTE_CALLS.load(Ordering::SeqCst);
            let pointer = numbers::PyLong_FromLongLong(value);
            assert!(!pointer.is_null());
            assert_eq!(numbers::PyLong_AsLongLong(pointer), value);
            bridge::molt_capi_any_incref(pointer);
            bridge::molt_capi_any_decref(pointer);
            assert_eq!(WORD_CALLS.load(Ordering::SeqCst), 0);
            assert_eq!(NEW_CALLS.load(Ordering::SeqCst), before_new);
            assert_eq!(BYTE_CALLS.load(Ordering::SeqCst), before_bytes);
            let bits = bridge::molt_capi_pyobj_to_handle(pointer);
            assert_ne!(bits, 0);
            assert_eq!(
                support::fake_runtime::integer_value(bits),
                Some(i128::from(value))
            );
            assert_eq!(GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits), pointer);
            assert_eq!(BYTE_CALLS.load(Ordering::SeqCst), before_bytes + 1);
            assert_eq!(bridge::molt_capi_pyobj_to_handle(pointer), bits);
            assert_eq!(BYTE_CALLS.load(Ordering::SeqCst), before_bytes + 1);
            refcount::Py_DECREF(pointer);
        }
        let errored_source = numbers::PyLong_FromUnsignedLongLong(u64::MAX);
        let before_live = support::fake_runtime::live_numeric_count();
        let before_bytes = BYTE_CALLS.load(Ordering::SeqCst);
        ERROR_AFTER_BYTES.store(true, Ordering::SeqCst);
        assert_eq!(bridge::molt_capi_pyobj_to_handle(errored_source), 0);
        ERROR_AFTER_BYTES.store(false, Ordering::SeqCst);
        assert_eq!(BYTE_CALLS.load(Ordering::SeqCst), before_bytes + 1);
        assert_eq!(
            support::fake_runtime::live_numeric_count(),
            before_live,
            "owned raw decode result retires when its producer also raises"
        );
        assert_eq!(errors::PyErr_Occurred(), (&raw mut PyExc_ValueError).cast());
        assert_eq!((*errored_source).ob_refcnt, 1);
        assert_eq!(numbers::PyLong_AsUnsignedLongLong(errored_source), u64::MAX);
        errors::PyErr_Clear();
        refcount::Py_DECREF(errored_source);

        let malformed = numbers::PyLong_FromUnsignedLongLong(u64::MAX);
        let tag = &raw mut (*malformed.cast::<PyLongObject>()).long_value.lv_tag;
        let original_tag = *tag;
        let before_bytes = BYTE_CALLS.load(Ordering::SeqCst);
        let before_live = support::fake_runtime::live_numeric_count();
        *tag = usize::MAX & !3usize; // impossible digit count, positive sign
        assert_eq!(bridge::molt_capi_pyobj_to_handle(malformed), 0);
        assert_eq!((*malformed).ob_refcnt, 1);
        assert_eq!(BYTE_CALLS.load(Ordering::SeqCst), before_bytes);
        assert_eq!(support::fake_runtime::live_numeric_count(), before_live);
        assert!(!errors::PyErr_Occurred().is_null());
        *tag = original_tag;
        errors::PyErr_Clear();
        assert_eq!(numbers::PyLong_AsUnsignedLongLong(malformed), u64::MAX);
        refcount::Py_DECREF(malformed);

        let before_bytes = BYTE_CALLS.load(Ordering::SeqCst);
        let maximum = numbers::PyLong_FromUnsignedLongLong(u64::MAX);
        assert!(!maximum.is_null());
        assert_eq!(numbers::PyLong_AsUnsignedLongLong(maximum), u64::MAX);
        assert_eq!(WORD_CALLS.load(Ordering::SeqCst), 0);
        assert_eq!(BYTE_CALLS.load(Ordering::SeqCst), before_bytes);
        let maximum_bits = bridge::molt_capi_pyobj_to_handle(maximum);
        assert_eq!(
            support::fake_runtime::integer_value(maximum_bits),
            Some(i128::from(u64::MAX))
        );
        assert_eq!(BYTE_CALLS.load(Ordering::SeqCst), before_bytes + 1);
        refcount::Py_DECREF(maximum);

        let before_complex = COMPLEX_CALLS.load(Ordering::SeqCst);
        let complex = numbers::PyComplex_FromDoubles(1.25, -2.5);
        assert!(!complex.is_null());
        assert_eq!(numbers::PyComplex_Check(complex), 1);
        assert_eq!(numbers::PyComplex_CheckExact(complex), 1);
        assert_eq!(numbers::PyComplex_RealAsDouble(complex), 1.25);
        assert_eq!(numbers::PyComplex_ImagAsDouble(complex), -2.5);
        assert_eq!(COMPLEX_CALLS.load(Ordering::SeqCst), before_complex);
        REJECT_NEW.store(true, Ordering::SeqCst);
        assert_eq!(bridge::molt_capi_pyobj_to_handle(complex), 0);
        assert_eq!((*complex).ob_refcnt, 1);
        assert_eq!(numbers::PyComplex_RealAsDouble(complex), 1.25);
        errors::PyErr_Clear();
        REJECT_NEW.store(false, Ordering::SeqCst);
        let complex_bits = bridge::molt_capi_pyobj_to_handle(complex);
        assert_eq!(
            support::fake_runtime::complex_value(complex_bits),
            Some((1.25, -2.5))
        );
        assert_eq!(
            GLOBAL_BRIDGE.handle_to_borrowed_pyobj(complex_bits),
            complex
        );
        assert_eq!(bridge::molt_capi_pyobj_to_handle(complex), complex_bits);
        assert_eq!(COMPLEX_CALLS.load(Ordering::SeqCst), before_complex + 2);
        refcount::Py_DECREF(complex);

        // Allocation can reenter the same source before the outer reservation
        // commits. Only a successful producer may reuse the exact winner.
        for reject_outer in [false, true] {
            let source = numbers::PyLong_FromLong(1000);
            let before_live = support::fake_runtime::live_numeric_count();
            let before_calls = NEW_CALLS.load(Ordering::SeqCst);
            REJECT_AFTER_REENTRY.store(reject_outer, Ordering::SeqCst);
            REENTER_SOURCE.store(source.expose_provenance(), Ordering::SeqCst);
            let returned = bridge::molt_capi_pyobj_to_handle(source);
            let winner = REENTER_WINNER.load(Ordering::SeqCst);
            assert_ne!(winner, 0);
            assert_eq!(NEW_CALLS.load(Ordering::SeqCst), before_calls + 2);
            assert_eq!(GLOBAL_BRIDGE.managed_handle_for_pyobj(source), Some(winner));
            assert_eq!(
                support::fake_runtime::live_numeric_count(),
                before_live + 1,
                "only the committed winner remains; redundant staged B retires"
            );
            if reject_outer {
                assert_eq!(
                    returned, 0,
                    "Some producer failure cannot reuse inner success"
                );
                assert!(!errors::PyErr_Occurred().is_null());
                errors::PyErr_Clear();
            } else {
                assert_eq!(returned, winner);
                assert!(errors::PyErr_Occurred().is_null());
            }
            assert_eq!(numbers::PyLong_AsLong(source), 1000);
            refcount::Py_DECREF(source);
            assert_eq!(support::fake_runtime::live_numeric_count(), before_live);
        }
        REJECT_AFTER_REENTRY.store(false, Ordering::SeqCst);

        let first = numbers::PyLong_FromLong(1000);
        let second = numbers::PyLong_FromLong(1000);
        assert_ne!(first, second);
        assert_ne!(
            bridge::molt_capi_pyobj_to_handle(first),
            bridge::molt_capi_pyobj_to_handle(second)
        );
        assert_eq!(numbers::PyLong_AsLong(first), 1000);
        assert_eq!(numbers::PyLong_AsLong(second), 1000);
        refcount::Py_DECREF(first);
        refcount::Py_DECREF(second);
        for value in [1.25, 0.0, -0.0, f64::INFINITY, f64::NAN] {
            let first = numbers::PyFloat_FromDouble(value);
            let second = numbers::PyFloat_FromDouble(value);
            assert!(!first.is_null() && !second.is_null());
            assert_ne!(first, second);
            assert_ne!(
                bridge::molt_capi_pyobj_to_handle(first),
                bridge::molt_capi_pyobj_to_handle(second)
            );
            let observed = numbers::PyFloat_AsDouble(first);
            if value.is_nan() {
                assert!(observed.is_nan());
            } else {
                assert_eq!(observed.to_bits(), value.to_bits());
            }
            refcount::Py_DECREF(first);
            refcount::Py_DECREF(second);
        }
        let bytes = [0xe8u8, 0x03];
        for signed in [true, false] {
            let constructor = if signed {
                numbers::PyLong_FromNativeBytes
            } else {
                numbers::PyLong_FromUnsignedNativeBytes
            };
            let first = constructor(bytes.as_ptr().cast(), bytes.len(), 1);
            let second = constructor(bytes.as_ptr().cast(), bytes.len(), 1);
            assert!(!first.is_null() && !second.is_null());
            assert_ne!(first, second);
            assert_eq!(numbers::PyLong_AsLong(first), 1000);
            refcount::Py_DECREF(first);
            refcount::Py_DECREF(second);
        }
        assert_eq!(
            numbers::PyLong_FromLong(7),
            numbers::PyLong_FromLong(7),
            "existing small cache remains canonical"
        );
        for heap in [
            support::fake_runtime::heap_integer(1000),
            support::fake_runtime::heap_float(1.25),
            support::fake_runtime::heap_complex(1.25, -2.5),
        ] {
            support::fake_runtime::inc_ref(heap); // independent runtime alias A
            let first = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(heap);
            let second = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(heap);
            assert_eq!(first, second);
            assert_eq!(
                bridge::molt_capi_pyobj_to_handle(first),
                heap,
                "runtime origin A is never reboxed as B"
            );
            refcount::Py_DECREF(first);
            refcount::Py_DECREF(second);
            support::fake_runtime::dec_ref(heap);
            support::fake_runtime::dec_ref(heap);
            assert!(!support::fake_runtime::contains(heap));
        }
        assert_eq!(support::fake_runtime::live_numeric_count(), initial_live);
        assert!(errors::PyErr_Occurred().is_null());
    }
}
