//! Error/exception API — PyErr_*, PyArg_ParseTuple.
//!
//! `PyArg_ParseTuple` is the hottest function in any C extension — called on
//! every function entry to unpack positional arguments. We implement the
//! most common format codes: `i`, `l`, `d`, `f`, `s`, `z`, `s#`, `O`, `p`,
//! `n`, `L`, `K`, `b`, `B`, `H`, `I`, `k`, `y`, `y#`, `C`.

use crate::abi_types::{
    MoltTypeTag, Py_buffer, Py_complex, Py_ssize_t, PyBUF_SIMPLE, PyBUF_WRITABLE,
    PyBaseExceptionObject, PyObject, PyTypeObject,
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
                (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
                c"PyErr_NewException: name must be module.class".as_ptr(),
            )
        };
        return ptr::null_mut();
    }
    let bytes = unsafe { CStr::from_ptr(name).to_bytes() };
    let Some(dot) = bytes.iter().rposition(|byte| *byte == b'.') else {
        unsafe {
            PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
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
        (&raw mut crate::abi_types::PyExc_Exception).cast::<crate::abi_types::PyObject>()
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

pub use crate::api::object::OwnedCError;

thread_local! {
    static NORMALIZING_EXCEPTION: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

struct ExceptionNormalizationGuard;

impl ExceptionNormalizationGuard {
    fn enter() -> Option<Self> {
        NORMALIZING_EXCEPTION.with(|active| {
            if active.replace(true) {
                None
            } else {
                Some(Self)
            }
        })
    }
}

impl Drop for ExceptionNormalizationGuard {
    fn drop(&mut self) {
        NORMALIZING_EXCEPTION.with(|active| active.set(false));
    }
}

fn replace_current_error(state: Option<OwnedCError>) {
    let old = crate::api::object::replace_thread_state_error(state);
    drop(old);
}

/// Transfer the exact owned C error-indicator triple.
pub fn take_current_error() -> Option<OwnedCError> {
    crate::api::object::take_thread_state_error()
}

/// Restore a previously detached, already-normalized C error triple without
/// re-running construction or projecting it through text.
pub fn restore_current_error_exact(error: OwnedCError) {
    replace_current_error(Some(error));
}

/// Non-consuming peek at the currently-pending exception's type-handle bits.
/// `Some(0)` = an exception is set whose type was NULL/unresolvable; `None` = no
/// exception pending. Used by `PyErr_ExceptionMatches` to compare the live
/// exception's type against a candidate rather than answering "is any set".
fn current_exc_type_ptr() -> Option<*mut PyObject> {
    crate::api::object::thread_state_error_type()
}

/// Install the only error shape available before runtime hooks are registered:
/// a concrete SystemError type with no fabricated payload. Production paths
/// normalize to an exception instance through `normalize_exception`; this
/// fail-closed state exists only when that authority itself is unavailable.
unsafe fn install_normalization_failure() {
    let exc_type =
        (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>();
    unsafe { crate::api::refcount::Py_INCREF(exc_type) };
    replace_current_error(Some(OwnedCError {
        exc_type,
        value: ptr::null_mut(),
        traceback: ptr::null_mut(),
    }));
}

fn owned_c_error_from_runtime_projection(
    result: crate::hooks::OwnedHandleResult,
    class_bits: u64,
    traceback_bits: u64,
) -> Option<OwnedCError> {
    let hooks = crate::hooks::hooks_or_stubs();
    let crate::hooks::DecodedHandleResult::Ok(exception_bits) = result.decode() else {
        return None;
    };
    if exception_bits == 0 || class_bits == 0 {
        if exception_bits != 0 {
            unsafe { (hooks.dec_ref)(exception_bits) };
        }
        return None;
    }
    let exc_type = unsafe { GLOBAL_BRIDGE.handle_to_borrowed_pyobj(class_bits) };
    let traceback = if traceback_bits == 0 {
        ptr::null_mut()
    } else {
        unsafe { GLOBAL_BRIDGE.handle_to_borrowed_pyobj(traceback_bits) }
    };
    if exc_type.is_null() || (traceback_bits != 0 && traceback.is_null()) {
        unsafe { (hooks.dec_ref)(exception_bits) };
        return None;
    }
    unsafe {
        crate::api::refcount::Py_INCREF(exc_type);
        crate::api::refcount::Py_XINCREF(traceback);
    }
    let value = unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(exception_bits) };
    if value.is_null() {
        unsafe {
            crate::api::refcount::Py_DECREF(exc_type);
            crate::api::refcount::Py_XDECREF(traceback);
        }
        return None;
    }
    Some(OwnedCError {
        exc_type,
        value,
        traceback,
    })
}

/// Move an exact runtime-pending exception into the C indicator without text
/// conversion. The hook detaches the runtime pending edge; this function takes
/// independent C references to its exact class and traceback before returning.
fn take_runtime_pending_error() -> Option<OwnedCError> {
    let hooks = crate::hooks::hooks_or_stubs();
    let mut class_bits = 0u64;
    let mut traceback_bits = 0u64;
    let result =
        unsafe { (hooks.take_pending_exception)(&raw mut class_bits, &raw mut traceback_bits) };
    owned_c_error_from_runtime_projection(result, class_bits, traceback_bits)
}

/// Move the exact runtime pending instance into CURRENT_EXC when the C channel
/// is clear. Returns whether either canonical channel now has an error.
pub(crate) fn transfer_runtime_pending_to_current() -> bool {
    if current_exc_type_ptr().is_some() {
        return true;
    }
    let Some(error) = take_runtime_pending_error() else {
        return false;
    };
    replace_current_error(Some(error));
    true
}

/// Explicitly suppress both error channels for APIs such as PyDict_GetItem
/// whose CPython contract masks lookup/hash failures.
pub(crate) fn clear_all_pending_errors() {
    replace_current_error(None);
    if let Some(error) = take_runtime_pending_error() {
        drop(error);
    }
}

fn clear_new_runtime_pending_error(had_pending: bool) {
    if had_pending {
        return;
    }
    let hooks = crate::hooks::hooks_or_stubs();
    if unsafe { (hooks.exception_pending)() } == 0 {
        return;
    }
    let mut class_bits = 0u64;
    let mut traceback_bits = 0u64;
    let result =
        unsafe { (hooks.take_pending_exception)(&raw mut class_bits, &raw mut traceback_bits) };
    if let crate::hooks::DecodedHandleResult::Ok(exception_bits) = result.decode()
        && exception_bits != 0
    {
        unsafe { (hooks.dec_ref)(exception_bits) };
    }
}

/// Consume an owned error ingress and return its canonical exception-instance
/// form. Managed builtin/user classes normalize through the runtime's class
/// call authority; genuine foreign exception classes keep their native
/// `tp_call`/`PyBaseExceptionObject` authority. No branch converts payloads.
unsafe fn normalize_owned_error(mut error: OwnedCError) -> Option<OwnedCError> {
    if let Some(requested_class) = GLOBAL_BRIDGE.molt_handle_for_pyobj(error.exc_type) {
        let hooks = crate::hooks::hooks_or_stubs();
        let has_value = !error.value.is_null();
        let (args_bits, value_bits) = if error.value.is_null()
            || std::ptr::eq(error.value, &raw mut crate::abi_types::Py_None)
        {
            let args_bits = unsafe { (hooks.alloc_tuple)(0) };
            if args_bits == 0 {
                return None;
            }
            (args_bits, has_value.then_some(MoltObject::none().bits()))
        } else if unsafe { crate::api::sequences::PyTuple_Check(error.value) } != 0 {
            let args_bits = if let Some(tuple) = GLOBAL_BRIDGE.molt_handle_for_pyobj(error.value) {
                unsafe { (hooks.inc_ref)(tuple.bits()) };
                tuple.bits()
            } else {
                unsafe { crate::api::object::molt_tuple_bits_from_c_tuple(error.value) }?
            };
            (args_bits, Some(args_bits))
        } else {
            let value_bits = unsafe { GLOBAL_BRIDGE.molt_value_for_pyobj(error.value) }?;
            let args_bits = unsafe { (hooks.alloc_tuple)(1) };
            if args_bits == 0 {
                unsafe { (hooks.dec_ref)(value_bits) };
                return None;
            }
            let set_result = unsafe { (hooks.tuple_set)(args_bits, 0, value_bits, error.value) };
            match set_result.decode() {
                crate::hooks::DecodedHandleResult::Ok(old_bits) if old_bits != 0 => unsafe {
                    (hooks.dec_ref)(old_bits)
                },
                crate::hooks::DecodedHandleResult::Ok(_)
                | crate::hooks::DecodedHandleResult::Missing => {}
                crate::hooks::DecodedHandleResult::Error => {
                    unsafe {
                        (hooks.dec_ref)(args_bits);
                        (hooks.dec_ref)(value_bits);
                    }
                    return None;
                }
            }
            (args_bits, Some(value_bits))
        };
        let traceback_bits = if error.traceback.is_null() {
            None
        } else {
            unsafe { GLOBAL_BRIDGE.molt_value_for_pyobj(error.traceback) }
        };
        if !error.traceback.is_null() && traceback_bits.is_none() {
            unsafe { (hooks.dec_ref)(args_bits) };
            if let Some(bits) = value_bits
                && bits != args_bits
            {
                unsafe { (hooks.dec_ref)(bits) };
            }
            return None;
        }
        let mut actual_class_bits = 0u64;
        let result = unsafe {
            (hooks.normalize_exception)(
                requested_class.bits(),
                args_bits,
                value_bits.unwrap_or(0),
                c_int::from(has_value),
                traceback_bits.unwrap_or(0),
                c_int::from(!error.traceback.is_null()),
                &raw mut actual_class_bits,
            )
        };
        unsafe { (hooks.dec_ref)(args_bits) };
        if let Some(bits) = value_bits
            && bits != args_bits
        {
            unsafe { (hooks.dec_ref)(bits) };
        }
        if let Some(bits) = traceback_bits {
            unsafe { (hooks.dec_ref)(bits) };
        }
        let crate::hooks::DecodedHandleResult::Ok(normalized_bits) = result.decode() else {
            if let Some(runtime_error) = take_runtime_pending_error() {
                replace_current_error(Some(runtime_error));
            }
            return None;
        };
        if normalized_bits == 0 || actual_class_bits == 0 {
            if normalized_bits != 0 {
                unsafe { (hooks.dec_ref)(normalized_bits) };
            }
            return None;
        }
        let actual_type = unsafe { GLOBAL_BRIDGE.handle_to_borrowed_pyobj(actual_class_bits) };
        if actual_type.is_null() {
            unsafe { (hooks.dec_ref)(normalized_bits) };
            return None;
        }
        let normalized = unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(normalized_bits) };
        if normalized.is_null() {
            return None;
        }
        if !std::ptr::eq(actual_type, error.exc_type) {
            unsafe {
                crate::api::refcount::Py_INCREF(actual_type);
                crate::api::refcount::Py_DECREF(error.exc_type);
            }
            error.exc_type = actual_type;
        }
        unsafe { crate::api::refcount::Py_XDECREF(error.value) };
        error.value = normalized;
        return Some(error);
    }

    let old_value = error.value;
    let normalized = if !old_value.is_null()
        && unsafe { PyErr_GivenExceptionMatches(old_value, error.exc_type) } != 0
    {
        unsafe { crate::api::refcount::Py_INCREF(old_value) };
        old_value
    } else {
        let args =
            if old_value.is_null() || std::ptr::eq(old_value, &raw mut crate::abi_types::Py_None) {
                unsafe { crate::api::sequences::native_call_args(&[]) }
            } else if unsafe { crate::api::sequences::PyTuple_Check(old_value) } != 0 {
                unsafe { crate::api::refcount::Py_INCREF(old_value) };
                old_value
            } else {
                unsafe { crate::api::sequences::native_call_args(&[old_value]) }
            };
        if args.is_null() {
            return None;
        }
        let normalized =
            unsafe { crate::api::object::PyObject_Call(error.exc_type, args, ptr::null_mut()) };
        unsafe { crate::api::refcount::Py_DECREF(args) };
        normalized
    };
    if normalized.is_null() {
        return None;
    }
    if !error.traceback.is_null()
        && unsafe { PyException_SetTraceback(normalized, error.traceback) } != 0
    {
        unsafe { crate::api::refcount::Py_DECREF(normalized) };
        return None;
    }
    let actual_type = unsafe { (*normalized).ob_type }.cast::<PyObject>();
    if !actual_type.is_null() && !std::ptr::eq(actual_type, error.exc_type) {
        unsafe {
            crate::api::refcount::Py_INCREF(actual_type);
            crate::api::refcount::Py_DECREF(error.exc_type);
        }
        error.exc_type = actual_type;
    }
    unsafe { crate::api::refcount::Py_XDECREF(error.value) };
    error.value = normalized;
    Some(error)
}

unsafe fn normalize_and_replace(error: OwnedCError) {
    // Error construction may itself fail. Suppress nested normalization and
    // let the outer transaction retain its original exact error triple; this
    // prevents MemoryError/SystemError reporting from recursively allocating
    // until stack exhaustion.
    let Some(_normalization) = ExceptionNormalizationGuard::enter() else {
        return;
    };
    unsafe {
        crate::api::refcount::Py_XINCREF(error.exc_type);
        crate::api::refcount::Py_XINCREF(error.value);
        crate::api::refcount::Py_XINCREF(error.traceback);
    }
    let fallback = OwnedCError {
        exc_type: error.exc_type,
        value: error.value,
        traceback: error.traceback,
    };
    replace_current_error(None);
    if let Some(error) = unsafe { normalize_owned_error(error) } {
        drop(fallback);
        replace_current_error(Some(error));
    } else if current_exc_type_ptr().is_none() {
        replace_current_error(Some(fallback));
    } else {
        drop(fallback);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_SetString(exc_type: *mut PyObject, message: *const c_char) {
    let msg = if message.is_null() {
        ""
    } else {
        let bytes = unsafe { CStr::from_ptr(message).to_bytes() };
        let Ok(message) = std::str::from_utf8(bytes) else {
            // Use the Unicode constructor's canonical UTF-8 decoder so the
            // decode failure itself becomes pending. Never turn invalid input
            // into an unrelated empty exception message.
            let invalid = unsafe { crate::api::strings::PyUnicode_FromString(message) };
            unsafe { crate::api::refcount::Py_XDECREF(invalid) };
            if current_exc_type_ptr().is_none() {
                unsafe { install_normalization_failure() };
            }
            return;
        };
        message
    };
    let value = exception_free_str(msg);
    if value.is_null() {
        // Bootstrap/partial-hook mode may lack the runtime string allocator.
        // Preserve the requested exception class with an empty argument tuple
        // instead of recursively replacing it with an unrelated SystemError.
        unsafe { PyErr_SetObject(exc_type, ptr::null_mut()) };
        if current_exc_type_ptr().is_none() {
            unsafe { install_normalization_failure() };
        }
        return;
    }
    unsafe {
        PyErr_SetObject(exc_type, value);
        crate::api::refcount::Py_DECREF(value);
    }
}

/// Allocate a concrete native `PyBaseExceptionObject` for canonical `PyExc_*`
/// types before (or without) runtime-handle binding. This is also inherited by
/// honest C subtypes through `PyType_Ready`, so `type_call` has one physical
/// exception construction authority instead of recursively reporting a missing
/// `tp_new` slot through `PyErr_SetString`.
pub unsafe extern "C" fn molt_native_exception_new(
    subtype: *mut PyTypeObject,
    args: *mut PyObject,
    kwds: *mut PyObject,
) -> *mut PyObject {
    if subtype.is_null() {
        unsafe {
            replace_current_error(None);
            install_normalization_failure();
        }
        return ptr::null_mut();
    }
    if !kwds.is_null() && unsafe { crate::api::mapping::PyDict_Size(kwds) } != 0 {
        unsafe {
            PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_TypeError).cast::<PyObject>(),
                c"BaseException does not take keyword arguments".as_ptr(),
            )
        };
        return ptr::null_mut();
    }
    let owned_args = if args.is_null() {
        unsafe { crate::api::sequences::PyTuple_New(0) }
    } else {
        unsafe { crate::api::refcount::Py_INCREF(args) };
        args
    };
    if owned_args.is_null() {
        return ptr::null_mut();
    }
    let size = unsafe { (*subtype).tp_basicsize }
        .max(std::mem::size_of::<PyBaseExceptionObject>() as crate::abi_types::Py_ssize_t)
        as usize;
    let Ok(layout) =
        std::alloc::Layout::from_size_align(size, std::mem::align_of::<PyBaseExceptionObject>())
    else {
        unsafe {
            crate::api::refcount::Py_DECREF(owned_args);
            replace_current_error(None);
            install_normalization_failure();
        }
        return ptr::null_mut();
    };
    let allocation = unsafe { std::alloc::alloc_zeroed(layout) };
    if allocation.is_null() {
        unsafe {
            crate::api::refcount::Py_DECREF(owned_args);
            replace_current_error(None);
            install_normalization_failure();
        }
        return ptr::null_mut();
    }
    let instance = allocation.cast::<PyBaseExceptionObject>();
    unsafe {
        crate::api::refcount::Py_INCREF(subtype.cast::<PyObject>());
        instance.write(PyBaseExceptionObject {
            ob_base: PyObject {
                ob_refcnt: 1,
                ob_type: subtype,
            },
            dict: ptr::null_mut(),
            args: owned_args,
            notes: ptr::null_mut(),
            traceback: ptr::null_mut(),
            context: ptr::null_mut(),
            cause: ptr::null_mut(),
            suppress_context: 0,
        });
    }
    instance.cast::<PyObject>()
}

/// CPython `BaseException.__str__`: zero args -> empty string, one arg ->
/// `str(arg)`, multiple args -> `str(args)`. Managed exception views expose the
/// same physical `PyBaseExceptionObject` fields, so this slot is shared by the
/// bootstrap-native and runtime-backed paths.
pub unsafe extern "C" fn molt_native_exception_str(op: *mut PyObject) -> *mut PyObject {
    let (args, owns_args) = if let Some(result) =
        unsafe { managed_exception_get_field(op, crate::hooks::ExceptionField::Args) }
    {
        match result {
            Ok(args) => (args, true),
            Err(()) => return ptr::null_mut(),
        }
    } else {
        let Some(base) = foreign_exception_layout(op) else {
            unsafe {
                PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_TypeError).cast::<PyObject>(),
                    c"BaseException.__str__ requires an exception instance".as_ptr(),
                )
            };
            return ptr::null_mut();
        };
        (unsafe { (*base).args }, false)
    };
    let result = if args.is_null() {
        unsafe { crate::api::strings::PyUnicode_FromStringAndSize(c"".as_ptr(), 0) }
    } else {
        match unsafe { crate::api::sequences::PyTuple_Size(args) } {
            0 => unsafe { crate::api::strings::PyUnicode_FromStringAndSize(c"".as_ptr(), 0) },
            1 => {
                let item = unsafe { crate::api::sequences::PyTuple_GetItem(args, 0) };
                if item.is_null() {
                    ptr::null_mut()
                } else {
                    unsafe { crate::api::typeobj::PyObject_Str(item) }
                }
            }
            value if value > 1 => unsafe { crate::api::typeobj::PyObject_Str(args) },
            _ => ptr::null_mut(),
        }
    };
    if owns_args {
        unsafe { crate::api::refcount::Py_XDECREF(args) };
    }
    result
}

