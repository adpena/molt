//! Error/exception API — PyErr_*, PyArg_ParseTuple.
//!
//! `PyArg_ParseTuple` is the hottest function in any C extension — called on
//! every function entry to unpack positional arguments. We implement the
//! most common format codes: `i`, `l`, `d`, `f`, `s`, `z`, `s#`, `O`, `p`,
//! `n`, `L`, `K`, `b`, `B`, `H`, `I`, `k`, `y`, `y#`, `C`.

use crate::abi_types::{
    MoltTypeTag, Py_buffer, Py_complex, Py_ssize_t, PyBaseExceptionObject, PyObject, PyTypeObject,
    PyBUF_SIMPLE, PyBUF_WRITABLE,
};
use crate::bridge::GLOBAL_BRIDGE;
use molt_lang_obj_model::MoltObject;
use std::ffi::{CStr, c_void};
use std::os::raw::{c_char, c_int, c_long, c_ulong};
use std::ptr;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_NewException(
    name: *const c_char,
    base: *mut PyObject,
    dict: *mut PyObject,
) -> *mut PyObject {
    if name.is_null() {
        unsafe {
            PyErr_SetString(
                &raw mut crate::abi_types::PyExc_SystemError,
                c"PyErr_NewException: name must be module.class".as_ptr(),
            )
        };
        return ptr::null_mut();
    }
    let bytes = unsafe { CStr::from_ptr(name).to_bytes() };
    let Some(dot) = bytes.iter().rposition(|byte| *byte == b'.') else {
        unsafe {
            PyErr_SetString(
                &raw mut crate::abi_types::PyExc_SystemError,
                c"PyErr_NewException: name must be module.class".as_ptr(),
            )
        };
        return ptr::null_mut();
    };
    let mut owned_dict = ptr::null_mut();
    let dict = if dict.is_null() {
        owned_dict = unsafe { crate::api::mapping::PyDict_New() };
        owned_dict
    } else {
        dict
    };
    if dict.is_null() {
        return ptr::null_mut();
    }
    let module =
        unsafe { crate::api::strings::PyUnicode_FromStringAndSize(name, dot as Py_ssize_t) };
    if module.is_null()
        || unsafe {
            crate::api::mapping::PyDict_SetItemString(dict, c"__module__".as_ptr(), module)
        } < 0
    {
        unsafe {
            crate::api::refcount::Py_XDECREF(module);
            crate::api::refcount::Py_XDECREF(owned_dict);
        }
        return ptr::null_mut();
    }
    unsafe { crate::api::refcount::Py_DECREF(module) };
    let base = if base.is_null() {
        &raw mut crate::abi_types::PyExc_Exception
    } else {
        base
    };
    let bases = if unsafe { crate::api::sequences::PyTuple_Check(base) } != 0 {
        unsafe { crate::api::object::Py_NewRef(base) }
    } else {
        let tuple = unsafe { crate::api::sequences::PyTuple_New(1) };
        if !tuple.is_null() {
            unsafe {
                crate::api::refcount::Py_INCREF(base);
                crate::api::sequences::PyTuple_SetItem(tuple, 0, base);
            }
        }
        tuple
    };
    let class_name = unsafe {
        crate::api::strings::PyUnicode_FromStringAndSize(
            name.add(dot + 1),
            (bytes.len() - dot - 1) as Py_ssize_t,
        )
    };
    let args = unsafe { crate::api::sequences::PyTuple_New(3) };
    if bases.is_null() || class_name.is_null() || args.is_null() {
        unsafe {
            crate::api::refcount::Py_XDECREF(bases);
            crate::api::refcount::Py_XDECREF(class_name);
            crate::api::refcount::Py_XDECREF(args);
            crate::api::refcount::Py_XDECREF(owned_dict);
        }
        return ptr::null_mut();
    }
    unsafe {
        crate::api::sequences::PyTuple_SetItem(args, 0, class_name);
        crate::api::sequences::PyTuple_SetItem(args, 1, bases);
        crate::api::refcount::Py_INCREF(dict);
        crate::api::sequences::PyTuple_SetItem(args, 2, dict);
    }
    let result = unsafe {
        crate::api::object::PyObject_Call(
            (&raw mut crate::abi_types::PyType_Type).cast(),
            args,
            ptr::null_mut(),
        )
    };
    unsafe {
        crate::api::refcount::Py_DECREF(args);
        crate::api::refcount::Py_XDECREF(owned_dict);
    }
    result
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_NewExceptionWithDoc(
    name: *const c_char,
    doc: *const c_char,
    base: *mut PyObject,
    dict: *mut PyObject,
) -> *mut PyObject {
    let mut owned_dict = ptr::null_mut();
    let dict = if dict.is_null() {
        owned_dict = unsafe { crate::api::mapping::PyDict_New() };
        owned_dict
    } else {
        dict
    };
    if dict.is_null() {
        return ptr::null_mut();
    }
    if !doc.is_null() {
        let doc_obj = unsafe { crate::api::strings::PyUnicode_FromString(doc) };
        if doc_obj.is_null()
            || unsafe {
                crate::api::mapping::PyDict_SetItemString(dict, c"__doc__".as_ptr(), doc_obj)
            } < 0
        {
            unsafe {
                crate::api::refcount::Py_XDECREF(doc_obj);
                crate::api::refcount::Py_XDECREF(owned_dict);
            }
            return ptr::null_mut();
        }
        unsafe { crate::api::refcount::Py_DECREF(doc_obj) };
    }
    let result = unsafe { PyErr_NewException(name, base, dict) };
    unsafe { crate::api::refcount::Py_XDECREF(owned_dict) };
    result
}

// ─── Thread-local error state ─────────────────────────────────────────────

thread_local! {
    static CURRENT_EXC: std::cell::RefCell<Option<(u64, String)>> = const { std::cell::RefCell::new(None) };
}

pub fn take_current_error_message() -> Option<String> {
    CURRENT_EXC.with(|c| c.borrow_mut().take().map(|(_type_bits, msg)| msg))
}

/// Transfer the pending C-API exception's runtime class handle and message.
///
/// This is the ABI-to-runtime equivalent of consuming CPython's error
/// indicator at a call boundary: the caller becomes responsible for recording
/// the same exception in Molt's unified exception state.
pub fn take_current_error() -> Option<(u64, String)> {
    CURRENT_EXC.with(|c| c.borrow_mut().take())
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
        GLOBAL_BRIDGE
            .pyobj_to_handle(exc_type)
            .map(|identity| identity.as_handle())
            .unwrap_or(0)
    };
    CURRENT_EXC.with(|c| *c.borrow_mut() = Some((type_bits, msg)));
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_SetNone(exc_type: *mut PyObject) {
    unsafe { PyErr_SetString(exc_type, ptr::null()) };
}

/// CPython `PyErr_Occurred` (Python/errors.c): returns the pending exception's
/// actual TYPE (borrowed) or NULL. Consumers do identity/subtype tests on the
/// result (`PyErr_Occurred() == PyExc_StopIteration`,
/// `GivenExceptionMatches(PyErr_Occurred(), X)`), so the pre-fix `&Py_None`
/// sentinel mis-decided every such probe. The stored type-handle bits resolve
/// back to the original `PyExc_*` pointer through the bridge raw registry;
/// only an unresolvable/NULL-typed exception falls back to the non-null
/// `Py_None` sentinel (never NULL while an exception is pending).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_Occurred() -> *mut PyObject {
    let Some(type_bits) = current_exc_type_bits() else {
        return ptr::null_mut();
    };
    if type_bits != 0 {
        let resolved = unsafe { GLOBAL_BRIDGE.handle_to_pyobj(type_bits) };
        if !resolved.is_null() {
            return resolved;
        }
    }
    &raw mut crate::abi_types::Py_None
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

unsafe extern "C" {
    /// C-runtime `errno` accessors from the shim (`pyarg_variadic.c`) — the C
    /// runtime is the only portable authority for `errno` (on Windows,
    /// `std::io::Error::last_os_error()` reads `GetLastError()`, which is a
    /// DIFFERENT channel from the C `errno` an extension just set).
    fn molt_capi_errno() -> c_int;
    fn molt_capi_strerror(errnum: c_int) -> *const c_char;
}

/// CPython `PyErr_SetFromErrno` (Python/errors.c): reads the C `errno` (NOT
/// GetLastError) and formats CPython's OSError shape
/// `[Errno N] strerror-text`. Residual (documented): the bridge error state
/// carries `(type, message)` only, so the instance `.errno`/`.strerror`
/// attributes CPython also sets are not materialized.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_SetFromErrno(exc_type: *mut PyObject) -> *mut PyObject {
    let errnum = unsafe { molt_capi_errno() };
    let detail = unsafe { molt_capi_strerror(errnum) };
    let detail = if detail.is_null() {
        "operating system error".to_string()
    } else {
        unsafe { CStr::from_ptr(detail) }
            .to_string_lossy()
            .into_owned()
    };
    let message = format!("[Errno {errnum}] {detail}");
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
    let value = GLOBAL_BRIDGE.molt_handle_for_pyobj(value)?;
    let bits = value.bits();
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
    if unsafe { (h.classify_heap)(value.bits()) } == crate::abi_types::MoltTypeTag::Str as u8 {
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

/// CPython `PyErr_BadInternalCall` (Python/errors.c): sets **SystemError**
/// ("bad argument to internal function") — the pre-fix RuntimeError broke any
/// caller's `ExceptionMatches(PyExc_SystemError)` probe.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_BadInternalCall() {
    unsafe {
        PyErr_SetString(
            &raw mut crate::abi_types::PyExc_SystemError,
            c"bad argument to internal function".as_ptr(),
        );
    }
}

/// CPython private `_PyErr_BadInternalCall(filename, lineno)` (Python/errors.c):
/// the located form of [`PyErr_BadInternalCall`] — sets **SystemError**
/// `"<file>:<line>: bad argument to internal function"`. When a C extension is
/// built with `assert`-style internal-call checks, its `PyErr_BadInternalCall()`
/// macro expands to this private form carrying `__FILE__`/`__LINE__`; numpy
/// links it. A NULL filename or an interior-NUL message degrades to the
/// no-location wrapper rather than dropping the error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyErr_BadInternalCall(filename: *const c_char, lineno: c_int) {
    if filename.is_null() {
        unsafe { PyErr_BadInternalCall() };
        return;
    }
    let file = unsafe { CStr::from_ptr(filename) }.to_string_lossy();
    let message = format!("{file}:{lineno}: bad argument to internal function");
    match std::ffi::CString::new(message) {
        Ok(cmessage) => unsafe {
            PyErr_SetString(
                &raw mut crate::abi_types::PyExc_SystemError,
                cmessage.as_ptr(),
            );
        },
        Err(_) => unsafe { PyErr_BadInternalCall() },
    }
}

