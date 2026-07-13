//! CPython context variable C-API surface, faithful to `Python/context.c`.
//!
//! Bindings live in a CURRENT-CONTEXT mapping keyed by the `ContextVar`
//! object (not in a mutable field on the var itself), matching CPython's
//! per-context HAMT model. Until a `copy_context()`/`Context.run()` surface
//! exists there is exactly one context per thread, so a thread-local map IS
//! the current context — Set in one thread no longer leaks into others.
//! `PyContextVar_Set` returns a real reset token consumed by
//! `PyContextVar_Reset`, never the previous value itself.

use crate::abi_types::{PyContextVarObject, PyObject, PyTypeObject};
use std::cell::RefCell;
use std::collections::HashMap;
use std::os::raw::c_int;
use std::ptr;

thread_local! {
    /// The current context: var pointer -> owned (incref'd) value pointer.
    static CURRENT_CONTEXT: RefCell<HashMap<usize, usize>> = RefCell::new(HashMap::new());
}

// ── Reset token object ───────────────────────────────────────────────────────
// CPython's PyContextToken wraps (var, old value | MISSING, used flag).

#[repr(C)]
struct ContextTokenObject {
    ob_base: PyObject,
    var: *mut PyObject,
    /// Previous value (owned) or NULL for CPython's `Token.MISSING`.
    old_value: *mut PyObject,
    used: bool,
}

fn token_type() -> *mut PyTypeObject {
    static TOKEN_TYPE: once_cell::sync::Lazy<usize> = once_cell::sync::Lazy::new(|| {
        let mut ty: Box<PyTypeObject> = Box::new(unsafe { std::mem::zeroed() });
        ty.tp_name = c"Token".as_ptr();
        ty.ob_base.ob_base.ob_type = &raw mut crate::abi_types::PyType_Type;
        ty.ob_base.ob_base.ob_refcnt = 1;
        Box::into_raw(ty) as usize
    });
    *TOKEN_TYPE as *mut PyTypeObject
}

unsafe fn token_object(op: *mut PyObject) -> Option<*mut ContextTokenObject> {
    if op.is_null() || !std::ptr::eq(unsafe { (*op).ob_type }, token_type()) {
        return None;
    }
    Some(op.cast::<ContextTokenObject>())
}

/// Deallocator for token objects (wired through the refcount drop path when
/// the bridge sees a raw C object; tokens are short-lived C-side objects).
pub unsafe extern "C" fn molt_context_token_dealloc(op: *mut PyObject) {
    let Some(token) = (unsafe { token_object(op) }) else {
        return;
    };
    unsafe {
        crate::api::refcount::Py_XDECREF((*token).var);
        crate::api::refcount::Py_XDECREF((*token).old_value);
        drop(Box::from_raw(token));
    }
}

unsafe fn is_contextvar(var: *mut PyObject) -> bool {
    !var.is_null()
        && std::ptr::eq(
            unsafe { (*var).ob_type },
            &raw mut crate::abi_types::PyContextVar_Type,
        )
}