/// Release the exact field/type edges owned by [`molt_native_exception_new`].
pub unsafe extern "C" fn molt_native_exception_dealloc(op: *mut PyObject) {
    if op.is_null() {
        return;
    }
    let instance = op.cast::<PyBaseExceptionObject>();
    let subtype = unsafe { (*op).ob_type };
    let size = if subtype.is_null() {
        std::mem::size_of::<PyBaseExceptionObject>()
    } else {
        unsafe { (*subtype).tp_basicsize }
            .max(std::mem::size_of::<PyBaseExceptionObject>() as crate::abi_types::Py_ssize_t)
            as usize
    };
    unsafe {
        crate::api::refcount::Py_XDECREF((*instance).dict);
        crate::api::refcount::Py_XDECREF((*instance).args);
        crate::api::refcount::Py_XDECREF((*instance).notes);
        crate::api::refcount::Py_XDECREF((*instance).traceback);
        crate::api::refcount::Py_XDECREF((*instance).context);
        crate::api::refcount::Py_XDECREF((*instance).cause);
        crate::api::refcount::Py_XDECREF(subtype.cast::<PyObject>());
    }
    if let Ok(layout) =
        std::alloc::Layout::from_size_align(size, std::mem::align_of::<PyBaseExceptionObject>())
    {
        unsafe { std::alloc::dealloc(op.cast::<u8>(), layout) };
    }
}

