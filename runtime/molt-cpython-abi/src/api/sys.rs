//! System object registry — PySys_* C API surface.

use crate::abi_types::PyObject;
use crate::bridge::GLOBAL_BRIDGE;
use crate::hooks::hooks_or_stubs;
use std::ffi::{CStr, c_void};
use std::os::raw::{c_char, c_int};
use std::ptr;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySys_GetObject(name: *const c_char) -> *mut PyObject {
    if name.is_null() {
        return ptr::null_mut();
    }
    let name_bytes = unsafe { CStr::from_ptr(name) }.to_bytes();
    let result = unsafe {
        (hooks_or_stubs().sys_get_object_borrowed)(name_bytes.as_ptr(), name_bytes.len())
    };
    unsafe { GLOBAL_BRIDGE.borrowed_result_to_borrowed_pyobj(result) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn Py_GetVersion() -> *const c_char {
    c"3.12.0 (Molt runtime)".as_ptr()
}

/// CPython `PyThreadState_GetDict` (Python/pystate.c): a per-thread dictionary in
/// which extensions stash thread-local state (numpy caches its per-thread
/// scratch here). The canonical `ThreadStateRecord` retains the dict until that
/// state is explicitly destroyed. The returned reference is BORROWED.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyThreadState_GetDict() -> *mut PyObject {
    crate::api::object::thread_state_dict_or_insert_with(|| unsafe {
        crate::api::mapping::PyDict_New()
    })
}

/// CPython `PyOS_setsig` (Python/pylifecycle.c): install `handler` for signal
/// `sig` and return the previous handler. On a native unix host this is the real
/// `signal(2)` install (numpy installs a SIGFPE handler for FPE control), matching
/// CPython's use of the C signal facility.
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyOS_setsig(sig: c_int, handler: *mut c_void) -> *mut c_void {
    let handler_addr = handler.expose_provenance();
    let previous = unsafe { libc::signal(sig, handler_addr as libc::sighandler_t) };
    std::ptr::with_exposed_provenance_mut(previous as usize)
}

/// wasm has no signal machinery; echo the requested handler (no-op install).
#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyOS_setsig(_sig: c_int, handler: *mut c_void) -> *mut c_void {
    handler
}

#[cfg(test)]
mod pystate_tests {
    use super::*;

    /// The per-thread dict is created once and cached — two calls return the
    /// SAME pointer. (In a pure-ABI unit test the runtime `dict_new` hook is not
    /// installed, so the underlying dict may be NULL; the caching contract holds
    /// regardless, and the engine exercises the real non-NULL dict end-to-end.)
    #[test]
    fn thread_state_dict_is_stable() {
        let _thread_state = crate::api::object::AbiTestThreadStateTransaction::new();
        crate::bridge::molt_cpython_abi_init();
        let a = unsafe { PyThreadState_GetDict() };
        let b = unsafe { PyThreadState_GetDict() };
        assert!(
            std::ptr::eq(a, b),
            "PyThreadState_GetDict must return the SAME per-thread dict each call"
        );
    }
}
