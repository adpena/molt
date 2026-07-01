//! CPython eval C-API surface.

use crate::abi_types::PyObject;
use once_cell::sync::OnceCell;

static BUILTINS_DICT: OnceCell<usize> = OnceCell::new();

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyEval_GetBuiltins() -> *mut PyObject {
    let raw = BUILTINS_DICT.get_or_init(|| unsafe { crate::api::mapping::PyDict_New() as usize });
    *raw as *mut PyObject
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyEval_EvalCode(
    _co: *mut PyObject,
    _globals: *mut PyObject,
    _locals: *mut PyObject,
) -> *mut PyObject {
    unsafe {
        crate::api::errors::PyErr_SetString(
            &raw mut crate::abi_types::PyExc_NotImplementedError,
            c"PyEval_EvalCode is not available in Molt static extension ABI".as_ptr(),
        );
    }
    std::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _Py_IsFinalizing() -> std::os::raw::c_int {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn Py_IsFinalizing() -> std::os::raw::c_int {
    0
}