/// Replace an already-pending C error with a SystemError while retaining the
/// exact original exception instance as implicit context. Native-call result
/// validators use this for the CPython "success result with error set" state.
pub(crate) unsafe fn replace_current_with_system_error(message: &str) {
    let original = take_current_error();
    let c_message = std::ffi::CString::new(message).unwrap_or_else(|_| {
        std::ffi::CString::new("native call contract violation")
            .expect("static message contains no nul")
    });
    unsafe {
        PyErr_SetString(
            (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
            c_message.as_ptr(),
        )
    };
    let Some(replacement) = take_current_error() else {
        drop(original);
        unsafe { install_normalization_failure() };
        return;
    };
    if let Some(original) = original {
        if !replacement.value.is_null()
            && !original.value.is_null()
            && !std::ptr::eq(replacement.value, original.value)
        {
            unsafe {
                crate::api::refcount::Py_INCREF(original.value);
                PyException_SetContext(replacement.value, original.value);
            }
            // The generic field authority should accept both normalized
            // instances. If it cannot, retain the SystemError rather than let
            // the attachment failure replace the boundary diagnosis.
            if current_exc_type_ptr().is_some() {
                replace_current_error(None);
            }
        }
        drop(original);
    }
    replace_current_error(Some(replacement));
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_SetNone(exc_type: *mut PyObject) {
    unsafe { PyErr_SetObject(exc_type, ptr::null_mut()) };
}

/// CPython `PyErr_Occurred` (Python/errors.c): returns the pending exception's
/// actual TYPE (borrowed) or NULL. Consumers do identity/subtype tests on the
/// result (`PyErr_Occurred() == PyExc_StopIteration`,
/// `GivenExceptionMatches(PyErr_Occurred(), X)`), so the pre-fix `&Py_None`
/// sentinel mis-decided every such probe. Normalization stores the exact class
/// of the canonical exception instance (including an accepted subtype or an
/// OSError-selected subtype); there is no non-type fallback pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_Occurred() -> *mut PyObject {
    let Some(exc_type) = current_exc_type_ptr() else {
        return ptr::null_mut();
    };
    exc_type
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_Clear() {
    // Raw ABI cleanup is valid before initialization and after shutdown, but
    // those phases have no live runtime whose managed pointers may be
    // decref'd. Shutdown proves the retained-state count is zero before it
    // frees RuntimeState; preserve that proof by never opening thread-state
    // TLS on the runtime-absent path.
    if !crate::api::object::runtime_is_initialized() {
        return;
    }
    replace_current_error(None);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_Print() {
    let Some(state) = take_current_error() else {
        return;
    };
    unsafe { print_owned_error(state) };
}

unsafe fn print_owned_error(state: OwnedCError) {
    let rendered = if state.value.is_null() {
        ptr::null_mut()
    } else {
        unsafe { crate::api::typeobj::PyObject_Str(state.value) }
    };
    if rendered.is_null() {
        // Display failures must not replace the exception being printed with a
        // new pending exception. PyErr_Print/PyErr_PrintEx consume the error
        // indicator even when bootstrap mode cannot allocate its text.
        replace_current_error(None);
        let type_name =
            crate::abi_types::exc_singleton_name(state.exc_type).unwrap_or("<exception>");
        eprintln!("[molt-cpython-abi] PyErr_Print: {type_name}");
        return;
    }
    let text = unsafe { crate::api::strings::PyUnicode_AsUTF8(rendered) };
    if !text.is_null() {
        eprintln!(
            "[molt-cpython-abi] PyErr_Print: {}",
            unsafe { CStr::from_ptr(text) }.to_string_lossy()
        );
    }
    unsafe { crate::api::refcount::Py_DECREF(rendered) };
    replace_current_error(None);
}

unsafe fn publish_sys_last_error(state: &OwnedCError) {
    let hooks = crate::hooks::hooks_or_stubs();
    let had_runtime_pending = unsafe { (hooks.exception_pending)() } != 0;
    let sys_bits = unsafe { (hooks.import_module)(b"sys".as_ptr(), 3) };
    if sys_bits == 0 {
        clear_new_runtime_pending_error(had_runtime_pending);
        replace_current_error(None);
        return;
    }
    for (name, value) in [
        (b"last_type".as_slice(), state.exc_type),
        (b"last_value".as_slice(), state.value),
        (b"last_traceback".as_slice(), state.traceback),
        (b"last_exc".as_slice(), state.value),
    ] {
        let (value_bits, owned) = if value.is_null() {
            (MoltObject::none().bits(), false)
        } else {
            match unsafe { GLOBAL_BRIDGE.molt_value_for_pyobj(value) } {
                Some(bits) => (bits, true),
                None => break,
            }
        };
        let status = unsafe {
            let status = (hooks.module_set_attr)(sys_bits, name.as_ptr(), name.len(), value_bits);
            if owned {
                (hooks.dec_ref)(value_bits);
            }
            status
        };
        if status != 0 {
            break;
        }
    }
    unsafe { (hooks.dec_ref)(sys_bits) };
    clear_new_runtime_pending_error(had_runtime_pending);
    replace_current_error(None);
}

/// Print and clear the exact pending exception; when requested, publish the
/// CPython 3.12 `sys.last_exc` and legacy `sys.last_*` views first (the ABI
/// target is fixed at 3.12).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_PrintEx(set_sys_last_vars: c_int) {
    let Some(state) = take_current_error() else {
        return;
    };
    if set_sys_last_vars != 0 {
        unsafe { publish_sys_last_error(&state) };
    }
    unsafe { print_owned_error(state) };
}

unsafe extern "C" {
    /// C-runtime `errno` accessors from the shim (`pyarg_variadic.c`) — the C
    /// runtime is the only portable authority for `errno` (on Windows,
    /// `std::io::Error::last_os_error()` reads `GetLastError()`, which is a
    /// DIFFERENT channel from the C `errno` an extension just set).
    fn molt_capi_errno() -> c_int;
    fn molt_capi_strerror(errnum: c_int) -> *const c_char;
}

/// Build CPython's structured OSError constructor tuple. The runtime's class
/// call remains the one construction authority; this layer only preserves the
/// `(errno, strerror[, filename[, 0, filename2]])` argument shape.
unsafe fn set_from_errno_with_filename_objects(
    exc_type: *mut PyObject,
    filename: *mut PyObject,
    filename2: *mut PyObject,
) -> *mut PyObject {
    let errnum = unsafe { molt_capi_errno() };
    let detail = unsafe { molt_capi_strerror(errnum) };
    let detail = if detail.is_null() {
        c"operating system error".as_ptr()
    } else {
        detail
    };
    let argc = if filename2.is_null() {
        if filename.is_null() { 2 } else { 3 }
    } else {
        5
    };
    let args = unsafe { crate::api::sequences::PyTuple_New(argc) };
    if args.is_null() {
        return ptr::null_mut();
    }
    let errno_obj = unsafe { crate::api::numbers::PyLong_FromLong(errnum as c_long) };
    let strerror_obj = unsafe { crate::api::strings::PyUnicode_FromString(detail) };
    if errno_obj.is_null() || strerror_obj.is_null() {
        unsafe {
            crate::api::refcount::Py_XDECREF(errno_obj);
            crate::api::refcount::Py_XDECREF(strerror_obj);
            crate::api::refcount::Py_DECREF(args);
        }
        return ptr::null_mut();
    }
    if unsafe { crate::api::sequences::PyTuple_SetItem(args, 0, errno_obj) } != 0
        || unsafe { crate::api::sequences::PyTuple_SetItem(args, 1, strerror_obj) } != 0
    {
        unsafe { crate::api::refcount::Py_DECREF(args) };
        return ptr::null_mut();
    }
    if !filename.is_null() {
        unsafe { crate::api::refcount::Py_INCREF(filename) };
        if unsafe { crate::api::sequences::PyTuple_SetItem(args, 2, filename) } != 0 {
            unsafe { crate::api::refcount::Py_DECREF(args) };
            return ptr::null_mut();
        }
    }
    if !filename2.is_null() {
        let winerror = unsafe { crate::api::numbers::PyLong_FromLong(0) };
        if winerror.is_null() {
            unsafe { crate::api::refcount::Py_DECREF(args) };
            return ptr::null_mut();
        }
        if unsafe { crate::api::sequences::PyTuple_SetItem(args, 3, winerror) } != 0 {
            unsafe { crate::api::refcount::Py_DECREF(args) };
            return ptr::null_mut();
        }
        unsafe { crate::api::refcount::Py_INCREF(filename2) };
        if unsafe { crate::api::sequences::PyTuple_SetItem(args, 4, filename2) } != 0 {
            unsafe { crate::api::refcount::Py_DECREF(args) };
            return ptr::null_mut();
        }
    }
    let exc_type = if exc_type.is_null() {
        (&raw mut crate::abi_types::PyExc_OSError).cast::<crate::abi_types::PyObject>()
    } else {
        exc_type
    };
    unsafe {
        PyErr_SetObject(exc_type, args);
        crate::api::refcount::Py_DECREF(args);
    }
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_SetFromErrno(exc_type: *mut PyObject) -> *mut PyObject {
    unsafe { set_from_errno_with_filename_objects(exc_type, ptr::null_mut(), ptr::null_mut()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_SetFromErrnoWithFilenameObject(
    exc_type: *mut PyObject,
    filename: *mut PyObject,
) -> *mut PyObject {
    unsafe { set_from_errno_with_filename_objects(exc_type, filename, ptr::null_mut()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_SetFromErrnoWithFilenameObjects(
    exc_type: *mut PyObject,
    filename: *mut PyObject,
    filename2: *mut PyObject,
) -> *mut PyObject {
    unsafe { set_from_errno_with_filename_objects(exc_type, filename, filename2) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_SetFromErrnoWithFilename(
    exc_type: *mut PyObject,
    filename: *const c_char,
) -> *mut PyObject {
    if filename.is_null() {
        return unsafe { PyErr_SetFromErrno(exc_type) };
    }
    let filename_obj = unsafe { crate::api::strings::PyUnicode_FromString(filename) };
    if filename_obj.is_null() {
        return ptr::null_mut();
    }
    let result = unsafe { PyErr_SetFromErrnoWithFilenameObject(exc_type, filename_obj) };
    unsafe { crate::api::refcount::Py_DECREF(filename_obj) };
    result
}

// ─── Additional error API ─────────────────────────────────────────────────

/// `PyErr_SetObject(type, value)` — set the current exception (Python/errors.c).
///
/// CPython 3.12 constructs or accepts the canonical exception instance
/// immediately. `value == NULL` is a zero-argument construction; a non-instance
/// value is the exact single positional argument. A matching instance is kept
/// by identity, and no payload is converted to text.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_SetObject(exc_type: *mut PyObject, value: *mut PyObject) {
    if exc_type.is_null() {
        replace_current_error(None);
        unsafe { install_normalization_failure() };
        return;
    }
    unsafe {
        crate::api::refcount::Py_XINCREF(exc_type);
        crate::api::refcount::Py_XINCREF(value);
    }
    unsafe {
        normalize_and_replace(OwnedCError {
            exc_type,
            value,
            traceback: ptr::null_mut(),
        })
    };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_NoMemory() -> *mut PyObject {
    unsafe {
        PyErr_SetString(
            (&raw mut crate::abi_types::PyExc_MemoryError).cast::<crate::abi_types::PyObject>(),
            c"out of memory".as_ptr(),
        );
    }
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_BadArgument() -> c_int {
    unsafe {
        PyErr_SetString(
            (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
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
            (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
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
                (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
                cmessage.as_ptr(),
            );
        },
        Err(_) => unsafe { PyErr_BadInternalCall() },
    }
}

/// Transfer the exact owned `(type, value, traceback)` error indicator and
/// clear it without normalization or payload conversion.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_Fetch(
    p_type: *mut *mut PyObject,
    p_value: *mut *mut PyObject,
    p_tb: *mut *mut PyObject,
) {
    let state = take_current_error().map(std::mem::ManuallyDrop::new);
    let (type_ptr, value_ptr, traceback_ptr) = state
        .as_ref()
        .map(|state| (state.exc_type, state.value, state.traceback))
        .unwrap_or((ptr::null_mut(), ptr::null_mut(), ptr::null_mut()));
    if !p_type.is_null() {
        unsafe { *p_type = type_ptr };
    } else if !type_ptr.is_null() {
        unsafe { crate::api::refcount::Py_DECREF(type_ptr) };
    }
    if !p_value.is_null() {
        unsafe { *p_value = value_ptr };
    } else if !value_ptr.is_null() {
        unsafe { crate::api::refcount::Py_DECREF(value_ptr) };
    }
    if !p_tb.is_null() {
        unsafe { *p_tb = traceback_ptr };
    } else if !traceback_ptr.is_null() {
        unsafe { crate::api::refcount::Py_DECREF(traceback_ptr) };
    }
}

/// Return the runtime's active handled exception (`sys.exception()`) as a new
/// reference, distinct from the propagating error indicator.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_GetHandledException() -> *mut PyObject {
    let result = unsafe { (crate::hooks::hooks_or_stubs().handled_exception_get)() };
    match result.decode() {
        crate::hooks::DecodedHandleResult::Missing => ptr::null_mut(),
        crate::hooks::DecodedHandleResult::Error => {
            let _ = transfer_runtime_pending_to_current();
            ptr::null_mut()
        }
        crate::hooks::DecodedHandleResult::Ok(bits) => unsafe {
            GLOBAL_BRIDGE.owned_handle_to_pyobj(bits)
        },
    }
}

/// Replace the active handled exception. The public CPython API borrows `exc`;
/// the hook consumes the independent owned runtime handle minted here.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_SetHandledException(exc: *mut PyObject) {
    let owned_bits = if exc.is_null() {
        Some(0)
    } else {
        unsafe { GLOBAL_BRIDGE.molt_value_for_pyobj(exc) }
    };
    let status = match owned_bits {
        Some(bits) => unsafe { (crate::hooks::hooks_or_stubs().handled_exception_set)(bits) },
        None => -1,
    };
    if status != 0 && !transfer_runtime_pending_to_current() {
        crate::capi_trace::record_silent_failure(
            "PyErr_SetHandledException",
            Some("runtime handled-exception authority unavailable"),
        );
        unsafe {
            PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_SystemError).cast::<PyObject>(),
                c"PyErr_SetHandledException: runtime handled-exception authority unavailable"
                    .as_ptr(),
            )
        };
    }
}

/// Read the runtime's active handled exception (`sys.exc_info()`). Every
/// non-NULL output is a new reference, matching CPython 3.12.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_GetExcInfo(
    p_type: *mut *mut PyObject,
    p_value: *mut *mut PyObject,
    p_tb: *mut *mut PyObject,
) {
    unsafe {
        if !p_type.is_null() {
            *p_type = ptr::null_mut();
        }
        if !p_value.is_null() {
            *p_value = ptr::null_mut();
        }
        if !p_tb.is_null() {
            *p_tb = ptr::null_mut();
        }
    }
    let value = unsafe { PyErr_GetHandledException() };
    if value.is_null() {
        return;
    }
    let exc_type = unsafe {
        let tp = (*value).ob_type;
        if tp.is_null() {
            ptr::null_mut()
        } else {
            crate::api::object::Py_NewRef(tp.cast::<PyObject>())
        }
    };
    let traceback = unsafe { PyException_GetTraceback(value) };
    unsafe {
        if p_type.is_null() {
            crate::api::refcount::Py_XDECREF(exc_type);
        } else {
            *p_type = exc_type;
        }
        if p_value.is_null() {
            crate::api::refcount::Py_DECREF(value);
        } else {
            *p_value = value;
        }
        if p_tb.is_null() {
            crate::api::refcount::Py_XDECREF(traceback);
        } else {
            *p_tb = traceback;
        }
    }
}

/// Replace the runtime's active handled exception. CPython 3.12 ignores the
/// legacy type/traceback values, derives both from `value`, and still steals all
/// three incoming references.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_SetExcInfo(
    exc_type: *mut PyObject,
    value: *mut PyObject,
    traceback: *mut PyObject,
) {
    unsafe { PyErr_SetHandledException(value) };
    unsafe {
        crate::api::refcount::Py_XDECREF(exc_type);
        crate::api::refcount::Py_XDECREF(value);
        crate::api::refcount::Py_XDECREF(traceback);
    }
}

/// Take ownership of an ingress triple, normalize it to the exact exception
/// instance, attach/validate its traceback, and install that canonical state.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_Restore(
    exc_type: *mut PyObject,
    value: *mut PyObject,
    tb: *mut PyObject,
) {
    if exc_type.is_null() {
        drop(OwnedCError {
            exc_type,
            value,
            traceback: tb,
        });
        replace_current_error(None);
    } else {
        unsafe {
            normalize_and_replace(OwnedCError {
                exc_type,
                value,
                traceback: tb,
            })
        };
    }
}

/// Normalize an owned caller triple in place without any text projection.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_NormalizeException(
    exc: *mut *mut PyObject,
    val: *mut *mut PyObject,
    tb: *mut *mut PyObject,
) {
    if exc.is_null() || val.is_null() {
        return;
    }
    let exc_type = unsafe { *exc };
    if exc_type.is_null() {
        return;
    }
    let value = unsafe { std::mem::replace(&mut *val, ptr::null_mut()) };
    unsafe { *exc = ptr::null_mut() };
    let traceback = if tb.is_null() {
        ptr::null_mut()
    } else {
        unsafe { std::mem::replace(&mut *tb, ptr::null_mut()) }
    };
    let Some(normalized) = (unsafe {
        normalize_owned_error(OwnedCError {
            exc_type,
            value,
            traceback,
        })
    }) else {
        if current_exc_type_ptr().is_none() {
            unsafe { install_normalization_failure() };
        }
        return;
    };
    let normalized = std::mem::ManuallyDrop::new(normalized);
    unsafe {
        *exc = normalized.exc_type;
        *val = normalized.value;
        if tb.is_null() {
            crate::api::refcount::Py_XDECREF(normalized.traceback);
        } else {
            *tb = normalized.traceback;
        }
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
    unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) }
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
    let given_handle = GLOBAL_BRIDGE.molt_handle_for_pyobj(given);
    let given = if let Some(handle) = given_handle
        && unsafe { (crate::hooks::hooks_or_stubs().classify_heap)(handle.bits()) }
            == MoltTypeTag::Exception as u8
    {
        match unsafe { (crate::hooks::hooks_or_stubs().exception_class_borrowed)(handle.bits()) }
            .decode()
        {
            crate::hooks::DecodedHandleResult::Ok(class_bits) => unsafe {
                GLOBAL_BRIDGE.handle_to_borrowed_pyobj(class_bits)
            },
            crate::hooks::DecodedHandleResult::Missing
            | crate::hooks::DecodedHandleResult::Error => return 0,
        }
    } else if crate::abi_types::exc_singleton_name(given).is_some() {
        given
    } else {
        let ob_type = unsafe { (*given).ob_type };
        if !ob_type.is_null() && crate::abi_types::exc_singleton_name(ob_type.cast()).is_some() {
            ob_type.cast::<PyObject>()
        } else {
            given
        }
    };
    // The tuple API is the single physical/runtime authority: its accessors
    // cover ABI-layout and managed tuples without holding a bridge lock across
    // element conversion.
    if unsafe { crate::api::sequences::PyTuple_Check(exc) } != 0 {
        let len = unsafe { crate::api::sequences::PyTuple_GET_SIZE(exc) };
        for index in 0..len {
            let item = unsafe { crate::api::sequences::PyTuple_GET_ITEM(exc, index) };
            if !item.is_null() && given_matches_single(given, item) {
                return 1;
            }
        }
        return 0;
    }
    given_matches_single(given, exc) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyException_SetTraceback(exc: *mut PyObject, tb: *mut PyObject) -> c_int {
    if exc.is_null() {
        unsafe {
            PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
                c"PyException_SetTraceback: NULL exception".as_ptr(),
            )
        };
        return -1;
    }
    if let Some(exception) = GLOBAL_BRIDGE.observed_handle_for_pyobj(exc) {
        let traceback_bits = if tb.is_null() {
            None
        } else {
            unsafe { GLOBAL_BRIDGE.molt_value_for_pyobj(tb) }
        };
        if !tb.is_null() && traceback_bits.is_none() {
            unsafe { set_exception_field_type_error(c"__traceback__ must be a traceback or None") };
            return -1;
        }
        let hooks = crate::hooks::hooks_or_stubs();
        let status = unsafe {
            (hooks.exception_set_field)(
                exception.bits(),
                crate::hooks::ExceptionField::Traceback as u32,
                traceback_bits.unwrap_or(0),
                c_int::from(!tb.is_null()),
            )
        };
        let published = status != 0 || GLOBAL_BRIDGE.refresh_exception_view(exception.bits());
        if let Some(bits) = traceback_bits {
            unsafe { (hooks.dec_ref)(bits) };
        }
        if status == 0 && published {
            return 0;
        }
        if !transfer_runtime_pending_to_current() && unsafe { PyErr_Occurred() }.is_null() {
            unsafe {
                PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_TypeError)
                        .cast::<crate::abi_types::PyObject>(),
                    c"__traceback__ must be a traceback or None".as_ptr(),
                )
            };
        }
        return -1;
    }
    let Some(base) = foreign_exception_layout(exc) else {
        unsafe {
            PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
                c"PyException_SetTraceback: expected an exception instance".as_ptr(),
            )
        };
        return -1;
    };
    let tb = if std::ptr::eq(tb, &raw mut crate::abi_types::Py_None) {
        ptr::null_mut()
    } else {
        tb
    };
    if !tb.is_null()
        && !std::ptr::eq(
            unsafe { (*tb).ob_type },
            &raw mut crate::abi_types::PyTraceBack_Type,
        )
    {
        unsafe { set_exception_field_type_error(c"__traceback__ must be a traceback or None") };
        return -1;
    }
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
    if let Some(exception) = GLOBAL_BRIDGE.observed_handle_for_pyobj(exc) {
        let result = unsafe {
            (crate::hooks::hooks_or_stubs().exception_get_field)(
                exception.bits(),
                crate::hooks::ExceptionField::Traceback as u32,
            )
        };
        return match result.decode() {
            crate::hooks::DecodedHandleResult::Missing => ptr::null_mut(),
            crate::hooks::DecodedHandleResult::Ok(bits) => unsafe {
                GLOBAL_BRIDGE.owned_handle_to_pyobj(bits)
            },
            crate::hooks::DecodedHandleResult::Error => {
                unsafe {
                    set_exception_field_type_error(
                        c"PyException_GetTraceback: expected an exception instance",
                    )
                };
                ptr::null_mut()
            }
        };
    }
    let Some(base) = foreign_exception_layout(exc) else {
        unsafe {
            PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
                c"PyException_GetTraceback: expected an exception instance".as_ptr(),
            )
        };
        return ptr::null_mut();
    };
    let traceback = unsafe { (*base).traceback };
    unsafe { crate::api::refcount::Py_XINCREF(traceback) };
    traceback
}

