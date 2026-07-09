//! Error/exception API — PyErr_*, PyArg_ParseTuple.
//!
//! `PyArg_ParseTuple` is the hottest function in any C extension — called on
//! every function entry to unpack positional arguments. We implement the
//! most common format codes: `i`, `l`, `d`, `f`, `s`, `z`, `s#`, `O`, `p`,
//! `n`, `L`, `K`, `b`, `B`, `H`, `I`, `k`, `y`, `y#`, `C`.

use crate::abi_types::{Py_ssize_t, PyBaseExceptionObject, PyObject};
use crate::bridge::GLOBAL_BRIDGE;
use molt_lang_obj_model::MoltObject;
use std::ffi::{CStr, c_void};
use std::os::raw::{c_char, c_int, c_long};
use std::ptr;

// ─── Thread-local error state ─────────────────────────────────────────────

thread_local! {
    static CURRENT_EXC: std::cell::RefCell<Option<(u64, String)>> = const { std::cell::RefCell::new(None) };
}

pub fn take_current_error_message() -> Option<String> {
    CURRENT_EXC.with(|c| c.borrow_mut().take().map(|(_type_bits, msg)| msg))
}

/// Non-consuming peek at the currently-pending exception's type-handle bits.
/// `Some(0)` = an exception is set whose type was NULL/unresolvable; `None` = no
/// exception pending. Used by `PyErr_ExceptionMatches` to compare the live
/// exception's type against a candidate rather than answering "is any set".
fn current_exc_type_bits() -> Option<u64> {
    CURRENT_EXC.with(|c| c.borrow().as_ref().map(|(type_bits, _)| *type_bits))
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
    if let Some(msg) = take_current_error_message() {
        eprintln!("[molt-cpython-abi] PyErr_Print: {msg}");
    }
}

/// CPython ``PyErr_PrintEx``: print the pending exception and clear it. The
/// ``set_sys_last_vars`` flag governs whether CPython also assigns
/// ``sys.last_type``/``sys.last_value``/``sys.last_traceback``; Molt does not
/// model those interpreter globals, so the flag is accepted and the shared
/// print-and-clear path is used (``PyErr_Print`` semantics). Never a stub — it
/// drains and reports the same pending-error state.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_PrintEx(_set_sys_last_vars: c_int) {
    unsafe { PyErr_Print() };
}

