//! Reference counting — Py_INCREF / Py_DECREF and variants.
//!
//! In a pure CPython world these are hot inlined macros. In our bridge they
//! are real functions because C extensions call them via PLT. We keep them as
//! `#[inline(always)]` to give the compiler maximum optimisation latitude when
//! the bridge itself calls them.
//!
//! Canonical managed ABI views have one lifecycle authority. The view owns one
//! runtime hold; the runtime owner contributes a C-visible bias while borrowed
//! access is valid. When runtime owners drain, that bias is detached and any
//! direct CPython C references keep the same view and runtime object alive.

use crate::abi_types::PyObject;
use std::ptr;

/// Increment the reference count.
///
/// # Safety
/// `op` must be a non-null bridge-managed PyObject.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Py_INCREF(op: *mut PyObject) {
    if op.is_null() {
        return;
    }
    let managed_bits = crate::bridge::GLOBAL_BRIDGE.managed_handle_for_pyobj(op);
    // Managed `ob_refcnt` and bridge lifecycle state are one transaction. Pin
    // the runtime before touching either; foreign C objects retain CPython's
    // caller-held-GIL contract and pay no Molt guard cost.
    let _runtime_gil = managed_bits.map(|_| crate::hooks::RuntimeGilGuard::ensure());
    if let Some(bits) = managed_bits {
        if unsafe { (crate::hooks::hooks_or_stubs().try_mark_abi_view)(bits, 1) } == 0 {
            eprintln!("molt fatal: Py_INCREF attempted after managed object terminal death");
            std::process::abort();
        }
    }
    // Immortal check via the single authority (mirrors CPython _Py_IsImmortal):
    // a static singleton is never incremented.
    unsafe {
        let rc = (*op).ob_refcnt;
        if !crate::abi_types::is_immortal_refcnt(rc) {
            (*op).ob_refcnt = rc.wrapping_add(1);
        }
    }
}

/// Decrement the reference count and its matching runtime ownership edge.
///
/// # Safety
/// `op` must be a non-null bridge-managed PyObject.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Py_DECREF(op: *mut PyObject) {
    if op.is_null() {
        return;
    }
    let managed_bits = crate::bridge::GLOBAL_BRIDGE.managed_handle_for_pyobj(op);
    let _runtime_gil = managed_bits.map(|_| crate::hooks::RuntimeGilGuard::ensure());
    unsafe {
        let rc = (*op).ob_refcnt;
        if crate::abi_types::is_immortal_refcnt(rc) {
            return; // immortal singleton — permanent no-op, never freed
        }
        if rc <= 0 {
            return;
        }
        let new_rc = rc - 1;
        (*op).ob_refcnt = new_rc;
        if new_rc == 0 {
            if let Some(bits) = managed_bits {
                if crate::bridge::GLOBAL_BRIDGE.c_ref_zero(bits) {
                    (crate::hooks::hooks_or_stubs().dec_ref)(bits);
                }
                return;
            }
            let released_registered_object = crate::bridge::GLOBAL_BRIDGE.release_pyobj(op);
            if !released_registered_object {
                let tp = (*op).ob_type;
                if !tp.is_null()
                    && let Some(dealloc) = (*tp).tp_dealloc
                {
                    dealloc(op);
                }
            }
        }
    }
}

/// CPython private `_Py_Dealloc` (Objects/object.c): the object finalizer that a
/// C extension's `Py_DECREF` macro tail-calls the moment a refcount reaches
/// zero. numpy links it directly. This mirrors the zero-refcount branch of
/// [`Py_DECREF`]: transfer a molt-owned canonical view into the runtime's
/// terminal/finalization path, which retires bridge identity only after the
/// resurrection window, or invoke a foreign C type's own `tp_dealloc`.
///
/// # Safety
/// `op` must be a valid PyObject whose refcount has already reached zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _Py_Dealloc(op: *mut PyObject) {
    if op.is_null() {
        return;
    }
    let managed_bits = crate::bridge::GLOBAL_BRIDGE.managed_handle_for_pyobj(op);
    let _runtime_gil = managed_bits.map(|_| crate::hooks::RuntimeGilGuard::ensure());
    unsafe {
        if let Some(bits) = managed_bits {
            if crate::bridge::GLOBAL_BRIDGE.c_ref_zero(bits) {
                (crate::hooks::hooks_or_stubs().dec_ref)(bits);
            }
        } else if !crate::bridge::GLOBAL_BRIDGE.release_pyobj(op) {
            let tp = (*op).ob_type;
            if !tp.is_null()
                && let Some(dealloc) = (*tp).tp_dealloc
            {
                dealloc(op);
            }
        }
    }
}

/// `Py_INCREF` that accepts null (null is silently ignored).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Py_XINCREF(op: *mut PyObject) {
    if !op.is_null() {
        unsafe { Py_INCREF(op) };
    }
}

/// `Py_DECREF` that accepts null (null is silently ignored).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Py_XDECREF(op: *mut PyObject) {
    if !op.is_null() {
        unsafe { Py_DECREF(op) };
    }
}

/// Clear a `*mut PyObject` pointer: Py_XDECREF + set to NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Py_CLEAR(op: *mut *mut PyObject) {
    if op.is_null() {
        return;
    }
    unsafe {
        let tmp = *op;
        if !tmp.is_null() {
            *op = ptr::null_mut();
            Py_DECREF(tmp);
        }
    }
}
