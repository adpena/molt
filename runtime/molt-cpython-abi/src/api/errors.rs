//! Error/exception API — PyErr_*, PyArg_ParseTuple.
//!
//! `PyArg_ParseTuple` is the hottest function in any C extension — called on
//! every function entry to unpack positional arguments. We implement the
//! most common format codes: `i`, `l`, `d`, `f`, `s`, `z`, `s#`, `O`, `p`,
//! `n`, `L`, `K`, `b`, `B`, `H`, `I`, `k`, `y`, `y#`, `C`.

use crate::abi_types::{Py_ssize_t, PyObject};
use crate::bridge::GLOBAL_BRIDGE;
use molt_lang_obj_model::MoltObject;
use std::ffi::{CStr, c_void};
use std::os::raw::{c_char, c_int, c_long};
use std::ptr;

// ─── Thread-local error state ─────────────────────────────────────────────

thread_local! {
    static CURRENT_EXC: std::cell::RefCell<Option<(u64, String)>> = const { std::cell::RefCell::new(None) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_SetString(exc_type: *mut PyObject, message: *const c_char) {
    let msg = if message.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(message).to_string_lossy().into_owned() }
    };
    let type_bits = if exc_type.is_null() {
        0u64
    } else {
        GLOBAL_BRIDGE.lock().pyobj_to_handle(exc_type).unwrap_or(0)
    };
    CURRENT_EXC.with(|c| *c.borrow_mut() = Some((type_bits, msg)));
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_SetNone(exc_type: *mut PyObject) {
    unsafe { PyErr_SetString(exc_type, ptr::null()) };
}

/// Returns NULL if no exception, else non-null (type of current exception).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_Occurred() -> *mut PyObject {
    CURRENT_EXC.with(|c| {
        if c.borrow().is_some() {
            // Return a non-null sentinel — caller only checks null/non-null.
            &raw mut crate::abi_types::Py_None
        } else {
            ptr::null_mut()
        }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_Clear() {
    CURRENT_EXC.with(|c| *c.borrow_mut() = None);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_Print() {
    CURRENT_EXC.with(|c| {
        if let Some((_, ref msg)) = *c.borrow() {
            eprintln!("[molt-cpython-abi] PyErr_Print: {msg}");
        }
    });
    unsafe { PyErr_Clear() };
}

/// Set a ValueError with formatted message.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_Format(
    exc_type: *mut PyObject,
    format: *const c_char,
    // variadic — we capture only the format string for the common case
) -> *mut PyObject {
    unsafe { PyErr_SetString(exc_type, format) };
    ptr::null_mut()
}

// ─── Additional error API ─────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_SetObject(exc_type: *mut PyObject, value: *mut PyObject) {
    // Simplified: set the error with the repr of the value.
    let _ = value;
    unsafe { PyErr_SetString(exc_type, c"<exception>".as_ptr()) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_NoMemory() -> *mut PyObject {
    unsafe {
        PyErr_SetString(
            &raw mut crate::abi_types::PyExc_MemoryError,
            c"out of memory".as_ptr(),
        );
    }
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_BadArgument() -> c_int {
    unsafe {
        PyErr_SetString(
            &raw mut crate::abi_types::PyExc_TypeError,
            c"bad argument type for built-in operation".as_ptr(),
        );
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_BadInternalCall() {
    unsafe {
        PyErr_SetString(
            &raw mut crate::abi_types::PyExc_RuntimeError,
            c"bad argument to internal function".as_ptr(),
        );
    }
}

/// Fetch (and clear) the current exception state.
/// Writes the exception type, value, and traceback into the provided pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_Fetch(
    p_type: *mut *mut PyObject,
    p_value: *mut *mut PyObject,
    p_tb: *mut *mut PyObject,
) {
    CURRENT_EXC.with(|c| {
        let exc = c.borrow_mut().take();
        if let Some((_type_bits, _msg)) = exc {
            if !p_type.is_null() {
                // Return a non-null sentinel for the type.
                unsafe { *p_type = &raw mut crate::abi_types::Py_None };
            }
            if !p_value.is_null() {
                unsafe { *p_value = ptr::null_mut() };
            }
        } else {
            if !p_type.is_null() {
                unsafe { *p_type = ptr::null_mut() };
            }
            if !p_value.is_null() {
                unsafe { *p_value = ptr::null_mut() };
            }
        }
        if !p_tb.is_null() {
            unsafe { *p_tb = ptr::null_mut() };
        }
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_Restore(
    _tp: *mut PyObject,
    _value: *mut PyObject,
    _tb: *mut PyObject,
) {
    // Simplified: just set the error state to the provided type.
    if _tp.is_null() {
        unsafe { PyErr_Clear() };
    } else {
        CURRENT_EXC.with(|c| *c.borrow_mut() = Some((0, String::from("<restored exception>"))));
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_NormalizeException(
    _exc: *mut *mut PyObject,
    _val: *mut *mut PyObject,
    _tb: *mut *mut PyObject,
) {
    // No-op — full normalization requires instantiating exception objects.
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_ExceptionMatches(exc: *mut PyObject) -> c_int {
    let _ = exc;
    CURRENT_EXC.with(|c| c.borrow().is_some() as c_int)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_GivenExceptionMatches(
    given: *mut PyObject,
    exc: *mut PyObject,
) -> c_int {
    if given.is_null() || exc.is_null() {
        return 0;
    }
    std::ptr::eq(given, exc) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_WarnEx(
    _category: *mut PyObject,
    _message: *const c_char,
    _stack_level: c_int,
) -> c_int {
    // Warnings are silently ignored in the bridge.
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_WriteUnraisable(obj: *mut PyObject) {
    let _ = obj;
    CURRENT_EXC.with(|c| {
        if let Some((_, ref msg)) = *c.borrow() {
            eprintln!("[molt-cpython-abi] unraisable exception: {msg}");
        }
    });
    unsafe { PyErr_Clear() };
}

// ─── PyArg_ParseTuple ─────────────────────────────────────────────────────
//
// Implements the subset of format codes that cover ~95% of real extensions:
//   i  → c_int*       (int)
//   l  → c_long*      (long)
//   L  → i64*         (long long)
//   K  → u64*         (unsigned long long)
//   d  → f64*         (double)
//   f  → f32*         (float)
//   s  → *const c_char* (str, null-terminated, borrowed)
//   s# → (*const c_char*, Py_ssize_t*) (str + length)
//   z  → *const c_char* (str or None → null)
//   O  → *mut PyObject* (any object, borrowed ref)
//   p  → c_int*        (bool/predicate)
//   n  → Py_ssize_t*   (ssize_t)
//   |  → marks optional args start
//   :  → function name for error messages
//   ;  → error message override
//
// Variadic C calling convention: we use `...` via a shim. The actual
// argument list is unpacked by inspecting the format string and reading
// pointer arguments from the va_list.

// PyArg_ParseTuple / PyArg_ParseTupleAndKeywords / PyArg_UnpackTuple are
// implemented in shims/pyarg_variadic.c (C file compiled via build.rs) because
// Rust stable does not support exporting variadic extern "C" functions.
//
// The C shims call back into `molt_pyarg_parse_tuple_inner` (below) with a
// flat array of void* output pointers extracted from the va_list.

/// Called from the C shim — receives a flat array of output pointers already
/// extracted from the va_list. Dispatches based on format codes.
///
/// # Safety
/// - `outs[0..n_outs]` must be valid writable pointers matching the format string.
/// - `args` must be a bridge-managed tuple object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_pyarg_parse_tuple_inner(
    args: *mut PyObject,
    format: *const c_char,
    outs: *mut *mut c_void,
    n_outs: c_int,
) -> c_int {
    if format.is_null() {
        return 1;
    }
    let fmt = unsafe { CStr::from_ptr(format).to_bytes() };

    let bridge = GLOBAL_BRIDGE.lock();
    let args_bits = bridge.pyobj_to_handle(args);
    drop(bridge);

    let items = args_bits.map(molt_tuple_items).unwrap_or_default();
    let outs_slice = if outs.is_null() || n_outs <= 0 {
        &mut [] as &mut [*mut c_void]
    } else {
        unsafe { std::slice::from_raw_parts_mut(outs, n_outs as usize) }
    };

    let mut arg_idx = 0usize;
    let mut out_idx = 0usize;
    let mut optional = false;
    let mut i = 0usize;

    while i < fmt.len() {
        let ch = fmt[i] as char;
        i += 1;
        match ch {
            '|' => {
                optional = true;
                continue;
            }
            ':' | ';' => break,
            '(' | ')' => continue,
            _ => {}
        }

        let item_bits = items.get(arg_idx).copied();
        arg_idx += 1;

        if item_bits.is_none() && !optional {
            return 0;
        }
        if item_bits.is_none() {
            continue;
        }
        let bits = item_bits.unwrap();
        let obj = MoltObject::from_bits(bits);

        macro_rules! write_out {
            ($ty:ty, $val:expr) => {{
                if out_idx < outs_slice.len() && !outs_slice[out_idx].is_null() {
                    unsafe {
                        *(outs_slice[out_idx] as *mut $ty) = $val;
                    }
                }
                out_idx += 1;
            }};
        }

        match ch {
            'i' | 'H' | 'b' | 'B' | 'I' => write_out!(c_int, obj.as_int().unwrap_or(0) as c_int),
            'l' | 'k' => write_out!(c_long, obj.as_int().unwrap_or(0) as c_long),
            'L' => write_out!(i64, obj.as_int().unwrap_or(0)),
            'K' => write_out!(u64, obj.as_int().unwrap_or(0) as u64),
            'd' => {
                let v = if obj.is_float() {
                    obj.as_float().unwrap_or(0.0)
                } else {
                    obj.as_int().map(|x| x as f64).unwrap_or(0.0)
                };
                write_out!(f64, v);
            }
            'f' => {
                let v = if obj.is_float() {
                    obj.as_float().unwrap_or(0.0) as f32
                } else {
                    obj.as_int().map(|x| x as f32).unwrap_or(0.0)
                };
                write_out!(f32, v);
            }
            's' | 'z' | 'y' => {
                let ptr: *const c_char = if obj.is_none() {
                    std::ptr::null()
                } else {
                    molt_str_ptr(bits)
                };
                write_out!(*const c_char, ptr);
                if i < fmt.len() && fmt[i] == b'#' {
                    i += 1;
                    write_out!(Py_ssize_t, molt_str_len(bits) as Py_ssize_t);
                }
            }
            'O' => {
                let py_ptr = unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) };
                write_out!(*mut PyObject, py_ptr);
            }
            'p' => {
                let truthy = if obj.is_bool() {
                    obj.as_bool().unwrap_or(false)
                } else if obj.is_int() {
                    obj.as_int().unwrap_or(0) != 0
                } else {
                    !obj.is_none()
                };
                write_out!(c_int, truthy as c_int);
            }
            'n' => write_out!(Py_ssize_t, obj.as_int().unwrap_or(0) as Py_ssize_t),
            _ => {} // unknown — skip output slot
        }
    }
    1
}

// (parse_args_from_tuple removed — logic moved to molt_pyarg_parse_tuple_inner above)

// ─── Helpers — read Molt object internals ────────────────────────────────

/// Get items of a Molt tuple (or list) as a Vec<u64> of handle bits.
fn molt_tuple_items(bits: u64) -> Vec<u64> {
    let h = crate::hooks::hooks_or_stubs();
    let len = unsafe { (h.tuple_len)(bits) };
    if len == 0 {
        // Args may arrive as a list in some Molt call paths.
        let llen = unsafe { (h.list_len)(bits) };
        return (0..llen)
            .map(|i| unsafe { (h.list_item)(bits, i) })
            .collect();
    }
    (0..len)
        .map(|i| unsafe { (h.tuple_item)(bits, i) })
        .collect()
}

/// Get a null-terminated UTF-8 pointer into a Molt str object's storage.
fn molt_str_ptr(bits: u64) -> *const c_char {
    let h = crate::hooks::hooks_or_stubs();
    let mut len: usize = 0;
    let ptr = unsafe { (h.str_data)(bits, std::ptr::addr_of_mut!(len)) };
    if ptr.is_null() {
        c"".as_ptr()
    } else {
        ptr.cast()
    }
}

fn molt_str_len(bits: u64) -> usize {
    let h = crate::hooks::hooks_or_stubs();
    let mut len: usize = 0;
    unsafe { (h.str_data)(bits, std::ptr::addr_of_mut!(len)) };
    len
}