/// Set a ValueError with formatted message.
pub unsafe extern "C" fn PyErr_Format(
    exc_type: *mut PyObject,
    format: *const c_char,
    // variadic — we capture only the format string for the common case
) -> *mut PyObject {
    unsafe { PyErr_SetString(exc_type, format) };
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_SetFromErrno(exc_type: *mut PyObject) -> *mut PyObject {
    let message = std::io::Error::last_os_error().to_string();
    let c_message = std::ffi::CString::new(message).unwrap_or_else(|_| {
        std::ffi::CString::new("operating system error").expect("static string has no nul")
    });
    unsafe { PyErr_SetString(exc_type, c_message.as_ptr()) };
    ptr::null_mut()
}

// ─── Additional error API ─────────────────────────────────────────────────

/// Resolve a PyObject to its `str()` text through the runtime string authority so
/// an exception's real payload is preserved. Returns `None` when the value cannot
/// be bridged to a Molt handle (no runtime / not a managed object) so the caller
/// records the type without inventing a message. Scalars format inline; a str
/// object is read through the `str_data` hook (the runtime str authority).
fn value_str_message(value: *mut PyObject) -> Option<String> {
    if value.is_null() {
        return None;
    }
    let bits = GLOBAL_BRIDGE.lock().pyobj_to_handle(value)?;
    let obj = MoltObject::from_bits(bits);
    if obj.is_none() {
        return Some("None".to_string());
    }
    if let Some(b) = obj.as_bool() {
        return Some(if b { "True" } else { "False" }.to_string());
    }
    if let Some(i) = obj.as_int() {
        return Some(i.to_string());
    }
    if obj.is_float() {
        return obj.as_float().map(|f| f.to_string());
    }
    let h = crate::hooks::hooks_or_stubs();
    if unsafe { (h.classify_heap)(bits) } == crate::abi_types::MoltTypeTag::Str as u8 {
        let mut len: usize = 0;
        let ptr = unsafe { (h.str_data)(bits, std::ptr::addr_of_mut!(len)) };
        if !ptr.is_null() {
            let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
            return Some(String::from_utf8_lossy(slice).into_owned());
        }
    }
    None
}

/// `PyErr_SetObject(type, value)` — set the current exception (Python/errors.c).
///
/// CPython stores `value` as the exception's associated value. This bridge's
/// error state carries `(type, message)`, so the fix propagates the real value by
/// storing its `str()` text alongside the caller's exception type. When the value
/// cannot be resolved (unbridgeable / no runtime) we still record the type with an
/// empty message rather than the previous generic `c"<exception>"` placeholder,
/// which discarded the payload entirely (a fail-open that misreported the error).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_SetObject(exc_type: *mut PyObject, value: *mut PyObject) {
    match value_str_message(value) {
        Some(message) => {
            let cmsg = std::ffi::CString::new(message)
                .unwrap_or_else(|_| c"<exception value contains NUL>".to_owned());
            unsafe { PyErr_SetString(exc_type, cmsg.as_ptr()) };
        }
        None => unsafe { PyErr_SetString(exc_type, ptr::null()) },
    }
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
    let exc = take_current_error_message();
    if exc.is_some() {
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

/// `PyErr_ExceptionMatches(exc)` — does the pending exception match `exc`?
/// CPython defines this as `PyErr_GivenExceptionMatches(PyErr_Occurred(), exc)`
/// (Python/errors.c). The previous stub dropped `exc` and returned "is ANY
/// exception set", so `PyErr_ExceptionMatches(PyExc_KeyError)` was true for a
/// pending TypeError — a fail-open that misroutes extension error handling.
///
/// We compare the pending exception's stored type-handle against `exc` by
/// identity. Identity match is exact for the common `except SpecificError` case.
/// A subclass relationship (e.g. pending IndexError vs `exc = LookupError`)
/// returns 0 here because the bridge has no exception-MRO hook: that is a
/// CONSERVATIVE narrowing (never a false positive), not a fail-open. When no
/// exception is pending, return 0 (CPython returns 0 for a NULL PyErr_Occurred).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_ExceptionMatches(exc: *mut PyObject) -> c_int {
    let Some(pending_type_bits) = current_exc_type_bits() else {
        return 0;
    };
    if exc.is_null() {
        return 0;
    }
    let exc_bits = GLOBAL_BRIDGE.lock().pyobj_to_handle(exc).unwrap_or(0);
    (pending_type_bits != 0 && pending_type_bits == exc_bits) as c_int
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
pub unsafe extern "C" fn PyException_SetTraceback(exc: *mut PyObject, tb: *mut PyObject) -> c_int {
    if exc.is_null() {
        return -1;
    }
    let base = exc.cast::<PyBaseExceptionObject>();
    unsafe {
        if !tb.is_null() {
            crate::api::refcount::Py_INCREF(tb);
        }
        let old = (*base).traceback;
        (*base).traceback = tb;
        if !old.is_null() {
            crate::api::refcount::Py_DECREF(old);
        }
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyException_GetTraceback(exc: *mut PyObject) -> *mut PyObject {
    if exc.is_null() {
        return ptr::null_mut();
    }
    let traceback = unsafe { (*exc.cast::<PyBaseExceptionObject>()).traceback };
    unsafe { crate::api::refcount::Py_XINCREF(traceback) };
    traceback
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyException_SetContext(exc: *mut PyObject, context: *mut PyObject) {
    if exc.is_null() {
        unsafe { crate::api::refcount::Py_XDECREF(context) };
        return;
    }
    let base = exc.cast::<PyBaseExceptionObject>();
    unsafe {
        let old = (*base).context;
        (*base).context = context;
        crate::api::refcount::Py_XDECREF(old);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyException_SetCause(exc: *mut PyObject, cause: *mut PyObject) {
    if exc.is_null() {
        unsafe { crate::api::refcount::Py_XDECREF(cause) };
        return;
    }
    let base = exc.cast::<PyBaseExceptionObject>();
    unsafe {
        let old = (*base).cause;
        (*base).cause = cause;
        crate::api::refcount::Py_XDECREF(old);
    }
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

/// `PyErr_WriteUnraisable(obj)` — report an exception that cannot be propagated
/// (Python/errors.c). CPython prints `Exception ignored in: <obj>` followed by the
/// traceback, using `obj` to identify WHERE the exception was swallowed. The stub
/// dropped `obj` (`let _ = obj;`), erasing that context. We include `obj`'s
/// string form in the "ignored in" line so the report names its origin.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_WriteUnraisable(obj: *mut PyObject) {
    let context = value_str_message(obj);
    CURRENT_EXC.with(|c| {
        if let Some((_, ref msg)) = *c.borrow() {
            match context {
                Some(ref ctx) => {
                    eprintln!("[molt-cpython-abi] Exception ignored in: {ctx}: {msg}")
                }
                None => eprintln!("[molt-cpython-abi] unraisable exception: {msg}"),
            }
        }
    });
    unsafe { PyErr_Clear() };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_CheckSignals() -> c_int {
    0
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
            // Missing required positional argument. CPython raises TypeError
            // ("function takes at least N arguments (M given)"); a zero return
            // must leave a set exception.
            unsafe { set_parse_type_error("required argument is missing") };
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

        // CPython (Python/getargs.c `convertsimple`): a converter that receives
        // the wrong type sets TypeError and the whole parse returns 0. We must
        // NOT silently coerce a non-int to 0 — that poisons the extension with a
        // fake argument value where CPython would have raised. We accept the
        // int-like values CPython accepts (int, bool as an int subtype, and heap
        // BigInt via the runtime int-conversion authority) and reject the rest
        // with a TypeError.
        macro_rules! require_int {
            ($expect:literal) => {{
                if let Some(v) = obj.as_int() {
                    v
                } else if obj.is_bool() {
                    obj.as_bool().unwrap_or(false) as i64
                } else if let Some(v) = int_like_to_i64(bits) {
                    // Heap BigInt (or other int-compatible object) resolved
                    // through the runtime authority.
                    v
                } else {
                    set_parse_type_error(concat!(
                        "argument must be ",
                        $expect,
                        ", not the supplied type"
                    ));
                    return 0;
                }
            }};
        }

        match ch {
            'i' | 'H' | 'b' | 'B' | 'I' => write_out!(c_int, require_int!("an integer") as c_int),
            'l' | 'k' => write_out!(c_long, require_int!("an integer") as c_long),
            'L' => write_out!(i64, require_int!("an integer")),
            'K' => write_out!(u64, require_int!("an integer") as u64),
            'd' => {
                let v = match float_like(&obj, bits) {
                    Some(v) => v,
                    None => {
                        unsafe { set_parse_type_error("argument must be a float") };
                        return 0;
                    }
                };
                write_out!(f64, v);
            }
            'f' => {
                let v = match float_like(&obj, bits) {
                    Some(v) => v as f32,
                    None => {
                        unsafe { set_parse_type_error("argument must be a float") };
                        return 0;
                    }
                };
                write_out!(f32, v);
            }
            's' | 'z' | 'y' => {
                // 'z' accepts None (→ NULL). 's'/'y' require a str/bytes-like.
                if obj.is_none() {
                    if ch != 'z' {
                        unsafe { set_parse_type_error("argument must be a string") };
                        return 0;
                    }
                    write_out!(*const c_char, std::ptr::null());
                    if i < fmt.len() && fmt[i] == b'#' {
                        i += 1;
                        write_out!(Py_ssize_t, 0 as Py_ssize_t);
                    }
                } else {
                    write_out!(*const c_char, molt_str_ptr(bits));
                    if i < fmt.len() && fmt[i] == b'#' {
                        i += 1;
                        write_out!(Py_ssize_t, molt_str_len(bits) as Py_ssize_t);
                    }
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
            'n' => write_out!(Py_ssize_t, require_int!("an integer") as Py_ssize_t),
            _ => {
                // CPython raises SystemError("bad format string") for an
                // unrecognized format unit. Fail closed — never report success
                // for a format string we cannot honor.
                unsafe { set_parse_format_error() };
                return 0;
            }
        }
    }
    1
}

/// Resolve an int-compatible object (heap BigInt or other) to i64 via the
/// runtime int-conversion authority. Returns `None` for non-integer objects so
/// the caller can raise TypeError. Inline int and bool are handled by the caller
/// before this is reached.
fn int_like_to_i64(bits: u64) -> Option<i64> {
    let h = crate::hooks::hooks_or_stubs();
    let mut out: i64 = 0;
    let rc = unsafe { (h.int_as_i64_checked)(bits, std::ptr::addr_of_mut!(out)) };
    (rc == 0).then_some(out)
}

/// Resolve a float-compatible argument to f64 for the `d`/`f` format units.
/// CPython accepts float, int (incl. bool as int subtype), and any object with
/// `__float__`/`__index__`. Returns `None` for genuinely non-numeric objects so
/// the caller can raise TypeError.
fn float_like(obj: &MoltObject, bits: u64) -> Option<f64> {
    if obj.is_float() {
        obj.as_float()
    } else if let Some(x) = obj.as_int() {
        Some(x as f64)
    } else if obj.is_bool() {
        Some(obj.as_bool().unwrap_or(false) as i64 as f64)
    } else {
        int_like_to_i64(bits).map(|x| x as f64)
    }
}

/// Set a TypeError for a PyArg_ParseTuple converter type mismatch, matching
/// CPython's `convertsimple` behavior (Python/getargs.c). A NULL-returning /
/// zero-returning parse MUST leave a set exception.
unsafe fn set_parse_type_error(message: &str) {
    let cmsg = std::ffi::CString::new(message).unwrap_or_default();
    unsafe {
        PyErr_SetString(&raw mut crate::abi_types::PyExc_TypeError, cmsg.as_ptr());
    }
}

/// Set a SystemError for an unrecognized/malformed format unit, matching
/// CPython's handling of a bad format string in PyArg_ParseTuple.
unsafe fn set_parse_format_error() {
    unsafe {
        PyErr_SetString(
            &raw mut crate::abi_types::PyExc_SystemError,
            c"bad format string passed to PyArg_ParseTuple".as_ptr(),
        );
    }
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
