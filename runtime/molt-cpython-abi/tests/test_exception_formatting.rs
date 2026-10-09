//! ABI allocation-boundary probes. Semantic rendering uses the real runtime in
//! cpython_abi_hooks::exception_rendering_tests; these injected allocator hooks
//! prove rejection before allocation and publication failure without readiness.
mod support;

use molt_cpython_abi::abi_types::*;
use molt_cpython_abi::api::{errors, object, refcount, strings};
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
use molt_cpython_abi::hooks::BorrowedHandleResult;
use molt_lang_obj_model::MoltObject;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

// The fixture stores bytes without decoding. Expectations below are literal
// Python-codepoint encodings, independent of the implementation's text view.
static CLASS_ANCHORS: [u64; 2] = [0; 2];
static FAIL_NEXT_TEXT_ALLOC: AtomicBool = AtomicBool::new(false);
static NON_UTF8_ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static REENTER_TEXT_ALLOC: AtomicBool = AtomicBool::new(false);
static REJECT_TEXT_CLASS: AtomicBool = AtomicBool::new(false);
static WRONG_TEXT_CLASS: AtomicBool = AtomicBool::new(false);
static DIAGNOSTIC_REENTRIES: AtomicUsize = AtomicUsize::new(0);
static CALLBACK_ERROR: AtomicUsize = AtomicUsize::new(0);
static CLASS_CALLBACK_ERROR: AtomicUsize = AtomicUsize::new(0);
static RETURN_TEXT_WITH_ERROR: AtomicBool = AtomicBool::new(false);
static OWNED_ERROR_TEXT: AtomicU64 = AtomicU64::new(0);
static OWNED_ERROR_TEXT_RELEASES: AtomicUsize = AtomicUsize::new(0);
fn class_bits(index: usize) -> u64 {
    MoltObject::from_ptr((&raw const CLASS_ANCHORS[index]).cast_mut().cast::<u8>()).bits()
}

unsafe extern "C" fn text_classify(bits: u64) -> u8 {
    if (0..2).any(|index| class_bits(index) == bits) {
        MoltTypeTag::Type as u8
    } else if support::fake_strings::contains(bits) {
        MoltTypeTag::Str as u8
    } else {
        MoltTypeTag::Other as u8
    }
}

unsafe extern "C" fn text_runtime_class(bits: u64) -> BorrowedHandleResult {
    match unsafe { text_classify(bits) } {
        tag if tag == MoltTypeTag::Type as u8 => BorrowedHandleResult::ok(class_bits(0)),
        tag if tag == MoltTypeTag::Str as u8 => {
            let error = CLASS_CALLBACK_ERROR.load(Ordering::SeqCst) as *mut PyObject;
            if !error.is_null() {
                unsafe { errors::PyErr_SetRaisedException(object::Py_NewRef(error)) };
                return BorrowedHandleResult::error();
            }
            if WRONG_TEXT_CLASS.load(Ordering::SeqCst) {
                return BorrowedHandleResult::ok(class_bits(0));
            }
            if REJECT_TEXT_CLASS.load(Ordering::SeqCst) {
                if DIAGNOSTIC_REENTRIES.fetch_add(1, Ordering::SeqCst) >= 64 {
                    unsafe { errors::PyErr_SetNone((&raw mut PyExc_LookupError).cast()) };
                }
                BorrowedHandleResult::error()
            } else {
                BorrowedHandleResult::ok(class_bits(1))
            }
        }
        _ => BorrowedHandleResult::error(),
    }
}

unsafe extern "C" fn text_alloc(data: *const u8, length: usize) -> u64 {
    if REENTER_TEXT_ALLOC.load(Ordering::SeqCst) {
        if DIAGNOSTIC_REENTRIES.fetch_add(1, Ordering::SeqCst) >= 64 {
            unsafe { errors::PyErr_SetNone((&raw mut PyExc_LookupError).cast()) };
            return 0;
        }
        unsafe {
            errors::PyErr_SetString(
                (&raw mut PyExc_ValueError).cast(),
                c"reentrant diagnostic allocator".as_ptr(),
            );
        }
        return 0;
    }
    let callback_error = CALLBACK_ERROR.load(Ordering::SeqCst) as *mut PyObject;
    if !callback_error.is_null() {
        unsafe { errors::PyErr_SetRaisedException(object::Py_NewRef(callback_error)) };
        return if RETURN_TEXT_WITH_ERROR.load(Ordering::SeqCst) {
            let bits = unsafe { support::fake_strings::alloc_str(data, length) };
            OWNED_ERROR_TEXT.store(bits, Ordering::SeqCst);
            bits
        } else {
            0
        };
    }
    if FAIL_NEXT_TEXT_ALLOC.swap(false, Ordering::SeqCst) {
        return 0;
    }
    if length != 0
        && std::str::from_utf8(unsafe { std::slice::from_raw_parts(data, length) }).is_err()
    {
        NON_UTF8_ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
    }
    unsafe { support::fake_strings::alloc_str(data, length) }
}