/// CPython `ENSURE_ContextVar`: any non-exact input is a TypeError — the C API
/// never duck-types through Python-level `get`/`set` attributes.
unsafe fn ensure_contextvar(var: *mut PyObject) -> bool {
    if unsafe { is_contextvar(var) } {
        return true;
    }
    unsafe {
        crate::api::errors::PyErr_SetString(
            (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
            c"an instance of ContextVar was expected".as_ptr(),
        );
    }
    false
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyContextVar_New(
    name: *const std::os::raw::c_char,
    default_value: *mut PyObject,
) -> *mut PyObject {
    if name.is_null() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
                c"context variable name must not be NULL".as_ptr(),
            );
        }
        return ptr::null_mut();
    }
    // NOTE: CPython imposes NO non-empty-name constraint — ContextVar('') is
    // legal (Python/context.c contextvar_new); the previous ValueError here was
    // an invented restriction.
    let name_obj = unsafe { crate::api::strings::PyUnicode_FromString(name) };
    if name_obj.is_null() {
        return ptr::null_mut();
    }
    if !default_value.is_null() {
        unsafe { crate::api::refcount::Py_INCREF(default_value) };
    }
    let obj = Box::new(PyContextVarObject {
        ob_base: PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut crate::abi_types::PyContextVar_Type,
        },
        name: name_obj,
        default_value,
        // Legacy field retained for ABI layout; bindings live in the
        // current-context map, never here.
        current_value: ptr::null_mut(),
    });
    Box::into_raw(obj).cast::<PyObject>()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyContextVar_Get(
    var: *mut PyObject,
    default_value: *mut PyObject,
    value: *mut *mut PyObject,
) -> c_int {
    if var.is_null() || value.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return -1;
    }
    if !unsafe { ensure_contextvar(var) } {
        return -1;
    }
    // Current-context binding.
    let bound = CURRENT_CONTEXT.with(|ctx| ctx.borrow().get(&(var as usize)).copied());
    if let Some(bound) = bound {
        let bound = bound as *mut PyObject;
        unsafe {
            crate::api::refcount::Py_INCREF(bound);
            *value = bound;
        }
        return 0;
    }
    // Python/context.c: the CALLER's def argument wins unconditionally; the
    // var's own default is consulted only when def == NULL.
    let context_var = var.cast::<PyContextVarObject>();
    let candidate = if !default_value.is_null() {
        default_value
    } else {
        unsafe { (*context_var).default_value }
    };
    if candidate.is_null() {
        // No value, no default: *value = NULL and SUCCESS (0) with no
        // exception — only Python-level ContextVar.get() raises LookupError,
        // never the C API.
        unsafe { *value = ptr::null_mut() };
        return 0;
    }
    unsafe {
        crate::api::refcount::Py_INCREF(candidate);
        *value = candidate;
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyContextVar_Set(
    var: *mut PyObject,
    value: *mut PyObject,
) -> *mut PyObject {
    if var.is_null() || value.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    if !unsafe { ensure_contextvar(var) } {
        return ptr::null_mut();
    }
    // Store the binding in the CURRENT CONTEXT (owned reference), capturing the
    // previous binding for the token.
    unsafe { crate::api::refcount::Py_INCREF(value) };
    let previous = CURRENT_CONTEXT.with(|ctx| {
        ctx.borrow_mut()
            .insert(var as usize, value as usize)
            .map(|old| old as *mut PyObject)
    });
    // Mint a real reset token wrapping (var, old-or-MISSING). The token owns
    // both references; `previous` ownership transfers from the map.
    unsafe { crate::api::refcount::Py_INCREF(var) };
    let token = Box::new(ContextTokenObject {
        ob_base: PyObject {
            ob_refcnt: 1,
            ob_type: token_type(),
        },
        var,
        old_value: previous.unwrap_or(ptr::null_mut()),
        used: false,
    });
    Box::into_raw(token).cast::<PyObject>()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyContextVar_Reset(var: *mut PyObject, token: *mut PyObject) -> c_int {
    if var.is_null() || token.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return -1;
    }
    if !unsafe { ensure_contextvar(var) } {
        return -1;
    }
    let Some(token) = (unsafe { token_object(token) }) else {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
                c"an instance of Token was expected".as_ptr(),
            );
        }
        return -1;
    };
    if unsafe { (*token).used } {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_RuntimeError)
                    .cast::<crate::abi_types::PyObject>(),
                c"Token has already been used once".as_ptr(),
            );
        }
        return -1;
    }
    if !std::ptr::eq(unsafe { (*token).var }, var) {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_ValueError).cast::<crate::abi_types::PyObject>(),
                c"Token was created by a different ContextVar".as_ptr(),
            );
        }
        return -1;
    }
    let old_value = unsafe { (*token).old_value };
    let displaced = CURRENT_CONTEXT.with(|ctx| {
        let mut ctx = ctx.borrow_mut();
        if old_value.is_null() {
            // Token.MISSING: the var had no binding before Set — remove it.
            ctx.remove(&(var as usize)).map(|v| v as *mut PyObject)
        } else {
            // Restore the previous binding (the map takes a new owned ref).
            unsafe { crate::api::refcount::Py_INCREF(old_value) };
            ctx.insert(var as usize, old_value as usize)
                .map(|v| v as *mut PyObject)
        }
    });
    if let Some(displaced) = displaced {
        unsafe { crate::api::refcount::Py_DECREF(displaced) };
    }
    unsafe { (*token).used = true };
    0
}

pub unsafe extern "C" fn molt_contextvar_dealloc(op: *mut PyObject) {
    if op.is_null() {
        return;
    }
    // Drop any current-context binding for this var (the map key is the var
    // pointer; a dangling key would leak the bound value).
    let bound = CURRENT_CONTEXT.with(|ctx| {
        ctx.borrow_mut()
            .remove(&(op as usize))
            .map(|v| v as *mut PyObject)
    });
    if let Some(bound) = bound {
        unsafe { crate::api::refcount::Py_DECREF(bound) };
    }
    let obj = op.cast::<PyContextVarObject>();
    unsafe {
        crate::api::refcount::Py_XDECREF((*obj).name);
        crate::api::refcount::Py_XDECREF((*obj).default_value);
        crate::api::refcount::Py_XDECREF((*obj).current_value);
        drop(Box::from_raw(obj));
    }
}
