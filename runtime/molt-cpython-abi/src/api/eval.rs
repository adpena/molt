//! CPython eval C-API surface.

use crate::abi_types::PyObject;
use once_cell::sync::OnceCell;

static BUILTINS_DICT: OnceCell<usize> = OnceCell::new();

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyEval_GetBuiltins() -> *mut PyObject {
    // CPython returns the interpreter's REAL `builtins` module dict (the
    // frame's `f_builtins`, normally `sys.modules['builtins'].__dict__`). The
    // previous body lazily created a fresh, permanently-empty `PyDict` in a
    // detached `OnceCell` — never populated by anything — so every
    // `PyDict_GetItemString(PyEval_GetBuiltins(), name)` lookup a C extension
    // makes (the common way to reach e.g. `len`/`print`/`Exception` from C)
    // would silently miss. Route through the real `import_module` hook (same
    // class of fix as `PyImport_GetModuleDict` <-> `sys.modules`) and fall
    // back to the detached dict only when hooks are genuinely unavailable.
    let builtins_module = unsafe {
        crate::api::imports::PyImport_ImportModule(c"builtins".as_ptr())
    };
    if !builtins_module.is_null() {
        let dict = unsafe { crate::api::modules::PyModule_GetDict(builtins_module) };
        unsafe { crate::api::refcount::Py_DECREF(builtins_module) };
        if !dict.is_null() {
            return dict;
        }
    }
    unsafe { crate::api::errors::PyErr_Clear() };
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