unsafe extern "C" fn text_release(bits: u64) {
    if bits == OWNED_ERROR_TEXT.load(Ordering::SeqCst) {
        OWNED_ERROR_TEXT_RELEASES.fetch_add(1, Ordering::SeqCst);
    }
}

unsafe fn raw_fixture(bytes: &[u8]) -> *mut PyObject {
    let bits = unsafe { support::fake_strings::alloc_str(bytes.as_ptr(), bytes.len()) };
    unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) }
}

#[test]
fn text_ingress_and_publication_preserve_allocation_failures() {
    let mut hooks = support::stub_runtime_hooks();
    support::fake_strings::wire(&mut hooks);
    hooks.alloc_str = Some(text_alloc);
    hooks.classify_heap = Some(text_classify);
    hooks.runtime_class_borrowed = Some(text_runtime_class);
    hooks.dec_ref = text_release;
    let _abi_test = support::enter_abi_test(hooks);
    unsafe {
        for (index, class) in [&raw mut PyType_Type, &raw mut PyUnicode_Type]
            .into_iter()
            .enumerate()
        {
            GLOBAL_BRIDGE
                .bind_static_pyobj_to_runtime_handle(class.cast(), class_bits(index), true)
                .expect("bind fixture class for allocation-boundary probe");
        }
        // Public UTF-8 ingress stays strict even though internal text accepts surrogates.
        for invalid in [
            b"\xed\xa0\x80".as_slice(),
            b"\xed\xb0\x80",
            b"\xff",
            b"\xc0\x80",
            b"\xf4\x90\x80\x80",
        ] {
            let before = NON_UTF8_ALLOCATIONS.load(Ordering::SeqCst);
            assert!(
                strings::PyUnicode_FromStringAndSize(
                    invalid.as_ptr().cast(),
                    invalid.len() as isize
                )
                .is_null()
            );
            // This allocator probe intentionally has no bytes/exception
            // constructor. The identical corpus's UnicodeDecodeError class
            // assertions live in exception_rendering_tests against the real
            // runtime. Here the independent invariant is that malformed input
            // cannot reach the text allocator or return silent success.
            assert!(!errors::PyErr_Occurred().is_null());
            assert_eq!(
                NON_UTF8_ALLOCATIONS.load(Ordering::SeqCst),
                before,
                "invalid external bytes never reach alloc_str"
            );
            errors::PyErr_Clear();
        }

        let high = strings::PyUnicode_FromOrdinal(0xd800);
        let low = strings::PyUnicode_FromOrdinal(0xdc00);
        assert!(!high.is_null() && !low.is_null());
        FAIL_NEXT_TEXT_ALLOC.store(true, Ordering::SeqCst);
        assert!(strings::PyUnicode_Concat(high, low).is_null());
        assert_eq!(
            errors::PyErr_ExceptionMatches((&raw mut PyExc_MemoryError).cast()),
            1
        );
        errors::PyErr_Clear();
        let malformed = raw_fixture(b"\xff");
        assert!(
            malformed.is_null(),
            "invalid Python text is rejected before C publication"
        );
        assert_eq!(
            errors::PyErr_ExceptionMatches((&raw mut PyExc_SystemError).cast()),
            1
        );
        errors::PyErr_Clear();

        // Installed allocator failure is MemoryError, not empty construction
        // of the originally requested exception.
        FAIL_NEXT_TEXT_ALLOC.store(true, Ordering::SeqCst);
        errors::PyErr_SetString((&raw mut PyExc_ValueError).cast(), c"allocation".as_ptr());
        assert_eq!(
            errors::PyErr_Occurred(),
            (&raw mut PyExc_MemoryError).cast()
        );
        let allocation_failure = errors::PyErr_GetRaisedException();
        let allocation_failure_owner = refcount::OwnedPyObject::from_owned(allocation_failure);
        assert!(!allocation_failure.is_null());
        drop(allocation_failure_owner);

        // These faults occur before the class constructor is entered. Both
        // must enter the same bounded scope as SetObject/Restore/Normalize.
        for (name, fault) in [
            ("allocator reentry", &REENTER_TEXT_ALLOC),
            ("semantic class reentry", &REJECT_TEXT_CLASS),
        ] {
            eprintln!("diagnostic preparation probe: {name}");
            DIAGNOSTIC_REENTRIES.store(0, Ordering::SeqCst);
            fault.store(true, Ordering::SeqCst);
            errors::PyErr_SetString((&raw mut PyExc_ValueError).cast(), c"reenter".as_ptr());
            fault.store(false, Ordering::SeqCst);
            let calls = DIAGNOSTIC_REENTRIES.load(Ordering::SeqCst);
            assert!(calls > 0 && calls < 64, "{name} must be bounded: {calls}");
            assert_eq!(
                errors::PyErr_Occurred(),
                (&raw mut PyExc_RecursionError).cast()
            );
            let raised = errors::PyErr_GetRaisedException();
            let raised_owner = refcount::OwnedPyObject::from_owned(raised);
            assert!(
                !raised.is_null(),
                "recovery uses the canonical native constructor"
            );
            assert_eq!((*raised).ob_type, &raw mut PyExc_RecursionError);
            drop(raised_owner);
            assert!(errors::PyErr_Occurred().is_null());

            errors::PyErr_SetString((&raw mut PyExc_IndexError).cast(), c"recovered".as_ptr());
            assert_eq!(errors::PyErr_Occurred(), (&raw mut PyExc_IndexError).cast());
            let recovered = errors::PyErr_GetRaisedException();
            let recovered_owner = refcount::OwnedPyObject::from_owned(recovered);
            assert!(
                !recovered.is_null(),
                "a later normalization must still succeed"
            );
            drop(recovered_owner);
        }

        // Physical Unicode storage cannot stand in for semantic admission of
        // the allocator's result as a string.
        WRONG_TEXT_CLASS.store(true, Ordering::SeqCst);
        errors::PyErr_SetString(
            (&raw mut PyExc_ValueError).cast(),
            c"invalid text class".as_ptr(),
        );
        WRONG_TEXT_CLASS.store(false, Ordering::SeqCst);
        assert_eq!(
            errors::PyErr_Occurred(),
            (&raw mut PyExc_SystemError).cast()
        );
        let rejected = errors::PyErr_GetRaisedException();
        let rejected_owner = refcount::OwnedPyObject::from_owned(rejected);
        assert!(
            !rejected.is_null(),
            "producer violation uses native exception construction"
        );
        drop(rejected_owner);

        // Preserve an allocator's exact selected exception for both failure
        // and an owned output returned alongside that exception.
        errors::PyErr_SetNone((&raw mut PyExc_TypeError).cast());
        let original = errors::PyErr_GetRaisedException();
        let original_owner = refcount::OwnedPyObject::from_owned(original);
        assert!(!original.is_null());
        struct ResetCallbackErrorSlots;
        impl Drop for ResetCallbackErrorSlots {
            fn drop(&mut self) {
                CALLBACK_ERROR.store(0, Ordering::SeqCst);
                CLASS_CALLBACK_ERROR.store(0, Ordering::SeqCst);
            }
        }
        let callback_slots = ResetCallbackErrorSlots;
        CALLBACK_ERROR.store(original.addr(), Ordering::SeqCst);
        for returns_text in [false, true] {
            RETURN_TEXT_WITH_ERROR.store(returns_text, Ordering::SeqCst);
            OWNED_ERROR_TEXT_RELEASES.store(0, Ordering::SeqCst);
            errors::PyErr_SetString((&raw mut PyExc_ValueError).cast(), c"callback".as_ptr());
            let selected = errors::PyErr_GetRaisedException();
            let selected_owner = refcount::OwnedPyObject::from_owned(selected);
            assert_eq!(
                selected, original,
                "diagnostics cannot replace the callback's exception"
            );
            drop(selected_owner);
            assert_eq!(
                OWNED_ERROR_TEXT_RELEASES.load(Ordering::SeqCst),
                usize::from(returns_text)
            );
        }
        CALLBACK_ERROR.store(0, Ordering::SeqCst);
        CLASS_CALLBACK_ERROR.store(original.addr(), Ordering::SeqCst);
        errors::PyErr_SetString(
            (&raw mut PyExc_ValueError).cast(),
            c"class callback".as_ptr(),
        );
        CLASS_CALLBACK_ERROR.store(0, Ordering::SeqCst);
        let selected = errors::PyErr_GetRaisedException();
        let selected_owner = refcount::OwnedPyObject::from_owned(selected);
        assert_eq!(
            selected, original,
            "class admission must preserve its callback's exact error"
        );
        drop(selected_owner);
        drop(callback_slots);
        drop(original_owner);

        refcount::Py_DECREF(high);
        refcount::Py_DECREF(low);
        assert!(errors::PyErr_Occurred().is_null());
    }
}