fn foreign_exception_layout(exc: *mut PyObject) -> Option<*mut PyBaseExceptionObject> {
    if exc.is_null() || GLOBAL_BRIDGE.molt_handle_for_pyobj(exc).is_some() {
        return None;
    }
    let exception_type = unsafe { (*exc).ob_type };
    if exception_type.is_null()
        || unsafe { (*exception_type).tp_flags } & crate::abi_types::Py_TPFLAGS_BASE_EXC_SUBCLASS
            == 0
    {
        return None;
    }
    Some(exc.cast::<PyBaseExceptionObject>())
}

/// Return `StopIteration.value` as a new reference without projecting a native
/// bootstrap exception through generic attribute lookup. Runtime-backed
/// exceptions retain their own attribute authority; native exceptions created
/// by [`molt_native_exception_new`] carry the constructor argument in `args`.
/// CPython's StopIteration constructor accepts zero or one positional value,
/// so those two physical forms are exact and allocation-free here.
pub(crate) unsafe fn stop_iteration_value(exc: *mut PyObject) -> *mut PyObject {
    if exc.is_null() {
        unsafe { PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    if GLOBAL_BRIDGE.molt_handle_for_pyobj(exc).is_some() {
        return unsafe { crate::api::object::PyObject_GetAttrString(exc, c"value".as_ptr()) };
    }
    let Some(base) = foreign_exception_layout(exc) else {
        return unsafe { crate::api::object::PyObject_GetAttrString(exc, c"value".as_ptr()) };
    };
    let args = unsafe { (*base).args };
    if args.is_null() {
        return unsafe { crate::api::object::Py_NewRef(&raw mut crate::abi_types::Py_None) };
    }
    match unsafe { crate::api::sequences::PyTuple_Size(args) } {
        0 => unsafe { crate::api::object::Py_NewRef(&raw mut crate::abi_types::Py_None) },
        1 => {
            let value = unsafe { crate::api::sequences::PyTuple_GetItem(args, 0) };
            unsafe { crate::api::object::Py_XNewRef(value) }
        }
        _ => unsafe { crate::api::object::PyObject_GetAttrString(exc, c"value".as_ptr()) },
    }
}

unsafe fn managed_exception_set_field(
    exc: *mut PyObject,
    field: crate::hooks::ExceptionField,
    value: *mut PyObject,
) -> Option<c_int> {
    let exception = GLOBAL_BRIDGE.observed_handle_for_pyobj(exc)?;
    let c_none = value.is_null()
        || (!matches!(field, crate::hooks::ExceptionField::Args)
            && std::ptr::eq(value, &raw mut crate::abi_types::Py_None));
    let value_bits = if c_none {
        None
    } else {
        unsafe { GLOBAL_BRIDGE.molt_value_for_pyobj(value) }
    };
    if !c_none && value_bits.is_none() {
        return Some(-1);
    }
    let hooks = crate::hooks::hooks_or_stubs();
    let status = unsafe {
        (hooks.exception_set_field)(
            exception.bits(),
            field as u32,
            value_bits.unwrap_or(0),
            c_int::from(!c_none),
        )
    };
    let published = status != 0 || GLOBAL_BRIDGE.refresh_exception_view(exception.bits());
    if let Some(bits) = value_bits {
        unsafe { (hooks.dec_ref)(bits) };
    }
    if status != 0 {
        let _ = transfer_runtime_pending_to_current();
    }
    Some(if published { status } else { -1 })
}

unsafe fn managed_exception_get_field(
    exc: *mut PyObject,
    field: crate::hooks::ExceptionField,
) -> Option<Result<*mut PyObject, ()>> {
    let exception = GLOBAL_BRIDGE.observed_handle_for_pyobj(exc)?;
    let result = unsafe {
        (crate::hooks::hooks_or_stubs().exception_get_field)(exception.bits(), field as u32)
    };
    Some(match result.decode() {
        crate::hooks::DecodedHandleResult::Missing => Ok(ptr::null_mut()),
        crate::hooks::DecodedHandleResult::Ok(bits) => {
            Ok(unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) })
        }
        crate::hooks::DecodedHandleResult::Error => {
            let _ = transfer_runtime_pending_to_current();
            Err(())
        }
    })
}

