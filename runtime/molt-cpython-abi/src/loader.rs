//! Explicit CPython-ABI bridge loader — dlopen an allowlisted `.so` and call
//! `PyInit_<name>()`.
//!
//! ## What this does
//!
//! This module is an explicit bridge lane, not Molt's primary extension
//! strategy. The primary path is recompiling extensions against `libmolt`.
//! When the bridge feature is intentionally enabled, callers provide an
//! allowlisted extension directory through `MOLT_EXTENSION_PATH`; this loader
//! never probes host Python or system site-packages.
//!
//! Execution flow:
//! 1. `load_cpython_extension(path, "numpy")` opens the `.so` via libloading.
//! 2. Resolves `PyInit_numpy` symbol.
//! 3. Calls `PyInit_numpy()` — this runs the extension's init code, which
//!    calls back into `PyModule_Create2`, `PyType_Ready`, etc. All of those
//!    calls land in our ABI shim implementations.
//! 4. Wraps the returned `*mut PyObject` (a bridge-managed module) as a
//!    Molt module handle.
//! 5. Returns the Molt module to the import system.
//!
//! ## SIMD / performance
//!
//! Hot path: argument marshalling in `PyArg_ParseTuple`. Optimized via the
//! SIMD type-tag lookup in `bridge.rs` (SSE4.1 / NEON).
//!
//! The dlopen itself is not on the hot path — it happens once at import time.

#![cfg(all(feature = "extension-loader", not(target_arch = "wasm32")))]

use crate::abi_types::PyObject;
use crate::bridge::GLOBAL_BRIDGE;
use libloading::{Library, Symbol};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::path::Path;

static LOADED_EXTENSION_LIBRARIES: Lazy<Mutex<Vec<Library>>> = Lazy::new(|| Mutex::new(Vec::new()));

/// Error type for extension loading failures.
#[derive(Debug)]
pub enum LoadError {
    /// `dlopen` failed — library not found or not a valid shared library.
    DlopenFailed(libloading::Error),
    /// `PyInit_<name>` symbol not found in the library.
    InitSymbolMissing { lib_path: String, symbol: String },
    /// `PyInit_<name>()` returned NULL — initialization error.
    InitReturnedNull { name: String },
    /// `PyInit_<name>()` returned an object that is not known to the bridge.
    InitReturnedUnmappedObject { name: String },
    /// No explicit extension artifact was found for this module.
    ExtensionNotFound { name: String },
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DlopenFailed(e) => write!(f, "dlopen failed: {e}"),
            Self::InitSymbolMissing { lib_path, symbol } => {
                write!(f, "{symbol} not found in {lib_path}")
            }
            Self::InitReturnedNull { name } => {
                write!(f, "PyInit_{name}() returned NULL (module init error)")
            }
            Self::InitReturnedUnmappedObject { name } => {
                write!(
                    f,
                    "PyInit_{name}() returned an object outside the libmolt bridge registry"
                )
            }
            Self::ExtensionNotFound { name } => {
                write!(
                    f,
                    "extension {name} not found in explicit MOLT_EXTENSION_PATH search roots"
                )
            }
        }
    }
}

/// Load a CPython C extension from `path` and initialize module `name`.
///
/// # Safety
/// - `path` must point to a valid CPython 3.12–compatible `.so`.
/// - The extension must not make assumptions about CPython's memory layout
///   beyond what our ABI shim provides.
/// - Must be called after `init_static_types()` and `init_tag_table()`.
pub unsafe fn load_cpython_extension(path: &Path, name: &str) -> Result<u64, LoadError> {
    // Ensure ABI is initialized.
    unsafe { crate::abi_types::init_static_types() };
    crate::bridge::init_tag_table();

    // dlopen the .so
    let lib = unsafe { Library::new(path) }.map_err(LoadError::DlopenFailed)?;

    // Locate PyInit_<name> entry point.
    let symbol_name = format!("PyInit_{name}");
    let init_fn: Symbol<unsafe extern "C" fn() -> *mut PyObject> = unsafe {
        lib.get(symbol_name.as_bytes())
            .map_err(|_| LoadError::InitSymbolMissing {
                lib_path: path.display().to_string(),
                symbol: symbol_name.clone(),
            })?
    };

    // Call the init function. This runs the extension's module setup code,
    // which calls back into our PyModule_Create2, PyType_Ready, etc.
    let raw_init = unsafe { init_fn() };
    if raw_init.is_null() {
        return Err(LoadError::InitReturnedNull {
            name: name.to_owned(),
        });
    }

    // PEP 489 multi-phase init: `PyInit_<name>()` may return a `PyModuleDef*`
    // (produced by `PyModuleDef_Init`) instead of a fully-created module. Drive
    // the create/exec slots (`PyModule_FromDefAndSpec`) if so; single-phase C
    // extensions return the module directly and pass through unchanged.
    let module_ptr = unsafe { drive_multiphase_if_needed(raw_init, name, path)? };

    // Convert the returned `*mut PyObject` to a Molt handle.
    //
    // Bridge-minted PyObject blocks carry the original Molt handle bits in a
    // trailing u64 field immediately after the `PyObject` header, so we can
    // recover them without sharing any in-memory state across rlib / dylib
    // copies of the bridge.  We validate the recovered bits by asking the
    // runtime hook to classify them — only a `MoltTypeTag::Module` is a
    // legitimate result for a `PyInit_<name>()` return value.
    let molt_bits = {
        let mut bridge = GLOBAL_BRIDGE.lock();
        let candidate_bits = match bridge.molt_handle_for_pyobj(module_ptr) {
            Some(value) => value.bits(),
            None => unsafe { bridge.molt_value_for_pyobj(module_ptr) }.ok_or_else(|| {
                LoadError::InitReturnedUnmappedObject {
                    name: name.to_owned(),
                }
            })?,
        };
        drop(bridge);
        let h = crate::hooks::hooks_or_stubs();
        let tag = unsafe { (h.classify_heap)(candidate_bits) };
        if tag != crate::abi_types::MoltTypeTag::Module as u8 {
            return Err(LoadError::InitReturnedUnmappedObject {
                name: name.to_owned(),
            });
        }
        candidate_bits
    };

    // Keep the library alive for the process lifetime; extension code/data may
    // be referenced by module objects and function pointers after init.
    LOADED_EXTENSION_LIBRARIES.lock().push(lib);

    Ok(molt_bits)
}