/// CPython `PyErr_Fetch` (Python/errors.c): transfers the pending exception's
/// REAL type plus a value carrying its message into the out-params and clears
/// the indicator, so a Fetch → Restore round-trip preserves the exception.
/// The pre-fix body wrote a `Py_None` type sentinel and a NULL value,
/// destroying both. The value is materialized as the message `str` (the
/// pre-normalization `(type, str, tb)` triple CPython itself allows); NULL
/// when the str authority is unavailable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_Fetch(
    p_type: *mut *mut PyObject,
    p_value: *mut *mut PyObject,
    p_tb: *mut *mut PyObject,
) {
    let state = CURRENT_EXC.with(|c| c.borrow_mut().take());
    let (type_ptr, value_ptr) = match state {
        Some((type_bits, message)) => {
            let type_ptr = if type_bits != 0 {
                let resolved = unsafe { GLOBAL_BRIDGE.handle_to_pyobj(type_bits) };
                if resolved.is_null() {
                    &raw mut crate::abi_types::Py_None
                } else {
                    resolved
                }
            } else {
                &raw mut crate::abi_types::Py_None
            };
            (type_ptr, exception_free_str(&message))
        }
        None => (ptr::null_mut(), ptr::null_mut()),
    };
    if !p_type.is_null() {
        unsafe { *p_type = type_ptr };
    }
    if !p_value.is_null() {
        unsafe { *p_value = value_ptr };
    } else if !value_ptr.is_null() {
        unsafe { crate::api::refcount::Py_DECREF(value_ptr) };
    }
    if !p_tb.is_null() {
        unsafe { *p_tb = ptr::null_mut() };
    }
}

