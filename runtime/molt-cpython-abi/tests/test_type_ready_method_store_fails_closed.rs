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
//! rejects the declared "reduce" entry in the dict_set hook. An attempt counter
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

unsafe extern "C" fn reject_method_store(dict: u64, key: u64, value: u64) -> i32 {
    let mut len = 0;
    let bytes = unsafe { support::fake_runtime::str_data(key, &mut len) };
    if !bytes.is_null() && unsafe { std::slice::from_raw_parts(bytes, len) } == b"reduce" {
        METHOD_STORE_FAILURES.fetch_add(1, Ordering::Relaxed);
        unsafe {
            molt_cpython_abi::api::errors::PyErr_SetString(
                (&raw mut PyExc_RuntimeError).cast(),
                c"fixture method store rejected".as_ptr(),
            );
        }
        -1
    } else {
        unsafe { support::fake_runtime::dict_set(dict, key, value) }
    }
}

fn install_hooks_with_rejected_method_store() {
    let mut hooks: RuntimeHooks = molt_cpython_abi::hooks::STUB_HOOKS;
    support::fake_runtime::wire(&mut hooks);
    hooks.dict_set = reject_method_store;
    support::prepare_abi_test_thread(hooks);
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