/// If `raw` is a `PyModuleDef` returned by a PEP 489 multi-phase `PyInit_<name>`,
/// drive `PyModule_FromDefAndSpec` (which executes the `Py_mod_create` and
/// `Py_mod_exec` slots) and return the resulting module; otherwise return `raw`
/// unchanged (single-phase init already produced the module).
///
/// CPython's import machinery makes exactly this distinction by the returned
/// object's type (`ob_type == &PyModuleDef_Type`). Modern Cython extensions
/// (e.g. scipy's `_ni_label` / `_nd_image`) use multi-phase init; hand-written
/// single-phase C extensions (e.g. numpy's `_multiarray_umath`) do not. Without
/// this branch a multi-phase `PyInit` return is a bare `PyModuleDef` that no
/// Molt handle maps to (`InitReturnedUnmappedObject`), blocking the whole
/// multi-phase extension class.
///
/// # Safety
/// `raw` must be the non-null pointer returned by an extension `PyInit_<name>()`.
unsafe fn drive_multiphase_if_needed(
    raw: *mut PyObject,
    name: &str,
    origin: &Path,
) -> Result<*mut PyObject, LoadError> {
    let ob_type = unsafe { (*raw).ob_type };
    let is_moduledef =
        !ob_type.is_null() && std::ptr::eq(ob_type, &raw mut crate::abi_types::PyModuleDef_Type);
    if !is_moduledef {
        return Ok(raw);
    }
    let def = raw.cast::<crate::abi_types::PyModuleDef>();
    // A minimal module spec carrying `.name`: a `Py_mod_create` slot reads
    // `spec.name`; exec-only modules ignore it. Any object exposing `name`
    // satisfies the create-slot contract.
    let spec = unsafe { build_min_module_spec(name, origin) };
    let module = unsafe { crate::api::modules::PyModule_FromDefAndSpec(def, spec) };
    if !spec.is_null() {
        unsafe { crate::api::refcount::Py_DECREF(spec) };
    }
    if module.is_null() {
        return Err(LoadError::InitReturnedNull {
            name: name.to_owned(),
        });
    }
    Ok(module)
}