unsafe fn set_exception_field_type_error(message: &'static CStr) {
    if !transfer_runtime_pending_to_current() {
        unsafe {
            PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
                message.as_ptr(),
            )
        };
    }
}

unsafe fn exception_instance_pointer(value: *mut PyObject) -> bool {
    if value.is_null() {
        return false;
    }
    if let Some(result) =
        unsafe { managed_exception_get_field(value, crate::hooks::ExceptionField::Args) }
    {
        return match result {
            Ok(args) => {
                unsafe { crate::api::refcount::Py_XDECREF(args) };
                true
            }
            Err(()) => false,
        };
    }
    foreign_exception_layout(value).is_some()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyException_SetContext(exc: *mut PyObject, context: *mut PyObject) {
    if exc.is_null() {
        unsafe { crate::api::refcount::Py_XDECREF(context) };
        return;
    }
    if let Some(status) =
        unsafe { managed_exception_set_field(exc, crate::hooks::ExceptionField::Context, context) }
    {
        unsafe { crate::api::refcount::Py_XDECREF(context) };
        if status != 0 {
            unsafe {
                set_exception_field_type_error(c"exception context must be an exception or None")
            };
        }
        return;
    }
    let Some(base) = foreign_exception_layout(exc) else {
        unsafe {
            crate::api::refcount::Py_XDECREF(context);
            set_exception_field_type_error(
                c"PyException_SetContext: expected an exception instance",
            );
        }
        return;
    };
    let context = if std::ptr::eq(context, &raw mut crate::abi_types::Py_None) {
        unsafe { crate::api::refcount::Py_DECREF(context) };
        ptr::null_mut()
    } else {
        context
    };
    if !context.is_null() && !unsafe { exception_instance_pointer(context) } {
        unsafe {
            crate::api::refcount::Py_DECREF(context);
            set_exception_field_type_error(c"exception context must be an exception or None");
        }
        return;
    }
    unsafe {
        let old = (*base).context;
        if old == context {
            crate::api::refcount::Py_XDECREF(context);
        } else {
            (*base).context = context;
            crate::api::refcount::Py_XDECREF(old);
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyException_GetContext(exc: *mut PyObject) -> *mut PyObject {
    if let Some(result) =
        unsafe { managed_exception_get_field(exc, crate::hooks::ExceptionField::Context) }
    {
        return match result {
            Ok(value) => value,
            Err(()) => {
                unsafe {
                    set_exception_field_type_error(
                        c"PyException_GetContext: expected an exception instance",
                    )
                };
                ptr::null_mut()
            }
        };
    }
    let Some(base) = foreign_exception_layout(exc) else {
        unsafe {
            set_exception_field_type_error(
                c"PyException_GetContext: expected an exception instance",
            )
        };
        return ptr::null_mut();
    };
    let context = unsafe { (*base).context };
    unsafe { crate::api::refcount::Py_XINCREF(context) };
    context
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyException_SetCause(exc: *mut PyObject, cause: *mut PyObject) {
    if exc.is_null() {
        unsafe { crate::api::refcount::Py_XDECREF(cause) };
        return;
    }
    if let Some(status) =
        unsafe { managed_exception_set_field(exc, crate::hooks::ExceptionField::Cause, cause) }
    {
        unsafe { crate::api::refcount::Py_XDECREF(cause) };
        if status != 0 {
            unsafe {
                set_exception_field_type_error(c"exception cause must be an exception or None")
            };
        }
        return;
    }
    let Some(base) = foreign_exception_layout(exc) else {
        unsafe {
            crate::api::refcount::Py_XDECREF(cause);
            set_exception_field_type_error(c"PyException_SetCause: expected an exception instance");
        }
        return;
    };
    let cause = if std::ptr::eq(cause, &raw mut crate::abi_types::Py_None) {
        unsafe { crate::api::refcount::Py_DECREF(cause) };
        ptr::null_mut()
    } else {
        cause
    };
    if !cause.is_null() && !unsafe { exception_instance_pointer(cause) } {
        unsafe {
            crate::api::refcount::Py_DECREF(cause);
            set_exception_field_type_error(c"exception cause must be an exception or None");
        }
        return;
    }
    unsafe {
        let old = (*base).cause;
        if old == cause {
            crate::api::refcount::Py_XDECREF(cause);
        } else {
            (*base).cause = cause;
            crate::api::refcount::Py_XDECREF(old);
        }
        (*base).suppress_context = 1;
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyException_GetCause(exc: *mut PyObject) -> *mut PyObject {
    if let Some(result) =
        unsafe { managed_exception_get_field(exc, crate::hooks::ExceptionField::Cause) }
    {
        return match result {
            Ok(value) => value,
            Err(()) => {
                unsafe {
                    set_exception_field_type_error(
                        c"PyException_GetCause: expected an exception instance",
                    )
                };
                ptr::null_mut()
            }
        };
    }
    let Some(base) = foreign_exception_layout(exc) else {
        unsafe {
            set_exception_field_type_error(c"PyException_GetCause: expected an exception instance")
        };
        return ptr::null_mut();
    };
    let cause = unsafe { (*base).cause };
    unsafe { crate::api::refcount::Py_XINCREF(cause) };
    cause
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyException_SetArgs(exc: *mut PyObject, args: *mut PyObject) {
    if let Some(status) =
        unsafe { managed_exception_set_field(exc, crate::hooks::ExceptionField::Args, args) }
    {
        if status != 0 {
            unsafe { set_exception_field_type_error(c"exception args must be a tuple") };
        }
        return;
    }
    let Some(base) = foreign_exception_layout(exc) else {
        unsafe {
            set_exception_field_type_error(c"PyException_SetArgs: expected an exception instance")
        };
        return;
    };
    if args.is_null() || unsafe { crate::api::sequences::PyTuple_Check(args) } == 0 {
        unsafe { set_exception_field_type_error(c"exception args must be a tuple") };
        return;
    }
    unsafe {
        crate::api::refcount::Py_INCREF(args);
        let old = (*base).args;
        (*base).args = args;
        crate::api::refcount::Py_XDECREF(old);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyException_GetArgs(exc: *mut PyObject) -> *mut PyObject {
    if let Some(result) =
        unsafe { managed_exception_get_field(exc, crate::hooks::ExceptionField::Args) }
    {
        return match result {
            Ok(value) => value,
            Err(()) => {
                unsafe {
                    set_exception_field_type_error(
                        c"PyException_GetArgs: expected an exception instance",
                    )
                };
                ptr::null_mut()
            }
        };
    }
    let Some(base) = foreign_exception_layout(exc) else {
        unsafe {
            set_exception_field_type_error(c"PyException_GetArgs: expected an exception instance")
        };
        return ptr::null_mut();
    };
    let args = unsafe { (*base).args };
    unsafe { crate::api::refcount::Py_XINCREF(args) };
    args
}

/// Route warnings through the runtime `warnings.warn` callable so filters,
/// warning-as-error policy, category validation, and stacklevel share the same
/// authority as compiled Python.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_WarnEx(
    category: *mut PyObject,
    message: *const c_char,
    stack_level: Py_ssize_t,
) -> c_int {
    let warnings = unsafe { crate::api::imports::PyImport_ImportModule(c"warnings".as_ptr()) };
    if warnings.is_null() {
        return -1;
    }
    let warn = unsafe { crate::api::object::PyObject_GetAttrString(warnings, c"warn".as_ptr()) };
    unsafe { crate::api::refcount::Py_DECREF(warnings) };
    if warn.is_null() {
        return -1;
    }
    let message = unsafe {
        crate::api::strings::PyUnicode_FromString(if message.is_null() {
            c"".as_ptr()
        } else {
            message
        })
    };
    let category = if category.is_null() {
        unsafe { crate::api::object::Py_NewRef(&raw mut crate::abi_types::Py_None) }
    } else {
        unsafe { crate::api::object::Py_NewRef(category) }
    };
    let stack_level = unsafe { crate::api::numbers::PyLong_FromLongLong(stack_level as i64) };
    let args = unsafe { crate::api::sequences::PyTuple_New(3) };
    if message.is_null() || category.is_null() || stack_level.is_null() || args.is_null() {
        unsafe {
            crate::api::refcount::Py_XDECREF(message);
            crate::api::refcount::Py_XDECREF(category);
            crate::api::refcount::Py_XDECREF(stack_level);
            crate::api::refcount::Py_XDECREF(args);
            crate::api::refcount::Py_DECREF(warn);
        }
        return -1;
    }
    unsafe {
        let _ = crate::api::sequences::PyTuple_SetItem(args, 0, message);
        let _ = crate::api::sequences::PyTuple_SetItem(args, 1, category);
        let _ = crate::api::sequences::PyTuple_SetItem(args, 2, stack_level);
    }
    let result = unsafe { crate::api::object::PyObject_CallObject(warn, args) };
    unsafe {
        crate::api::refcount::Py_DECREF(args);
        crate::api::refcount::Py_DECREF(warn);
    }
    if result.is_null() {
        -1
    } else {
        unsafe { crate::api::refcount::Py_DECREF(result) };
        0
    }
}

/// `PyErr_WriteUnraisable(obj)` consumes the C-API error indicator and forwards
/// its typed payload plus owned runtime context to the runtime's canonical
/// unraisable transaction. This surface has CPython's null `err_msg` contract;
/// version-specific formatted messages are supplied by runtime call sites that
/// model `PyErr_FormatUnraisable` instead.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyErr_WriteUnraisable(obj: *mut PyObject) {
    unsafe { write_unraisable_impl(obj, None) };
}

unsafe fn write_unraisable_impl(obj: *mut PyObject, err_msg: Option<&[u8]>) {
    let Some(state) = take_current_error() else {
        return;
    };
    let owned_bits = |ptr: *mut PyObject| {
        if ptr.is_null() {
            (MoltObject::none().bits(), false)
        } else {
            match unsafe { GLOBAL_BRIDGE.molt_value_for_pyobj(ptr) } {
                Some(bits) => (bits, true),
                None => (MoltObject::none().bits(), false),
            }
        }
    };
    let (type_bits, owns_type) = owned_bits(state.exc_type);
    let (value_bits, owns_value) = owned_bits(state.value);
    let (traceback_bits, owns_traceback) = owned_bits(state.traceback);
    let (context_bits, owns_context) = if obj.is_null() {
        (MoltObject::none().bits(), false)
    } else {
        match unsafe { GLOBAL_BRIDGE.molt_value_for_pyobj(obj) } {
            Some(bits) => (bits, true),
            None => (MoltObject::none().bits(), false),
        }
    };
    let hooks = crate::hooks::hooks_or_stubs();
    unsafe {
        let (err_ptr, err_len, has_err) = err_msg
            .map(|text| (text.as_ptr(), text.len(), 1))
            .unwrap_or((std::ptr::null(), 0, 0));
        (hooks.report_unraisable)(
            context_bits,
            type_bits,
            value_bits,
            traceback_bits,
            std::ptr::null(),
            0,
            err_ptr,
            err_len,
            has_err,
        )
    };
    for (bits, owned) in [
        (context_bits, owns_context),
        (type_bits, owns_type),
        (value_bits, owns_value),
        (traceback_bits, owns_traceback),
    ] {
        if owned {
            unsafe { (hooks.dec_ref)(bits) };
        }
    }
}

/// Variadic C shim entry after CPython-style `%R`/`%T` formatting.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_capi_err_format_unraisable(message: *const u8, len: usize) {
    let formatted = if message.is_null() {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(message, len) }
    };
    unsafe { write_unraisable_impl(std::ptr::null_mut(), Some(formatted)) };
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

unsafe extern "C" {
    /// Variadic formatter authority implemented by `shims/pyarg_variadic.c`.
    pub fn PyUnicode_FromFormat(format: *const c_char, ...) -> *mut PyObject;

    /// Error formatter backed by the same fallible Unicode formatter.
    pub fn PyErr_Format(exc_type: *mut PyObject, format: *const c_char, ...) -> *mut PyObject;
}

/// Formatter-only typed adapter for `%T`/`%N`.
///
/// Both static extension types and Molt-managed type views keep their
/// canonical `module.qualname` spelling in `tp_name`. The C variadic shim
/// cannot safely redeclare the full `PyTypeObject` layout just to reach that
/// field, so it crosses this fixed-arity boundary instead.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_capi_type_fully_qualified_name(
    tp: *mut PyTypeObject,
) -> *mut PyObject {
    if tp.is_null() {
        return ptr::null_mut();
    }
    let name = unsafe { (*tp).tp_name };
    if name.is_null() {
        return ptr::null_mut();
    }
    let bytes = unsafe { CStr::from_ptr(name) }.to_bytes();
    unsafe {
        crate::api::strings::PyUnicode_FromStringAndSize(
            bytes.as_ptr().cast(),
            bytes.len() as Py_ssize_t,
        )
    }
}

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
                let py_ptr = unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) };
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
                let py_ptr = unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) };
                let mut owned_bytes = ptr::null_mut();
                let source = if accepts_bytes && arg_is_bytes(&obj, bits) {
                    py_ptr
                } else if arg_is_str(&obj, bits) {
                    owned_bytes = unsafe {
                        crate::api::strings::PyUnicode_AsEncodedString(
                            py_ptr,
                            if encoding.is_null() {
                                c"utf-8".as_ptr()
                            } else {
                                encoding
                            },
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
                if !has_len
                    && unsafe {
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
                    if !has_len
                        || len_dest.is_null()
                        || unsafe { *len_dest } < required as Py_ssize_t
                    {
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
                        let py_ptr = unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) };
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
                    let py_ptr = unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) };
                    if unsafe { crate::api::buffer::PyObject_GetBuffer(py_ptr, view, PyBUF_SIMPLE) }
                        != 0
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
                    let py_ptr = unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) };
                    write_out!(*mut PyObject, py_ptr);
                } else {
                    unsafe { set_parse_type_error("argument must be bytes") };
                    return 0;
                }
            }
            'Y' => {
                let py_ptr = unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) };
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
                    let py_ptr = unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) };
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
                let py_ptr = unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) };
                if unsafe { crate::api::buffer::PyObject_GetBuffer(py_ptr, view, PyBUF_WRITABLE) }
                    != 0
                {
                    return 0;
                }
            }
            'O' => {
                let py_ptr = unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) };
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
            (&raw mut crate::abi_types::PyExc_OverflowError).cast::<crate::abi_types::PyObject>(),
            cmsg.as_ptr(),
        );
    }
}

