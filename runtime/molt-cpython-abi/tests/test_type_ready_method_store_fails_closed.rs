//! Mask-proof regression for POISON Lane A #2 — `add_methods_to_dict` fail-open.
//!
//! `PyType_Ready` populates `tp_dict` from `tp_methods` via
//! `add_methods_to_dict`: for each method it builds a `PyCFunction` and calls
//! `PyDict_SetItemString(dict, name, func)`. The bug: on a `PyDict_SetItemString`
//! failure it recorded a silent failure but returned READY(0) with the method
//! SILENTLY DROPPED from `tp_dict` — inverse of the sibling `add_members_` /
//! `add_getset_` paths (which return -1). A numpy scalar/DType type could be
//! marked ready while missing methods, surfacing much later as an
//! `AttributeError` / wrong dispatch with no exec-time failure.
//!
//! CPython's add_methods propagates a dictionary-store error. The fixture
//! supplies normal native-callable crossing and dictionary ownership, then
//! rejects the declared "reduce" entry in the dict_mutate hook. An attempt counter
//! proves readiness failed at that store, rather than at missing fake runtime
//! capabilities before method publication.

#![allow(non_snake_case)]

mod support;

use molt_cpython_abi::abi_types::*;
use molt_cpython_abi::hooks::RuntimeHooks;
use std::os::raw::c_char;
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};

static METHOD_STORE_FAILURES: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn reject_method_store(
    dict: u64,
    key: u64,
    value: u64,
    delete: u8,
    publish: Option<unsafe extern "C" fn(*mut std::ffi::c_void) -> i32>,
    context: *mut std::ffi::c_void,
) -> i32 {
    let mut len = 0;
    let bytes = unsafe { support::fake_runtime::str_data(key, &mut len) };
    if delete == 0
        && !bytes.is_null()
        && unsafe { std::slice::from_raw_parts(bytes, len) } == b"reduce"
    {
        METHOD_STORE_FAILURES.fetch_add(1, Ordering::Relaxed);
        unsafe {
            molt_cpython_abi::api::errors::PyErr_SetString(
                (&raw mut PyExc_RuntimeError).cast(),
                c"fixture method store rejected".as_ptr(),
            );
        }
        -1
    } else {
        unsafe { support::fake_runtime::dict_mutate(dict, key, value, delete, publish, context) }
    }
}

fn install_hooks_with_rejected_method_store() {
    let mut hooks: RuntimeHooks = molt_cpython_abi::hooks::STUB_HOOKS;
    support::fake_runtime::wire(&mut hooks);
    hooks.dict_mutate = reject_method_store;
    support::prepare_runtime_class_abi_test_thread(hooks);
    METHOD_STORE_FAILURES.store(0, Ordering::Relaxed);
}

unsafe extern "C" fn dummy_method(_self: *mut PyObject, _args: *mut PyObject) -> *mut PyObject {
    ptr::null_mut()
}

fn method_def(name: &'static [u8]) -> PyMethodDef {
    PyMethodDef {
        ml_name: name.as_ptr() as *const c_char,
        ml_meth: Some(dummy_method),
        ml_flags: METH_VARARGS,
        ml_doc: ptr::null(),
    }
}

