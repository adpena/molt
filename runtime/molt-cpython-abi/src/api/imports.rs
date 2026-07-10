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
use once_cell::sync::OnceCell;
use std::ffi::CStr;
use std::os::raw::{c_char, c_int};
use std::ptr;

static MODULE_DICT: OnceCell<usize> = OnceCell::new();

unsafe fn set_import_unavailable(_name: *const c_char) {
    unsafe {
        crate::api::errors::PyErr_SetString(
            &raw mut crate::abi_types::PyExc_ImportError,
            c"import API is not available in standalone molt-cpython-abi".as_ptr(),
        );
    }
}

/// Import the dotted module path in `name` through the runtime hook and
/// return an owned bridge `PyObject` for the imported module, or null on
/// failure.
///
/// The C caller contract (Python/import.c) is that a NULL return ALWAYS
/// leaves `PyErr_Occurred()` non-NULL — extensions branch on it, and CPython
/// itself raises "returned NULL without setting an exception" otherwise. The
/// ABI-side `PyErr` reads only `CURRENT_EXC`, never the runtime's pending
/// exception, so the hook-failure branch must mirror an ImportError here.
/// To avoid masking the runtime's precise diagnostics (module-init drains the
/// ABI error first), the mirror is set only when the ABI error state is clear
/// and names the failing module explicitly.
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
        if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
            let message = format!(
                "import of '{}' failed (runtime import error pending)",
                String::from_utf8_lossy(name)
            );
            if let Ok(cmessage) = std::ffi::CString::new(message) {
                unsafe {
                    crate::api::errors::PyErr_SetString(
                        &raw mut crate::abi_types::PyExc_ImportError,
                        cmessage.as_ptr(),
                    );
                }
            }
        }
        return ptr::null_mut();
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(module_bits) }
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

/// Resolve the runtime's real `sys.modules` dict handle, or 0 when the sys
/// module (or hooks) are unavailable.
unsafe fn sys_modules_bits() -> u64 {
    let Some(h) = hooks::hooks() else {
        return 0;
    };
    unsafe { (h.sys_get_object_borrowed)(b"modules".as_ptr(), b"modules".len()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyImport_AddModule(name: *const c_char) -> *mut PyObject {
    // CPython Python/import.c: return the module registered in sys.modules if
    // present, else insert a NEW EMPTY module (no import is run) and return a
    // BORROWED reference. Backed by the runtime's real sys.modules + module
    // allocator hooks; without hooks this stays honestly fail-closed.
    if name.is_null() {
        unsafe { set_import_unavailable(name) };
        return ptr::null_mut();
    }
    let Some(h) = hooks::hooks() else {
        unsafe { set_import_unavailable(name) };
        return ptr::null_mut();
    };
    let modules_bits = unsafe { sys_modules_bits() };
    if modules_bits == 0 {
        unsafe { set_import_unavailable(name) };
        return ptr::null_mut();
    }
    let name_bytes = unsafe { CStr::from_ptr(name).to_bytes() };
    let key_bits = unsafe { (h.alloc_str)(name_bytes.as_ptr(), name_bytes.len()) };
    if key_bits == 0 {
        return unsafe { crate::api::errors::PyErr_NoMemory() };
    }
    let existing = unsafe { (h.dict_get)(modules_bits, key_bits) };
    if existing != 0 {
        unsafe { (h.dec_ref)(key_bits) };
        return unsafe { GLOBAL_BRIDGE.lock().handle_to_borrowed_pyobj(existing) };
    }
    // Absent: create an empty module and register it in sys.modules.
    let module_bits = unsafe { (h.alloc_module)(name_bytes.as_ptr(), name_bytes.len()) };
    if module_bits == 0 {
        unsafe { (h.dec_ref)(key_bits) };
        return unsafe { crate::api::errors::PyErr_NoMemory() };
    }
    unsafe { (h.dict_set)(modules_bits, key_bits, module_bits) };
    unsafe { (h.dec_ref)(key_bits) };
    unsafe { GLOBAL_BRIDGE.lock().handle_to_borrowed_pyobj(module_bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyImport_GetModuleDict() -> *mut PyObject {
    // CPython returns the interpreter's REAL modules dict (== sys.modules).
    // Route through the runtime's sys module so PyDict_GetItemString(
    // GetModuleDict(), name) sees genuinely imported modules; the detached
    // OnceCell dict remains only as the hook-less fallback.
    let bits = unsafe { sys_modules_bits() };
    if bits != 0 {
        return unsafe { GLOBAL_BRIDGE.lock().handle_to_borrowed_pyobj(bits) };
    }
    let raw = MODULE_DICT.get_or_init(|| unsafe { crate::api::mapping::PyDict_New() as usize });
    *raw as *mut PyObject
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
                &raw mut crate::abi_types::PyExc_KeyError,
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
                        &raw mut crate::abi_types::PyExc_KeyError,
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
                                &raw mut crate::abi_types::PyExc_ImportError,
                                c"attempted relative import with no known parent package"
                                    .as_ptr(),
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
                &raw mut crate::abi_types::PyExc_ImportError,
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
                        &raw mut crate::abi_types::PyExc_ImportError,
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
    if fromlist_empty {
        if let Some(dot) = name.iter().position(|byte| *byte == b'.') {
            let root = unsafe { import_module_bytes(&name[..dot]) };
            unsafe { crate::api::refcount::Py_DECREF(leaf) };
            return root;
        }
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