/// CPython `PyErr_Restore` (Python/errors.c): takes ownership of
/// `(type, value, tb)` and re-installs them as the pending exception. The
/// pre-fix THEATER discarded all three and stored a fabricated
/// `(0, "<restored exception>")`, so a later `ExceptionMatches` against the
/// intended type always failed. The real type handle and the value's `str()`
/// text are preserved; the stolen references are released after flattening.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_Restore(
    exc_type: *mut PyObject,
    value: *mut PyObject,
    tb: *mut PyObject,
) {
    if exc_type.is_null() {
        unsafe { PyErr_Clear() };
    } else {
        let type_bits = GLOBAL_BRIDGE
            .pyobj_to_handle(exc_type)
            .map(|identity| identity.as_handle())
            .unwrap_or(0);
        let message = value_str_message(value).unwrap_or_default();
        CURRENT_EXC.with(|c| *c.borrow_mut() = Some((type_bits, message)));
    }
    // Restore steals all three references.
    unsafe {
        crate::api::refcount::Py_XDECREF(exc_type);
        crate::api::refcount::Py_XDECREF(value);
        crate::api::refcount::Py_XDECREF(tb);
    }
}

/// CPython `PyErr_NormalizeException` (Python/errors.c): guarantees a caller
/// reading `*val` after normalization never dereferences NULL. Full
/// instantiation of exception objects needs the runtime class authority; the
/// bridge materializes the message-`str` value (matching what `PyErr_Fetch`
/// produces) when `*val` is NULL, and leaves an already-populated triple
/// untouched.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_NormalizeException(
    exc: *mut *mut PyObject,
    val: *mut *mut PyObject,
    _tb: *mut *mut PyObject,
) {
    if exc.is_null() || val.is_null() {
        return;
    }
    let exc_ptr = unsafe { *exc };
    if exc_ptr.is_null() || !unsafe { *val }.is_null() {
        return;
    }
    let materialized = exception_free_str("");
    if !materialized.is_null() {
        unsafe { *val = materialized };
    }
}

/// Build a str `PyObject` from `text` WITHOUT ever touching the pending-error
/// indicator — `PyUnicode_FromString` sets MemoryError when the str authority
/// is unavailable, which inside `PyErr_Fetch`/`NormalizeException` would
/// clobber the very exception being transferred. Returns NULL (silently) when
/// the authority is absent.
fn exception_free_str(text: &str) -> *mut PyObject {
    let h = crate::hooks::hooks_or_stubs();
    let bits = unsafe { (h.alloc_str)(text.as_ptr(), text.len()) };
    if bits == 0 {
        return ptr::null_mut();
    }
    unsafe { GLOBAL_BRIDGE.handle_to_pyobj(bits) }
}

/// `PyErr_ExceptionMatches(exc)` — does the pending exception match `exc`?
/// CPython defines this as `PyErr_GivenExceptionMatches(PyErr_Occurred(), exc)`
/// (Python/errors.c); with `PyErr_Occurred` now returning the REAL pending
/// type, the delegation is exact — subclass walks (pending IndexError vs
/// `exc = LookupError`) and tuple candidates both match.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_ExceptionMatches(exc: *mut PyObject) -> c_int {
    let given = unsafe { PyErr_Occurred() };
    unsafe { PyErr_GivenExceptionMatches(given, exc) }
}

/// One `given`-vs-single-candidate match: pointer identity, then the builtin
/// exception-hierarchy subclass walk (`exc_singleton_parent`), then the
/// `PyType_IsSubtype` walk for genuine heap exception TYPES an extension
/// registered (both sides must be type objects for that to be sound).
fn given_matches_single(given: *mut PyObject, exc: *mut PyObject) -> bool {
    if std::ptr::eq(given, exc) {
        return true;
    }
    // Builtin singleton subclass chain: IndexError -> LookupError -> ... .
    let mut cursor = given.cast_const();
    while let Some(parent) = crate::abi_types::exc_singleton_parent(cursor) {
        if std::ptr::eq(parent, exc) {
            return true;
        }
        cursor = parent;
    }
    // Heap exception classes (PyType_FromSpec etc.): a real subtype walk when
    // BOTH sides are type objects (ob_type == PyType_Type).
    let is_type_obj = |p: *mut PyObject| {
        !p.is_null()
            && std::ptr::eq(
                unsafe { (*p).ob_type },
                &raw const crate::abi_types::PyType_Type,
            )
    };
    if is_type_obj(given) && is_type_obj(exc) {
        return unsafe {
            crate::api::typeobj::PyType_IsSubtype(
                given.cast::<PyTypeObject>(),
                exc.cast::<PyTypeObject>(),
            )
        } != 0;
    }
    false
}

