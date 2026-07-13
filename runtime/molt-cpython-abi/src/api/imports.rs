//! Import API entrypoints for C extensions.
//!
//! These are ABI-level hooks. Package custody lives in the Molt
//! runtime/import pipeline, so absolute imports route through the
//! `import_module` runtime hook (package custody, static extension
//! registry, sys.modules cache). Paths the runtime cannot own — relative
//! imports without package context, or genuinely standalone use with no
//! registered hooks — fail closed with a Python exception.

use crate::abi_types::PyObject;
use crate::bridge::GLOBAL_BRIDGE;
use crate::hooks;
use std::ffi::CStr;
use std::os::raw::{c_char, c_int};
use std::ptr;

unsafe fn set_import_unavailable(_name: *const c_char) {
    unsafe {
        crate::api::errors::PyErr_SetString(
            (&raw mut crate::abi_types::PyExc_ImportError).cast::<crate::abi_types::PyObject>(),
            c"import API is not available in standalone molt-cpython-abi".as_ptr(),
        );
    }
}

/// Import the dotted module path in `name` through the runtime hook and
/// return an owned bridge `PyObject` for the imported module, or null on
/// failure.
///
/// A failed runtime import can report its exception through either the ABI
/// `PyErr` state or the runtime's unified pending-exception state. The latter
/// is the authority for re-entrant imports executed by static extension
/// `Py_mod_exec` slots. Never synthesize a second ABI `ImportError` while that
/// runtime exception is pending: static-init diagnostics drain ABI state first,
/// so a mirror would hide the real exception. A synthetic error is reserved for
/// the contract violation where neither exception channel was set.
unsafe fn import_module_bytes(name: &[u8]) -> *mut PyObject {
    if name.is_empty() {
        unsafe { set_import_unavailable(ptr::null()) };
        return ptr::null_mut();
    }
    let Some(h) = hooks::hooks() else {
        unsafe { set_import_unavailable(ptr::null()) };
        return ptr::null_mut();
    };
    let module_bits = unsafe { (h.import_module)(name.as_ptr(), name.len()) };
    if module_bits == 0 {
        let abi_error_clear = unsafe { crate::api::errors::PyErr_Occurred() }.is_null();
        let runtime_error_clear = unsafe { (h.exception_pending)() } == 0;
        if abi_error_clear && runtime_error_clear {
            let message = format!(
                "import of '{}' failed without setting an exception",
                String::from_utf8_lossy(name)
            );
            if let Ok(cmessage) = std::ffi::CString::new(message) {
                unsafe {
                    crate::api::errors::PyErr_SetString(
                        (&raw mut crate::abi_types::PyExc_ImportError)
                            .cast::<crate::abi_types::PyObject>(),
                        cmessage.as_ptr(),
                    );
                }
            }
        }
        return ptr::null_mut();
    }
    unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(module_bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyImport_ImportModule(name: *const c_char) -> *mut PyObject {
    if name.is_null() {
        unsafe { set_import_unavailable(name) };
        return ptr::null_mut();
    }
    let name_bytes = unsafe { CStr::from_ptr(name).to_bytes() };
    crate::capi_trace::trace_call(
        "PyImport_ImportModule",
        Some(&String::from_utf8_lossy(name_bytes)),
    );
    unsafe { import_module_bytes(name_bytes) }
}

/// Resolve the runtime's real `sys.modules` dict without collapsing an
/// execution error into an ordinary missing value.
unsafe fn sys_modules_result() -> hooks::DecodedHandleResult {
    let Some(h) = hooks::hooks() else {
        return hooks::DecodedHandleResult::Missing;
    };
    decode_borrowed_hook_result(unsafe {
        (h.sys_get_object_borrowed)(b"modules".as_ptr(), b"modules".len())
    })
}

/// Decode a borrowed runtime-hook result without allowing a non-error status
/// to escape alongside an already-pending exception. The runtime and C
/// indicators are one logical error channel at this boundary.
pub(crate) fn decode_borrowed_hook_result(
    result: hooks::BorrowedHandleResult,
) -> hooks::DecodedHandleResult {
    let decoded = result.decode();
    if crate::api::errors::transfer_runtime_pending_to_current()
        || !unsafe { crate::api::errors::PyErr_Occurred() }.is_null()
    {
        hooks::DecodedHandleResult::Error
    } else {
        decoded
    }
}

pub(crate) fn propagate_hook_error(message: &'static CStr) {
    if crate::api::errors::transfer_runtime_pending_to_current()
        || !unsafe { crate::api::errors::PyErr_Occurred() }.is_null()
    {
        return;
    }
    unsafe {
        crate::api::errors::PyErr_SetString(
            (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
            message.as_ptr(),
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyImport_AddModule(name: *const c_char) -> *mut PyObject {
    // CPython Python/import.c: under one sys.modules critical section, return
    // the existing value only when it is a module; otherwise create and
    // publish a new empty module. The runtime hook owns that entire
    // get/type-check/create/set transaction, including replacement of a
    // non-module value. This entrypoint validates the UTF-8 C name, preserves
    // the exact error indicator, and projects the borrowed result.
    if name.is_null() {
        unsafe { set_import_unavailable(name) };
        return ptr::null_mut();
    }
    let name_bytes = unsafe { CStr::from_ptr(name).to_bytes() };
    if std::str::from_utf8(name_bytes).is_err() {
        // CPython first calls PyUnicode_FromString, so malformed UTF-8 must
        // raise the same UnicodeDecodeError before the transaction is entered.
        // This second decode only runs on the cold error path.
        let decoded = unsafe { crate::api::strings::PyUnicode_FromString(name) };
        unsafe { crate::api::refcount::Py_XDECREF(decoded) };
        if !decoded.is_null() {
            propagate_hook_error(c"invalid UTF-8 module name decoded without an exception");
        }
        return ptr::null_mut();
    }
    let Some(h) = hooks::hooks() else {
        unsafe { set_import_unavailable(name) };
        return ptr::null_mut();
    };
    match decode_borrowed_hook_result(unsafe {
        (h.import_add_module_borrowed)(name_bytes.as_ptr(), name_bytes.len())
    }) {
        hooks::DecodedHandleResult::Ok(module_bits) => unsafe {
            GLOBAL_BRIDGE.handle_to_borrowed_pyobj(module_bits)
        },
        hooks::DecodedHandleResult::Error => {
            propagate_hook_error(
                c"atomic sys.modules publication failed without setting an exception",
            );
            ptr::null_mut()
        }
        hooks::DecodedHandleResult::Missing => {
            propagate_hook_error(
                c"atomic sys.modules publication returned no module without setting an exception",
            );
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyImport_GetModuleDict() -> *mut PyObject {
    // CPython returns the interpreter's REAL modules dict (== sys.modules).
    // There is one modules-dict authority: the runtime's sys.modules. A
    // detached hook-less dictionary would make imports and this API disagree.
    match unsafe { sys_modules_result() } {
        hooks::DecodedHandleResult::Ok(bits) => {
            return unsafe { GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits) };
        }
        hooks::DecodedHandleResult::Missing if hooks::hooks().is_none() => unsafe {
            set_import_unavailable(ptr::null())
        },
        hooks::DecodedHandleResult::Missing => {
            propagate_hook_error(c"runtime has no sys.modules dictionary")
        }
        hooks::DecodedHandleResult::Error => {
            propagate_hook_error(c"sys.modules lookup failed without setting an exception")
        }
    }
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyImport_GetModule(name: *mut PyObject) -> *mut PyObject {
    let modules = unsafe { PyImport_GetModuleDict() };
    if modules.is_null() {
        return ptr::null_mut();
    }
    let module = unsafe { crate::api::mapping::PyDict_GetItemWithError(modules, name) };
    unsafe { crate::api::object::Py_XNewRef(module) }
}

/// Read a `str`-valued item from a `globals` dict by C-string key, into an
/// owned byte vector (`None` on absence, non-str, or a non-dict `globals`).
unsafe fn dict_get_string_bytes(globals: *mut PyObject, key: &CStr) -> Option<Vec<u8>> {
    if globals.is_null() {
        return None;
    }
    let value = unsafe { crate::api::mapping::PyDict_GetItemString(globals, key.as_ptr()) };
    if value.is_null() || std::ptr::eq(value, &raw mut crate::abi_types::Py_None) {
        return None;
    }
    let mut size: crate::abi_types::Py_ssize_t = 0;
    let ptr = unsafe { crate::api::strings::PyUnicode_AsUTF8AndSize(value, &raw mut size) };
    if ptr.is_null() || size < 0 {
        // Not a str (or conversion failed): clear so a stray TypeError from
        // the probe doesn't leak into the caller's import.
        unsafe { crate::api::errors::PyErr_Clear() };
        return None;
    }
    Some(unsafe { std::slice::from_raw_parts(ptr as *const u8, size as usize) }.to_vec())
}

/// CPython `Python/import.c` `resolve_name`: combine `level` leading dots with
/// the caller's package context (read from `globals`) to compute the absolute
/// module name for a relative import (`from . import x` / `from .. import x`).
/// Faithful to the dominant path — `__package__` when present, else derived
/// from `__name__` (minus its trailing component unless `__path__` marks the
/// caller as a package's own `__init__`). The `__spec__.parent` agreement
/// check is not reproduced: in CPython it only emits a DeprecationWarning on
/// mismatch and never changes the resolved name.
unsafe fn resolve_relative_name(
    name: &[u8],
    globals: *mut PyObject,
    level: c_int,
) -> Option<Vec<u8>> {
    if globals.is_null() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_KeyError).cast::<crate::abi_types::PyObject>(),
                c"'__name__' not in globals".as_ptr(),
            );
        }
        return None;
    }
    let mut package = match unsafe { dict_get_string_bytes(globals, c"__package__") } {
        Some(bytes) => bytes,
        None => {
            let Some(mut name_bytes) = (unsafe { dict_get_string_bytes(globals, c"__name__") })
            else {
                unsafe {
                    crate::api::errors::PyErr_SetString(
                        (&raw mut crate::abi_types::PyExc_KeyError)
                            .cast::<crate::abi_types::PyObject>(),
                        c"'__name__' not in globals".as_ptr(),
                    );
                }
                return None;
            };
            let has_path = !unsafe {
                crate::api::mapping::PyDict_GetItemString(globals, c"__path__".as_ptr())
            }
            .is_null();
            if !has_path {
                // A regular (non-package) module: its parent package is
                // `__name__` minus the trailing dotted component.
                match name_bytes.iter().rposition(|&b| b == b'.') {
                    Some(dot) => name_bytes.truncate(dot),
                    None => {
                        unsafe {
                            crate::api::errors::PyErr_SetString(
                                (&raw mut crate::abi_types::PyExc_ImportError)
                                    .cast::<crate::abi_types::PyObject>(),
                                c"attempted relative import with no known parent package".as_ptr(),
                            );
                        }
                        return None;
                    }
                }
            }
            name_bytes
        }
    };
    if package.is_empty() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_ImportError).cast::<crate::abi_types::PyObject>(),
                c"attempted relative import with no known parent package".as_ptr(),
            );
        }
        return None;
    }
    // Walk `level - 1` additional dotted components upward (level==1 keeps
    // `package` as-is: a single leading dot means "the current package").
    let mut last_dot = package.len();
    for _ in 1..level {
        match package[..last_dot].iter().rposition(|&b| b == b'.') {
            Some(dot) => last_dot = dot,
            None => {
                unsafe {
                    crate::api::errors::PyErr_SetString(
                        (&raw mut crate::abi_types::PyExc_ImportError)
                            .cast::<crate::abi_types::PyObject>(),
                        c"attempted relative import beyond top-level package".as_ptr(),
                    );
                }
                return None;
            }
        }
    }
    package.truncate(last_dot);
    if name.is_empty() {
        return Some(package);
    }
    package.push(b'.');
    package.extend_from_slice(name);
    Some(package)
}

unsafe fn import_module_level_bytes(
    name: &[u8],
    globals: *mut PyObject,
    fromlist: *mut PyObject,
    level: c_int,
) -> *mut PyObject {
    let name = if level != 0 {
        // CPython resolves the absolute name via `resolve_name` before
        // importing; the previous body failed every relative import
        // regardless of package context, even though `globals` (carrying
        // `__package__`/`__name__`/`__path__`) is available at every call site.
        match unsafe { resolve_relative_name(name, globals, level) } {
            Some(resolved) => resolved,
            None => return ptr::null_mut(),
        }
    } else {
        name.to_vec()
    };
    let name = name.as_slice();
    let fromlist_empty =
        if fromlist.is_null() || std::ptr::eq(fromlist, &raw mut crate::abi_types::Py_None) {
            true
        } else {
            match unsafe { crate::api::object::PyObject_IsTrue(fromlist) } {
                -1 => return ptr::null_mut(),
                0 => true,
                _ => false,
            }
        };
    // Import the full dotted chain first; __import__ with an empty fromlist
    // then binds the ROOT package, otherwise the leaf module.
    let leaf = unsafe { import_module_bytes(name) };
    if leaf.is_null() {
        return ptr::null_mut();
    }
    if fromlist_empty && let Some(dot) = name.iter().position(|byte| *byte == b'.') {
        let root = unsafe { import_module_bytes(&name[..dot]) };
        unsafe { crate::api::refcount::Py_DECREF(leaf) };
        return root;
    }
    leaf
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyImport_ImportModuleLevel(
    name: *const c_char,
    globals: *mut PyObject,
    _locals: *mut PyObject,
    fromlist: *mut PyObject,
    level: c_int,
) -> *mut PyObject {
    if name.is_null() {
        unsafe { set_import_unavailable(name) };
        return ptr::null_mut();
    }
    let name_bytes = unsafe { CStr::from_ptr(name).to_bytes() };
    unsafe { import_module_level_bytes(name_bytes, globals, fromlist, level) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyImport_ImportModuleLevelObject(
    name: *mut PyObject,
    globals: *mut PyObject,
    _locals: *mut PyObject,
    fromlist: *mut PyObject,
    level: c_int,
) -> *mut PyObject {
    if name.is_null() {
        unsafe { set_import_unavailable(ptr::null()) };
        return ptr::null_mut();
    }
    let name_ptr = unsafe { crate::api::strings::PyUnicode_AsUTF8(name) };
    if name_ptr.is_null() {
        return ptr::null_mut();
    }
    let name_bytes = unsafe { CStr::from_ptr(name_ptr).to_bytes() };
    unsafe { import_module_level_bytes(name_bytes, globals, fromlist, level) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyImport_Import(name: *mut PyObject) -> *mut PyObject {
    if name.is_null() {
        unsafe { set_import_unavailable(ptr::null()) };
        return ptr::null_mut();
    }
    let name_ptr = unsafe { crate::api::strings::PyUnicode_AsUTF8(name) };
    if name_ptr.is_null() {
        return ptr::null_mut();
    }
    unsafe { PyImport_ImportModule(name_ptr) }
}
