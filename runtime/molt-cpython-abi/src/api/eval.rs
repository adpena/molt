//! CPython eval C-API surface.

use crate::abi_types::PyObject;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyEval_GetBuiltins() -> *mut PyObject {
    // CPython returns the interpreter's REAL `builtins` module dict (the
    // frame's `f_builtins`, normally `sys.modules['builtins'].__dict__`). The
    // previous body lazily created a fresh, permanently-empty `PyDict` in a
    // detached `OnceCell` — never populated by anything — so every
    // `PyDict_GetItemString(PyEval_GetBuiltins(), name)` lookup a C extension
    // makes (the common way to reach e.g. `len`/`print`/`Exception` from C)
    // would silently miss. Route through the runtime's current-frame/default
    // builtins hook: no import re-entry and no second builtins namespace.
    let Some(h) = crate::hooks::hooks() else {
        crate::api::imports::propagate_hook_error(
            c"builtins lookup is unavailable without runtime hooks",
        );
        return std::ptr::null_mut();
    };
    match crate::api::imports::decode_borrowed_hook_result(unsafe {
        (h.eval_get_builtins_borrowed)()
    }) {
        crate::hooks::DecodedHandleResult::Ok(bits) => unsafe {
            crate::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits)
        },
        crate::hooks::DecodedHandleResult::Missing => {
            crate::api::imports::propagate_hook_error(
                c"runtime has no builtins dictionary for the current context",
            );
            std::ptr::null_mut()
        }
        crate::hooks::DecodedHandleResult::Error => {
            crate::api::imports::propagate_hook_error(
                c"builtins lookup failed without setting an exception",
            );
            std::ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyEval_EvalCode(
    _co: *mut PyObject,
    _globals: *mut PyObject,
    _locals: *mut PyObject,
) -> *mut PyObject {
    unsafe {
        crate::api::errors::PyErr_SetString(
            (&raw mut crate::abi_types::PyExc_NotImplementedError)
                .cast::<crate::abi_types::PyObject>(),
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