/// CPython `PyErr_GivenExceptionMatches` (Python/errors.c): resolves an
/// exception INSTANCE to its class, iterates a tuple of candidates, and walks
/// the subclass relation — the pre-fix raw `ptr::eq` failed every
/// `except (A, B)` and every subclass catch a numpy C path relies on.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_GivenExceptionMatches(
    given: *mut PyObject,
    exc: *mut PyObject,
) -> c_int {
    if given.is_null() || exc.is_null() {
        return 0;
    }
    // An exception INSTANCE resolves to its class first (Python/errors.c).
    // Exception singletons/types carry no bridge instance state; only a
    // non-singleton object whose ob_type is a registered exception type is an
    // instance here. The singleton fast path covers the dominant case.
    let given = if crate::abi_types::exc_singleton_name(given).is_some() {
        given
    } else {
        let ob_type = unsafe { (*given).ob_type };
        if !ob_type.is_null() && crate::abi_types::exc_singleton_name(ob_type.cast()).is_some() {
            ob_type.cast::<PyObject>()
        } else {
            given
        }
    };
    // A tuple of candidates: match any member (bridged tuple via the runtime
    // tuple hooks). Resolve `exc`'s bits and RELEASE the bridge lock before the
    // per-item `handle_to_pyobj` calls — the bridge Mutex is non-reentrant, so
    // holding the guard across the loop would self-deadlock.
    let exc_bits = GLOBAL_BRIDGE.molt_handle_for_pyobj(exc);
    if let Some(value) = exc_bits
        && value.decode().is_ptr()
    {
        let h = crate::hooks::hooks_or_stubs();
        if unsafe { (h.classify_heap)(value.bits()) } == MoltTypeTag::Tuple as u8 {
            let len = unsafe { (h.tuple_len)(value.bits()) };
            for i in 0..len {
                let item_bits = unsafe { (h.tuple_item)(value.bits(), i) };
                if item_bits == 0 {
                    continue;
                }
                let item = unsafe { GLOBAL_BRIDGE.handle_to_pyobj(item_bits) };
                if !item.is_null() && given_matches_single(given, item) {
                    return 1;
                }
            }
            return 0;
        }
    }
    given_matches_single(given, exc) as c_int
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

/// CPython `PyErr_WarnEx` (Python/_warnings.c): runs the warnings machinery,
/// which by default PRINTS the warning to stderr ("<source>: Category:
/// message"). The bridge has no warnings-filter state (documented residual: a
/// filter mapping the category to "error" cannot raise here), but the default
/// visible-emission path is honored — the pre-fix silent swallow hid every
/// extension warning. Writing to stderr matches CPython's own default
/// destination.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_WarnEx(
    category: *mut PyObject,
    message: *const c_char,
    _stack_level: c_int,
) -> c_int {
    let text = if message.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(message) }
            .to_string_lossy()
            .into_owned()
    };
    let category_name = crate::abi_types::exc_singleton_name(category)
        .map(|name| name.strip_prefix("PyExc_").unwrap_or(name))
        .unwrap_or("UserWarning");
    eprintln!("<molt-cpython-abi>: {category_name}: {text}");
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