/// Set ValueError for an embedded-NUL `s`/`z`/`y` argument.
unsafe fn set_parse_value_error(message: &str) {
    let cmsg = std::ffi::CString::new(message).unwrap_or_default();
    unsafe {
        PyErr_SetString(
            (&raw mut crate::abi_types::PyExc_ValueError).cast::<crate::abi_types::PyObject>(),
            cmsg.as_ptr(),
        );
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
        PyErr_SetString(
            (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
            cmsg.as_ptr(),
        );
    }
}

/// Set a SystemError for an unrecognized/malformed format unit, matching
/// CPython's handling of a bad format string in PyArg_ParseTuple.
unsafe fn set_parse_format_error() {
    unsafe {
        PyErr_SetString(
            (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
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
            .filter_map(|i| match unsafe { (h.list_item)(bits, i) }.decode() {
                crate::hooks::DecodedHandleResult::Ok(item) => Some(item),
                crate::hooks::DecodedHandleResult::Missing
                | crate::hooks::DecodedHandleResult::Error => None,
            })
            .collect();
    }
    (0..len)
        .filter_map(|i| match unsafe { (h.tuple_item)(bits, i) }.decode() {
            crate::hooks::DecodedHandleResult::Ok(item) => Some(item),
            crate::hooks::DecodedHandleResult::Missing
            | crate::hooks::DecodedHandleResult::Error => None,
        })
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
