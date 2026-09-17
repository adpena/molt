//! A installed callable producer's failure never switches ownership backends.
mod support;

use molt_cpython_abi::abi_types::*;
use molt_cpython_abi::api::{errors, object};
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};

static CALLS: AtomicUsize = AtomicUsize::new(0);
unsafe extern "C" fn reject_registration(
    _target: u64,
    _flags: i32,
    _self_bits: u64,
    _self_is_null: bool,
    _class_bits: u64,
    _name: *const u8,
    _len: usize,
) -> u64 {
    CALLS.fetch_add(1, Ordering::Relaxed);
    0
}
unsafe extern "C" fn target(_self: *mut PyObject, _arg: *mut PyObject) -> *mut PyObject {
    unsafe { object::Py_NewRef(&raw mut Py_None) }
}

#[test]
fn cfunction_registered_construction_failure_has_no_raw_callable_fallback() {
    let mut hooks = support::stub_runtime_hooks();
    support::fake_strings::wire(&mut hooks);
    hooks.register_c_function = reject_registration;
    support::prepare_abi_test_thread(hooks);
    let mut definition = PyMethodDef {
        ml_name: c"rejected".as_ptr(),
        ml_meth: Some(target),
        ml_flags: METH_NOARGS,
        ml_doc: ptr::null(),
    };
    unsafe {
        let callable = object::PyCFunction_New(&raw mut definition, ptr::null_mut());
        assert!(callable.is_null());
        assert_eq!(CALLS.load(Ordering::Relaxed), 1);
        assert_eq!(
            errors::PyErr_Occurred(),
            (&raw mut PyExc_SystemError).cast()
        );
        errors::PyErr_Clear();
    }
}