/// CPython `PyErr_CheckSignals` runs pending signal handlers so a Ctrl-C can
/// interrupt a long C loop. ACCEPTED NO-OP (ledger `errors.rs:375`): the wasm
/// witness tier has no signal delivery (WASI has no signals) and the native
/// bridge installs no handlers, so there is never a pending signal to run;
/// returning 0 is the truthful answer, not a stub. Revisit if a host with
/// real signal routing is added.
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

    let bridge = &*GLOBAL_BRIDGE;
    let args_bits = bridge.molt_handle_for_pyobj(args).map(|value| value.bits());

    let items = args_bits.map(molt_tuple_items).unwrap_or_default();
    let outs_slice = if outs.is_null() || n_outs <= 0 {
        &mut [] as &mut [*mut c_void]
    } else {
        unsafe { std::slice::from_raw_parts_mut(outs, n_outs as usize) }
    };

    let mut arg_idx = 0usize;
    let mut out_idx = 0usize;
    let mut optional = false;
    // Number of required value units (before `|`); `usize::MAX` until a `|` is
    // seen, meaning min == max ("exactly N"). Drives the surplus-arg TypeError.
    let mut min_units = usize::MAX;
    let mut i = 0usize;

    while i < fmt.len() {
        let ch = fmt[i] as char;
        i += 1;
        match ch {
            '|' => {
                optional = true;
                min_units = arg_idx;
                continue;
            }
            '$' => continue,
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
                // Evaluate the value (which may itself raise + early-return)
                // OUTSIDE the unsafe store so it never nests in this block.
                let stored: $ty = $val;
                if out_idx < outs_slice.len() && !outs_slice[out_idx].is_null() {
                    unsafe {
                        *(outs_slice[out_idx] as *mut $ty) = stored;
                    }
                }
                out_idx += 1;
            }};
        }

        // CPython (Python/getargs.c `convertsimple`): a converter that receives
        // the wrong type sets TypeError and the whole parse returns 0. Resolve
        // the current argument to an int-like `i64` (int, bool-as-int-subtype, or
        // a heap BigInt / `__index__` object via the runtime authority); a float
        // is NOT int-like for the integer units. `None` => raise TypeError.
        macro_rules! int_arg {
            () => {{
                match arg_int_like(&obj, bits) {
                    Some(v) => v,
                    None => {
                        unsafe { set_parse_type_error("argument must be an integer") };
                        return 0;
                    }
                }
            }};
        }

        // Signed, range-checked store (CPython 'b'/'h'/'i' raise OverflowError
        // with the exact getargs.c message). The store width is the EXACT C width
        // the caller declared — u8 for b, i16 for h, i32 for i — so the previous
        // 4-byte `c_int` store into a 1/2-byte target (adjacent-memory clobber)
        // is gone.
        macro_rules! int_ranged {
            ($v:expr, $ty:ty, $lo:expr, $hi:expr, $lomsg:literal, $himsg:literal) => {{
                let value = $v;
                if value < $lo {
                    unsafe { set_parse_overflow($lomsg) };
                    return 0;
                }
                if value > $hi {
                    unsafe { set_parse_overflow($himsg) };
                    return 0;
                }
                write_out!($ty, value as $ty);
            }};
        }

        match ch {
            // ── Signed, range-checked (OverflowError on out-of-range) ──────────
            // Each stores its EXACT declared C width; the previous `as c_int`
            // 4-byte store into a 1/2-byte b/B/H target (OOB write) is gone.
            'b' => int_ranged!(
                int_arg!(),
                u8,
                0,
                u8::MAX as i64,
                "unsigned byte integer is less than minimum",
                "unsigned byte integer is greater than maximum"
            ),
            'h' => int_ranged!(
                int_arg!(),
                i16,
                i16::MIN as i64,
                i16::MAX as i64,
                "signed short integer is less than minimum",
                "signed short integer is greater than maximum"
            ),
            'i' => int_ranged!(
                int_arg!(),
                i32,
                i32::MIN as i64,
                i32::MAX as i64,
                "signed integer is less than minimum",
                "signed integer is greater than maximum"
            ),
            // 'l': PyLong_AsLong range (OverflowError). `try_from` is width- and
            // platform-correct (c_long is 32-bit on Windows/wasm32, 64-bit on
            // LP64) and clippy-clean (no absurd fixed-width comparison).
            'l' => match c_long::try_from(int_arg!()) {
                Ok(v) => write_out!(c_long, v),
                Err(_) => {
                    unsafe { set_parse_overflow("Python int too large to convert to C long") };
                    return 0;
                }
            },
            // ── Unsigned bitfield (mask low N bits, no range check) ────────────
            'B' => write_out!(u8, int_arg!() as u8),
            'H' => write_out!(u16, int_arg!() as u16),
            'I' => write_out!(u32, int_arg!() as u32),
            'k' => write_out!(c_ulong, int_arg!() as c_ulong),
            'L' => write_out!(i64, int_arg!()),
            'K' => write_out!(u64, int_arg!() as u64),
            'n' => write_out!(Py_ssize_t, int_arg!() as Py_ssize_t),
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
            'D' => {
                let py_ptr = unsafe { GLOBAL_BRIDGE.handle_to_pyobj(bits) };
                let value = unsafe { crate::api::numbers::PyComplex_AsCComplex(py_ptr) };
                if !unsafe { PyErr_Occurred() }.is_null() {
                    return 0;
                }
                write_out!(Py_complex, value);
            }
            'e' => {
                if i >= fmt.len() || (fmt[i] != b's' && fmt[i] != b't') {
                    unsafe { set_parse_format_error() };
                    return 0;
                }
                let accepts_bytes = fmt[i] == b't';
                i += 1;
                let has_len = i < fmt.len() && fmt[i] == b'#';
                if has_len {
                    i += 1;
                }
                let encoding = outs_slice
                    .get(out_idx)
                    .copied()
                    .unwrap_or(ptr::null_mut())
                    .cast::<c_char>();
                let dest = outs_slice
                    .get(out_idx + 1)
                    .copied()
                    .unwrap_or(ptr::null_mut())
                    .cast::<*mut c_char>();
                let len_dest = if has_len {
                    outs_slice
                        .get(out_idx + 2)
                        .copied()
                        .unwrap_or(ptr::null_mut())
                        .cast::<Py_ssize_t>()
                } else {
                    ptr::null_mut()
                };
                out_idx += if has_len { 3 } else { 2 };
                if dest.is_null() {
                    unsafe { set_parse_format_error() };
                    return 0;
                }
                let py_ptr = unsafe { GLOBAL_BRIDGE.handle_to_pyobj(bits) };
                let mut owned_bytes = ptr::null_mut();
                let source = if accepts_bytes && arg_is_bytes(&obj, bits) {
                    py_ptr
                } else if arg_is_str(&obj, bits) {
                    owned_bytes = unsafe {
                        crate::api::strings::PyUnicode_AsEncodedString(
                            py_ptr,
                            if encoding.is_null() { c"utf-8".as_ptr() } else { encoding },
                            c"strict".as_ptr(),
                        )
                    };
                    if owned_bytes.is_null() {
                        return 0;
                    }
                    owned_bytes
                } else {
                    unsafe { set_parse_type_error("argument must be str") };
                    return 0;
                };
                let mut source_ptr = ptr::null_mut();
                let mut source_len = 0;
                if unsafe {
                    crate::api::strings::PyBytes_AsStringAndSize(
                        source,
                        &raw mut source_ptr,
                        &raw mut source_len,
                    )
                } != 0
                {
                    unsafe { crate::api::refcount::Py_XDECREF(owned_bytes) };
                    return 0;
                }
                if !has_len && unsafe {
                    std::slice::from_raw_parts(source_ptr.cast::<u8>(), source_len as usize)
                }
                .contains(&0)
                {
                    unsafe { crate::api::refcount::Py_XDECREF(owned_bytes) };
                    unsafe { set_parse_type_error("encoded string without null bytes") };
                    return 0;
                }
                let required = source_len as usize + 1;
                let buffer = unsafe { *dest };
                let output = if buffer.is_null() {
                    unsafe { crate::api::memory::PyMem_Malloc(required) }.cast::<c_char>()
                } else {
                    if !has_len || len_dest.is_null() || unsafe { *len_dest } < required as Py_ssize_t {
                        unsafe { crate::api::refcount::Py_XDECREF(owned_bytes) };
                        unsafe { set_parse_value_error("encoded string too long") };
                        return 0;
                    }
                    buffer
                };
                if output.is_null() {
                    unsafe { crate::api::refcount::Py_XDECREF(owned_bytes) };
                    return 0;
                }
                unsafe {
                    ptr::copy_nonoverlapping(source_ptr, output, source_len as usize);
                    *output.add(source_len as usize) = 0;
                    *dest = output;
                    if has_len && !len_dest.is_null() {
                        *len_dest = source_len;
                    }
                    crate::api::refcount::Py_XDECREF(owned_bytes);
                }
            }
            's' | 'z' => {
                // CPython 's' requires str; 'z' also accepts None (→ NULL).
                // A non-str non-None object is a TypeError — NOT a fabricated
                // empty string (the prior `molt_str_ptr` theater on an int/list).
                let has_len = i < fmt.len() && fmt[i] == b'#';
                let has_buffer = i < fmt.len() && fmt[i] == b'*';
                if obj.is_none() {
                    if ch != 'z' {
                        unsafe { set_parse_type_error("argument must be str, not None") };
                        return 0;
                    }
                    if has_buffer {
                        i += 1;
                        let view = outs_slice
                            .get(out_idx)
                            .copied()
                            .unwrap_or(ptr::null_mut())
                            .cast::<Py_buffer>();
                        out_idx += 1;
                        if !view.is_null() {
                            unsafe { ptr::write_bytes(view, 0, 1) };
                        }
                        continue;
                    }
                    write_out!(*const c_char, std::ptr::null());
                    if has_len {
                        i += 1;
                        write_out!(Py_ssize_t, 0 as Py_ssize_t);
                    }
                } else if arg_is_str(&obj, bits) {
                    if has_buffer {
                        i += 1;
                        let view = outs_slice
                            .get(out_idx)
                            .copied()
                            .unwrap_or(ptr::null_mut())
                            .cast::<Py_buffer>();
                        out_idx += 1;
                        let py_ptr = unsafe { GLOBAL_BRIDGE.handle_to_pyobj(bits) };
                        if unsafe {
                            crate::api::buffer::PyBuffer_FillInfo(
                                view,
                                py_ptr,
                                molt_str_ptr(bits).cast_mut().cast(),
                                molt_str_len(bits) as Py_ssize_t,
                                1,
                                PyBUF_SIMPLE,
                            )
                        } != 0
                        {
                            return 0;
                        }
                        continue;
                    }
                    if !has_len && str_has_interior_nul(bits) {
                        unsafe { set_parse_value_error("embedded null character") };
                        return 0;
                    }
                    write_out!(*const c_char, molt_str_ptr(bits));
                    if has_len {
                        i += 1;
                        write_out!(Py_ssize_t, molt_str_len(bits) as Py_ssize_t);
                    }
                } else {
                    unsafe { set_parse_type_error("argument must be str") };
                    return 0;
                }
            }
            'y' => {
                // CPython 'y' requires a bytes-like object (buffer protocol), NOT
                // the str authority. Non-'#' form rejects an interior NUL.
                let has_len = i < fmt.len() && fmt[i] == b'#';
                let has_buffer = i < fmt.len() && fmt[i] == b'*';
                if has_buffer {
                    i += 1;
                    let view = outs_slice
                        .get(out_idx)
                        .copied()
                        .unwrap_or(ptr::null_mut())
                        .cast::<Py_buffer>();
                    out_idx += 1;
                    let py_ptr = unsafe { GLOBAL_BRIDGE.handle_to_pyobj(bits) };
                    if unsafe {
                        crate::api::buffer::PyObject_GetBuffer(py_ptr, view, PyBUF_SIMPLE)
                    } != 0
                    {
                        return 0;
                    }
                    continue;
                }
                if arg_is_bytes(&obj, bits) {
                    if !has_len && bytes_has_interior_nul(bits) {
                        unsafe { set_parse_value_error("embedded null byte") };
                        return 0;
                    }
                    write_out!(*const c_char, molt_bytes_ptr(bits));
                    if has_len {
                        i += 1;
                        write_out!(Py_ssize_t, molt_bytes_len(bits) as Py_ssize_t);
                    }
                } else {
                    unsafe { set_parse_type_error("a bytes-like object is required") };
                    return 0;
                }
            }
            'S' => {
                // Requires PyBytes; stores the borrowed object.
                if arg_is_bytes(&obj, bits) {
                    let py_ptr = unsafe { GLOBAL_BRIDGE.handle_to_pyobj(bits) };
                    write_out!(*mut PyObject, py_ptr);
                } else {
                    unsafe { set_parse_type_error("argument must be bytes") };
                    return 0;
                }
            }
            'Y' => {
                let py_ptr = unsafe { GLOBAL_BRIDGE.handle_to_pyobj(bits) };
                if unsafe { crate::api::strings::PyByteArray_Check(py_ptr) } != 0 {
                    write_out!(*mut PyObject, py_ptr);
                } else {
                    unsafe { set_parse_type_error("argument must be bytearray") };
                    return 0;
                }
            }
            'U' => {
                // Requires PyUnicode; stores the borrowed object.
                if arg_is_str(&obj, bits) {
                    let py_ptr = unsafe { GLOBAL_BRIDGE.handle_to_pyobj(bits) };
                    write_out!(*mut PyObject, py_ptr);
                } else {
                    unsafe { set_parse_type_error("argument must be str") };
                    return 0;
                }
            }
            'c' => {
                // A bytes/bytearray of length 1 → one C `char`.
                if arg_is_bytes(&obj, bits) && molt_bytes_len(bits) == 1 {
                    let p = molt_bytes_ptr(bits);
                    let byte = if p.is_null() {
                        0u8
                    } else {
                        unsafe { *p.cast::<u8>() }
                    };
                    write_out!(c_char, byte as c_char);
                } else {
                    unsafe { set_parse_type_error("argument must be a byte string of length 1") };
                    return 0;
                }
            }
            'C' => {
                // A str of length 1 → the code point as a C `int`.
                match str_single_codepoint_if_str(&obj, bits) {
                    Some(cp) => write_out!(c_int, cp as c_int),
                    None => {
                        unsafe {
                            set_parse_type_error(
                                "argument must be a unicode character, not a string",
                            )
                        };
                        return 0;
                    }
                }
            }
            'w' => {
                if i >= fmt.len() || fmt[i] != b'*' {
                    unsafe { set_parse_format_error() };
                    return 0;
                }
                i += 1;
                let view = outs_slice
                    .get(out_idx)
                    .copied()
                    .unwrap_or(ptr::null_mut())
                    .cast::<Py_buffer>();
                out_idx += 1;
                let py_ptr = unsafe { GLOBAL_BRIDGE.handle_to_pyobj(bits) };
                if unsafe {
                    crate::api::buffer::PyObject_GetBuffer(py_ptr, view, PyBUF_WRITABLE)
                } != 0
                {
                    return 0;
                }
            }
            'O' => {
                let py_ptr = unsafe { GLOBAL_BRIDGE.handle_to_pyobj(bits) };
                // Peek for the 'O!' (type-checked) / 'O&' (converter) modifiers.
                let modifier = if i < fmt.len() && (fmt[i] == b'!' || fmt[i] == b'&') {
                    let m = fmt[i];
                    i += 1;
                    Some(m)
                } else {
                    None
                };
                match modifier {
                    None => write_out!(*mut PyObject, py_ptr),
                    Some(b'!') => {
                        // getargs.c 'O!' consumes TWO varargs: the caller's
                        // `PyTypeObject*` (a VALUE, never written through) then the
                        // `PyObject**` destination. The prior grammar skipped '!'
                        // and let the plain-'O' store write into the type slot,
                        // clobbering the type-object header (UB). Here the type is
                        // only READ, the object is stored into the destination.
                        let type_ptr = outs_slice
                            .get(out_idx)
                            .copied()
                            .unwrap_or(std::ptr::null_mut())
                            .cast::<PyTypeObject>();
                        let dest = outs_slice
                            .get(out_idx + 1)
                            .copied()
                            .unwrap_or(std::ptr::null_mut())
                            .cast::<*mut PyObject>();
                        out_idx += 2;
                        let arg_type = unsafe { crate::api::object::Py_TYPE(py_ptr) };
                        if type_ptr.is_null()
                            || unsafe { crate::api::typeobj::PyType_IsSubtype(arg_type, type_ptr) }
                                == 0
                        {
                            unsafe { set_parse_o_bang_type_error(type_ptr, arg_type) };
                            return 0;
                        }
                        if !dest.is_null() {
                            unsafe { *dest = py_ptr };
                        }
                    }
                    Some(b'&') => {
                        // 'O&' consumes a converter fn + a destination address and
                        // calls `convert(arg, addr)`; a 0 return fails the parse.
                        let raw = outs_slice
                            .get(out_idx)
                            .copied()
                            .unwrap_or(std::ptr::null_mut());
                        let addr = outs_slice
                            .get(out_idx + 1)
                            .copied()
                            .unwrap_or(std::ptr::null_mut());
                        out_idx += 2;
                        if raw.is_null() {
                            unsafe { set_parse_format_error() };
                            return 0;
                        }
                        let convert: ConverterFn =
                            unsafe { std::mem::transmute::<*mut c_void, ConverterFn>(raw) };
                        if unsafe { convert(py_ptr, addr) } == 0 {
                            // The converter should have set an exception; guarantee
                            // a NULL-return never escapes without one.
                            if unsafe { PyErr_Occurred() }.is_null() {
                                unsafe { set_parse_type_error("argument conversion failed") };
                            }
                            return 0;
                        }
                    }
                    Some(_) => unreachable!("modifier peek only accepts b'!'/b'&'"),
                }
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
            _ => {
                // CPython raises SystemError("bad format string") for an
                // unrecognized format unit. Fail closed — never report success
                // for a format string we cannot honor.
                unsafe { set_parse_format_error() };
                return 0;
            }
        }
    }
    // CPython vgetargs1_impl rejects an args tuple with MORE items than the
    // format's value units: TypeError "... takes {exactly|at most} N argument(s)
    // (M given)". `arg_idx` == the number of value units the format declared.
    if items.len() > arg_idx {
        let effective_min = if min_units == usize::MAX {
            arg_idx
        } else {
            min_units
        };
        let word = if effective_min == arg_idx {
            "exactly"
        } else {
            "at most"
        };
        let plural = if arg_idx == 1 { "" } else { "s" };
        unsafe {
            set_parse_type_error(&format!(
                "function takes {word} {arg_idx} argument{plural} ({} given)",
                items.len()
            ))
        };
        return 0;
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

/// Converter-function pointer for the PyArg `O&` unit
/// (CPython `int (*)(PyObject *, void *)`).
type ConverterFn = unsafe extern "C" fn(*mut PyObject, *mut c_void) -> c_int;

/// Resolve the current argument to an int-like `i64` for the integer format
/// units. Accepts int, bool (an int subtype), and a heap BigInt / `__index__`
/// object via the runtime authority; a float is deliberately NOT int-like
/// (CPython's integer units convert through `PyLong_AsLong`, which rejects a
/// float). `None` => the caller raises TypeError.
fn arg_int_like(obj: &MoltObject, bits: u64) -> Option<i64> {
    if let Some(v) = obj.as_int() {
        Some(v)
    } else if obj.is_bool() {
        Some(obj.as_bool().unwrap_or(false) as i64)
    } else if obj.is_float() {
        None
    } else {
        int_like_to_i64(bits)
    }
}

/// Classify a heap argument handle via the runtime tag hook (`None` for a
/// non-heap immediate). Backs the `s`/`z`/`y`/`S`/`U`/`c`/`C` type checks so a
/// wrong-typed arg raises TypeError instead of fabricating an empty string.
fn arg_heap_tag(obj: &MoltObject, bits: u64) -> Option<u8> {
    obj.is_ptr()
        .then(|| unsafe { (crate::hooks::hooks_or_stubs().classify_heap)(bits) })
}

fn arg_is_str(obj: &MoltObject, bits: u64) -> bool {
    arg_heap_tag(obj, bits) == Some(MoltTypeTag::Str as u8)
}

fn arg_is_bytes(obj: &MoltObject, bits: u64) -> bool {
    arg_heap_tag(obj, bits) == Some(MoltTypeTag::Bytes as u8)
}

/// Null-terminated pointer into a bytes handle's storage (runtime `bytes_data`
/// authority). Returns a null pointer when unavailable.
fn molt_bytes_ptr(bits: u64) -> *const c_char {
    let h = crate::hooks::hooks_or_stubs();
    let mut len = 0usize;
    let ptr = unsafe { (h.bytes_data)(bits, std::ptr::addr_of_mut!(len)) };
    if ptr.is_null() {
        std::ptr::null()
    } else {
        ptr.cast()
    }
}

fn molt_bytes_len(bits: u64) -> usize {
    let h = crate::hooks::hooks_or_stubs();
    let mut len = 0usize;
    unsafe { (h.bytes_data)(bits, std::ptr::addr_of_mut!(len)) };
    len
}

/// True when a str handle's UTF-8 storage contains an interior NUL byte, which
/// CPython rejects for the non-`#` `s`/`z` units (ValueError).
fn str_has_interior_nul(bits: u64) -> bool {
    let h = crate::hooks::hooks_or_stubs();
    let mut len = 0usize;
    let ptr = unsafe { (h.str_data)(bits, std::ptr::addr_of_mut!(len)) };
    !ptr.is_null() && unsafe { std::slice::from_raw_parts(ptr, len) }.contains(&0)
}

fn bytes_has_interior_nul(bits: u64) -> bool {
    let h = crate::hooks::hooks_or_stubs();
    let mut len = 0usize;
    let ptr = unsafe { (h.bytes_data)(bits, std::ptr::addr_of_mut!(len)) };
    !ptr.is_null() && unsafe { std::slice::from_raw_parts(ptr, len) }.contains(&0)
}

/// The single code point of a length-1 str argument (for the `C` unit); `None`
/// unless the arg is a str whose content is exactly one code point.
fn str_single_codepoint_if_str(obj: &MoltObject, bits: u64) -> Option<u32> {
    if !arg_is_str(obj, bits) {
        return None;
    }
    let h = crate::hooks::hooks_or_stubs();
    let mut len = 0usize;
    let ptr = unsafe { (h.str_data)(bits, std::ptr::addr_of_mut!(len)) };
    if ptr.is_null() {
        return None;
    }
    let text = std::str::from_utf8(unsafe { std::slice::from_raw_parts(ptr, len) }).ok()?;
    let mut chars = text.chars();
    let first = chars.next()?;
    chars.next().is_none().then_some(first as u32)
}

/// Set OverflowError for an out-of-range integer format unit (getargs.c range
/// checks).
unsafe fn set_parse_overflow(message: &str) {
    let cmsg = std::ffi::CString::new(message).unwrap_or_default();
    unsafe {
        PyErr_SetString(
            &raw mut crate::abi_types::PyExc_OverflowError,
            cmsg.as_ptr(),
        );
    }
}

/// Set ValueError for an embedded-NUL `s`/`z`/`y` argument.
unsafe fn set_parse_value_error(message: &str) {
    let cmsg = std::ffi::CString::new(message).unwrap_or_default();
    unsafe {
        PyErr_SetString(&raw mut crate::abi_types::PyExc_ValueError, cmsg.as_ptr());
    }
}

/// TypeError for an `O!` type mismatch, shaped like CPython's
/// `converterr(type->tp_name, ...)`.
unsafe fn set_parse_o_bang_type_error(want: *mut PyTypeObject, got: *mut PyTypeObject) {
    fn tp_name(tp: *mut PyTypeObject) -> String {
        if tp.is_null() {
            return "<unknown>".to_string();
        }
        let name = unsafe { (*tp).tp_name };
        if name.is_null() {
            "<unknown>".to_string()
        } else {
            unsafe { CStr::from_ptr(name) }
                .to_string_lossy()
                .into_owned()
        }
    }
    let message = format!("argument must be {}, not {}", tp_name(want), tp_name(got));
    unsafe { set_parse_type_error(&message) };
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

#[cfg(test)]
mod bad_internal_call_tests {
    use super::*;

    /// `_PyErr_BadInternalCall(file, line)` sets a SystemError whose message
    /// carries the `file:line` location (the located form numpy links), and a
    /// NULL filename degrades to the no-location wrapper rather than dropping the
    /// error. The message is stored in the thread-local exception state, so this
    /// is exercisable without runtime hooks.
    #[test]
    fn located_bad_internal_call_carries_location() {
        unsafe {
            PyErr_Clear();
            _PyErr_BadInternalCall(c"multiarraymodule.c".as_ptr(), 4242);
        }
        let msg = take_current_error_message().unwrap_or_default();
        assert!(
            msg.contains("multiarraymodule.c:4242"),
            "located message must carry file:line, got {msg:?}"
        );
        assert!(
            msg.contains("bad argument to internal function"),
            "got {msg:?}"
        );
    }

    #[test]
    fn null_filename_degrades_to_wrapper() {
        unsafe {
            PyErr_Clear();
            _PyErr_BadInternalCall(std::ptr::null(), 0);
        }
        let msg = take_current_error_message().unwrap_or_default();
        assert_eq!(msg, "bad argument to internal function");
    }
}