/// Build a throwaway object serving as the PEP 489 module spec. It exposes the
/// attributes a multi-phase init reads: `name` (the dotted module name),
/// `loader` (None — Cython's `__Pyx_copy_spec_to_module` copies `spec.loader`
/// to `__loader__` with `allow_missing = 0`, i.e. the attribute must be
/// present), `origin` (the `.so` path — copied to `__file__`), and `parent`
/// (empty — copied to `__package__`). This mirrors what importlib's real
/// `ModuleSpec` supplies in a stock CPython import; without `loader` present,
/// Cython bails during exec. Returns null on allocation failure.
///
/// # Safety
/// Calls the ABI string/module/object entry points; must run after ABI init.
unsafe fn build_min_module_spec(name: &str, origin: &Path) -> *mut PyObject {
    let Ok(cname) = std::ffi::CString::new(name) else {
        return std::ptr::null_mut();
    };
    let spec = unsafe { crate::api::modules::PyModule_New(cname.as_ptr()) };
    if spec.is_null() {
        return std::ptr::null_mut();
    }
    unsafe fn set_owned(spec: *mut PyObject, key: &std::ffi::CStr, value: *mut PyObject) {
        if value.is_null() {
            return;
        }
        let rc =
            unsafe { crate::api::object::PyObject_SetAttrString(spec, key.as_ptr(), value) };
        unsafe { crate::api::refcount::Py_DECREF(value) };
        if rc != 0 {
            unsafe { crate::api::errors::PyErr_Clear() };
        }
    }
    // name -> spec.name
    let name_obj = unsafe { crate::api::strings::PyUnicode_FromString(cname.as_ptr()) };
    unsafe { set_owned(spec, c"name", name_obj) };
    // loader -> spec.loader = None (required-present by Cython; None is allowed)
    let none = &raw mut crate::abi_types::Py_None;
    let rc = unsafe { crate::api::object::PyObject_SetAttrString(spec, c"loader".as_ptr(), none) };
    if rc != 0 {
        unsafe { crate::api::errors::PyErr_Clear() };
    }
    // origin -> spec.origin = <.so path> (copied to __file__)
    if let Ok(corigin) = std::ffi::CString::new(origin.to_string_lossy().into_owned()) {
        let origin_obj =
            unsafe { crate::api::strings::PyUnicode_FromString(corigin.as_ptr()) };
        unsafe { set_owned(spec, c"origin", origin_obj) };
    }
    // parent -> spec.parent = "" (copied to __package__)
    let parent_obj = unsafe { crate::api::strings::PyUnicode_FromString(c"".as_ptr()) };
    unsafe { set_owned(spec, c"parent", parent_obj) };
    // submodule_search_locations -> __path__ = None (None => not a package)
    let none = &raw mut crate::abi_types::Py_None;
    let rc = unsafe {
        crate::api::object::PyObject_SetAttrString(
            spec,
            c"submodule_search_locations".as_ptr(),
            none,
        )
    };
    if rc != 0 {
        unsafe { crate::api::errors::PyErr_Clear() };
    }
    // cached -> spec.cached = None (copied to __cached__)
    let rc =
        unsafe { crate::api::object::PyObject_SetAttrString(spec, c"cached".as_ptr(), none) };
    if rc != 0 {
        unsafe { crate::api::errors::PyErr_Clear() };
    }
    spec
}

/// Search standard CPython extension paths for `name`.
///
/// This bridge loader intentionally searches only explicit
/// `MOLT_EXTENSION_PATH` roots. It does not inspect host Python, site-packages,
/// or system lib-dynload directories.
pub fn find_extension(name: &str) -> Option<std::path::PathBuf> {
    let candidates = extension_candidate_paths(name);
    candidates.into_iter().find(|p| p.exists())
}

fn extension_candidate_paths(name: &str) -> Vec<std::path::PathBuf> {
    use std::path::PathBuf;

    let mut out = Vec::new();

    if let Ok(env_path) = std::env::var("MOLT_EXTENSION_PATH") {
        for dir in env_path.split(':') {
            if dir.is_empty() {
                continue;
            }
            let dir = PathBuf::from(dir);
            // Try common suffixes.
            for suffix in cpython_so_suffixes(name) {
                out.push(dir.join(&suffix));
            }
        }
    }

    out
}

fn cpython_so_suffixes(name: &str) -> Vec<String> {
    // Order matches CPython's import machinery search order.
    vec![
        // CPython 3.12 ABI tag — most common on modern systems.
        #[cfg(target_os = "macos")]
        format!("{name}.cpython-312-darwin.so"),
        #[cfg(all(target_os = "linux", not(target_arch = "aarch64")))]
        format!("{name}.cpython-312-x86_64-linux-gnu.so"),
        #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
        format!("{name}.cpython-312-aarch64-linux-gnu.so"),
        // Stable ABI (abi3)
        format!("{name}.abi3.so"),
        // Bare name (rare, non-versioned)
        format!("{name}.so"),
    ]
    .into_iter()
    .filter(|s| !s.is_empty())
    .collect()
}

/// High-level convenience: find and load an extension by module name.
///
/// # Safety
/// Same requirements as `load_cpython_extension`.
pub unsafe fn import_cpython_extension(name: &str) -> Result<u64, LoadError> {
    let path = find_extension(name).ok_or_else(|| LoadError::ExtensionNotFound {
        name: name.to_owned(),
    })?;
    unsafe { load_cpython_extension(&path, name) }
}

#[cfg(test)]
mod tests {
    use super::{LoadError, extension_candidate_paths};

    #[test]
    fn extension_search_uses_only_explicit_env_roots() {
        let prior = std::env::var("MOLT_EXTENSION_PATH").ok();
        unsafe {
            std::env::set_var("MOLT_EXTENSION_PATH", "/explicit/a:/explicit/b");
        }

        let candidates = extension_candidate_paths("demoext");

        match prior {
            Some(value) => unsafe {
                std::env::set_var("MOLT_EXTENSION_PATH", value);
            },
            None => unsafe {
                std::env::remove_var("MOLT_EXTENSION_PATH");
            },
        }

        assert!(!candidates.is_empty());
        assert!(candidates.iter().all(|path| {
            let text = path.to_string_lossy();
            text.starts_with("/explicit/a/") || text.starts_with("/explicit/b/")
        }));
    }

    #[test]
    fn extension_not_found_error_is_explicit() {
        let error = LoadError::ExtensionNotFound {
            name: "demoext".to_string(),
        };
        assert!(error.to_string().contains("MOLT_EXTENSION_PATH"));
    }
}
