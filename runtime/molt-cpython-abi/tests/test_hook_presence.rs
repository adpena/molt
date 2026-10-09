//! Installed failure and capability absence differ even when codegen merges
//! identical rejecting callback bodies. Exercise public constructor boundaries.
mod support;
use molt_cpython_abi::hooks::OwnedHandleResult;
use molt_cpython_abi::{
    abi_types::*,
    api::{errors, numbers, object, refcount},
    bridge,
};
use std::ptr;
unsafe extern "C" fn reject_cfunction(
    _: u64,
    _: i32,
    _: u64,
    _: bool,
    _: u64,
    _: *const u8,
    _: usize,
) -> u64 {
    0
}
unsafe extern "C" fn reject_method(_: u64, _: u64) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn reject_numeric(_: u64) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn target(_: *mut PyObject, _: *mut PyObject) -> *mut PyObject {
    unsafe { object::Py_NewRef(&raw mut Py_None) }
}
#[test]
fn installed_stub_equivalent_producers_never_select_physical_fallback() {
    let mut hooks = support::stub_runtime_hooks();
    support::fake_runtime::wire(&mut hooks);
    hooks.register_c_function = Some(reject_cfunction);
    hooks.method_new = Some(reject_method);
    hooks.numeric_identity_new = Some(reject_numeric);
    let _abi_test = support::enter_runtime_class_abi_test(hooks);
    unsafe {
        let mut definition = PyMethodDef {
            ml_name: c"rejected".as_ptr(),
            ml_meth: Some(target),
            ml_flags: METH_NOARGS,
            ml_doc: ptr::null(),
        };
        assert!(object::PyCFunction_New(&raw mut definition, ptr::null_mut()).is_null());
        assert_eq!(
            errors::PyErr_Occurred(),
            (&raw mut PyExc_SystemError).cast()
        );
        errors::PyErr_Clear();
        let function = numbers::PyLong_FromLong(7);
        let receiver = numbers::PyLong_FromLong(8);
        assert!(object::PyMethod_New(function, receiver).is_null());
        assert_eq!(
            errors::PyErr_Occurred(),
            (&raw mut PyExc_SystemError).cast()
        );
        errors::PyErr_Clear();
        let number = numbers::PyLong_FromLong(1000);
        assert!(
            !number.is_null(),
            "physical construction requires no runtime identity"
        );
        assert_eq!(bridge::molt_capi_pyobj_to_handle(number), 0);
        assert_eq!(
            errors::PyErr_Occurred(),
            (&raw mut PyExc_SystemError).cast()
        );
        assert_eq!((*number).ob_refcnt, 1);
        assert_eq!(numbers::PyLong_AsLong(number), 1000);
        errors::PyErr_Clear();
        refcount::Py_DECREF(number);
    }
}
