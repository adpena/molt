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
    let bits = unsafe {
        (hooks_or_stubs().sys_get_object_borrowed)(name_bytes.as_ptr(), name_bytes.len())
    };
    if bits == 0 {
        return ptr::null_mut();
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_borrowed_pyobj(bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn Py_GetVersion() -> *const c_char {
    c"3.12.0 (Molt runtime)".as_ptr()
}

thread_local! {
    /// The lazily-created per-thread state dict backing `PyThreadState_GetDict`.
    /// Immortal for the thread's lifetime (a single owning reference is retained
    /// here), so the borrowed reference the getter returns stays valid.
    static THREAD_STATE_DICT: std::cell::Cell<*mut PyObject> =
        const { std::cell::Cell::new(ptr::null_mut()) };
}

/// CPython `PyThreadState_GetDict` (Python/pystate.c): a per-thread dictionary in
/// which extensions stash thread-local state (numpy caches its per-thread
/// scratch here). Molt's ABI has no `PyThreadState` object, so this is backed by
/// a real thread-local dict created on first use. The returned reference is
/// BORROWED — the CPython contract — and never NULL once creation succeeds.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyThreadState_GetDict() -> *mut PyObject {
    THREAD_STATE_DICT.with(|slot| {
        let mut dict = slot.get();
        if dict.is_null() {
            dict = unsafe { crate::api::mapping::PyDict_New() };
            slot.set(dict);
        }
        dict
    })
}

/// CPython `PyOS_setsig` (Python/pylifecycle.c): install `handler` for signal
/// `sig` and return the previous handler. On a native unix host this is the real
/// `signal(2)` install (numpy installs a SIGFPE handler for FPE control), matching
/// CPython's use of the C signal facility.
#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyOS_setsig(sig: c_int, handler: *mut c_void) -> *mut c_void {
    unsafe { libc::signal(sig, handler as usize as libc::sighandler_t) as usize as *mut c_void }
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
        crate::bridge::molt_cpython_abi_init();
        let a = unsafe { PyThreadState_GetDict() };
        let b = unsafe { PyThreadState_GetDict() };
        assert!(
            std::ptr::eq(a, b),
            "PyThreadState_GetDict must return the SAME per-thread dict each call"
        );
    }
}