#[test]
fn type_ready_fails_closed_when_method_store_fails() {
    install_hooks_with_rejected_method_store();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    let mut methods = [
        method_def(b"reduce\0"),
        PyMethodDef {
            ml_name: ptr::null(),
            ml_meth: None,
            ml_flags: 0,
            ml_doc: ptr::null(),
        },
    ];
    let mut tp = support::StaticType::new();
    tp.tp_name = c"scalar_store_fail".as_ptr();
    tp.tp_basicsize = std::mem::size_of::<PyObject>() as Py_ssize_t;
    tp.tp_methods = methods.as_mut_ptr();

    let rc = unsafe { molt_cpython_abi::api::typeobj::PyType_Ready(tp.as_ptr()) };
    assert_eq!(
        METHOD_STORE_FAILURES.load(Ordering::Relaxed),
        1,
        "readiness must reach the declared method store"
    );

    // The whole point of the fix: a method that cannot be stored in tp_dict must
    // FAIL PyType_Ready, not leave a "ready" type with a silently-dropped method.
    assert_eq!(
        rc, -1,
        "PyType_Ready must fail closed when a tp_methods entry cannot be stored \
         in tp_dict (pre-fix returned 0 with the method silently dropped)"
    );
    let pending = unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() };
    assert!(
        !pending.is_null(),
        "a method-store failure must leave a pending exception (never a contentless -1)"
    );
    // The type must NOT be advertised as READY when a declared method was dropped.
    assert_eq!(
        tp.tp_flags & Py_TPFLAGS_READY,
        0,
        "a type whose method population failed must not be marked READY"
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

struct PublicationObservation {
    dict: u64,
    key: u64,
    displaced: u64,
    expected: Option<u64>,
    calls: usize,
    committed: bool,
    owners_alive: bool,
    reentered: bool,
    fail: bool,
    error_value: usize,
}

unsafe extern "C" fn observe_publication(context: *mut std::ffi::c_void) -> i32 {
    use molt_cpython_abi::hooks::{DecodedHandleResult, DictHashSource};
    let observation = unsafe { &mut *context.cast::<PublicationObservation>() };
    let hooks = molt_cpython_abi::hooks::hooks_or_stubs();
    observation.calls += 1;
    observation.committed = match unsafe {
        (hooks.dict_get)(
            observation.dict,
            observation.key,
            DictHashSource::Compute,
            0,
        )
    }
    .decode()
    {
        DecodedHandleResult::Ok(value) => observation.expected == Some(value),
        DecodedHandleResult::Missing => observation.expected.is_none(),
        DecodedHandleResult::Error => false,
    };
    observation.owners_alive = unsafe {
        hooks.ref_count(observation.key) > 0 && hooks.ref_count(observation.displaced) == 1
    };
    // Mutating this same dictionary in the callback also proves that the
    // fixture has released its storage lock before publishing.
    observation.reentered = unsafe {
        (hooks.dict_mutate)(
            observation.dict,
            molt_lang_obj_model::MoltObject::from_int(91).bits(),
            molt_lang_obj_model::MoltObject::from_int(92).bits(),
            0,
            None,
            ptr::null_mut(),
        ) == 0
    };
    if observation.fail {
        unsafe {
            molt_cpython_abi::api::errors::PyErr_SetString(
                (&raw mut PyExc_RuntimeError).cast(),
                c"publication rejected".as_ptr(),
            );
        }
        let error = molt_cpython_abi::api::errors::take_current_error().unwrap();
        observation.error_value = error.value.addr();
        molt_cpython_abi::api::errors::restore_current_error_exact(error);
        -1
    } else {
        0
    }
}

#[test]
fn dict_mutation_publishes_committed_storage_before_retiring_owners() {
    use molt_cpython_abi::hooks::DecodedHandleResult;
    install_hooks_with_rejected_method_store();
    let hooks = molt_cpython_abi::hooks::hooks_or_stubs();
    unsafe {
        molt_cpython_abi::api::errors::PyErr_Clear();
        let dict = (hooks.alloc_dict)();
        let key = support::fake_runtime::fresh_handle();
        let old = support::fake_runtime::fresh_handle();
        let new = support::fake_runtime::fresh_handle();
        assert_eq!(
            (hooks.dict_mutate)(dict, key, old, 0, None, ptr::null_mut()),
            0
        );
        (hooks.dec_ref)(key);
        (hooks.dec_ref)(old);
        let mut observation = PublicationObservation {
            dict,
            key,
            displaced: old,
            expected: Some(new),
            calls: 0,
            committed: false,
            owners_alive: false,
            reentered: false,
            fail: false,
            error_value: 0,
        };
        // Drop the caller's new owner only after the transaction has borrowed it.
        assert_eq!(
            (hooks.dict_mutate)(
                dict,
                key,
                new,
                0,
                Some(observe_publication),
                (&raw mut observation).cast()
            ),
            0
        );
        assert!(observation.committed && observation.owners_alive && observation.reentered);
        assert_eq!(observation.calls, 1);
        assert!(
            !support::fake_runtime::contains(old),
            "old owner retires after publication"
        );
        (hooks.dec_ref)(new);
        observation.displaced = new;
        observation.expected = None;
        observation.fail = true;
        assert_eq!(
            (hooks.dict_mutate)(
                dict,
                key,
                0,
                1,
                Some(observe_publication),
                (&raw mut observation).cast()
            ),
            -1
        );
        assert!(observation.committed && observation.owners_alive && observation.reentered);
        assert_eq!(observation.calls, 2);
        assert!(!support::fake_runtime::contains(key));
        assert!(!support::fake_runtime::contains(new));
        let error = molt_cpython_abi::api::errors::take_current_error().unwrap();
        assert_eq!(error.exc_type, (&raw mut PyExc_RuntimeError).cast());
        assert_eq!(error.value.addr(), observation.error_value);
        drop(error);
        assert_eq!(
            (hooks.dict_mutate)(
                dict,
                key,
                0,
                1,
                Some(observe_publication),
                (&raw mut observation).cast()
            ),
            1
        );
        assert_eq!(observation.calls, 2, "absent deletion does not publish");

        // Same-object keys/values retain distinct edges, including replacement
        // and pop's transfer of the removed value owner.
        let alias = support::fake_runtime::fresh_handle();
        for _ in 0..2 {
            assert_eq!(
                (hooks.dict_mutate)(dict, alias, alias, 0, None, ptr::null_mut()),
                0
            );
            assert_eq!(hooks.ref_count(alias), 3);
        }
        (hooks.dec_ref)(alias);
        assert!(
            matches!((hooks.dict_pop)(dict, alias).decode(), DecodedHandleResult::Ok(value) if value == alias)
        );
        assert_eq!(hooks.ref_count(alias), 1);
        (hooks.dec_ref)(alias);
        assert!(!support::fake_runtime::contains(alias));
        assert!(matches!(
            (hooks.dict_pop)(dict, alias).decode(),
            DecodedHandleResult::Missing
        ));
        (hooks.dec_ref)(dict);
        assert!(molt_cpython_abi::api::errors::PyErr_Occurred().is_null());
    }
}
