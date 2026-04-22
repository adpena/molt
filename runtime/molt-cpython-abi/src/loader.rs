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
    let module_ptr = unsafe { init_fn() };
    if module_ptr.is_null() {
        return Err(LoadError::InitReturnedNull {
            name: name.to_owned(),
        });
    }

    // Convert the returned *mut PyObject to a Molt handle. Unknown pointers are
    // a bridge contract violation and must fail loudly; returning None would
    // hide an unsupported extension init path.
    let molt_bits = {
        let bridge = GLOBAL_BRIDGE.lock();
        bridge
            .pyobj_to_handle(module_ptr)
            .ok_or_else(|| LoadError::InitReturnedUnmappedObject {
                name: name.to_owned(),
            })?
    };

    // Keep the library alive for the process lifetime; extension code/data may
    // be referenced by module objects and function pointers after init.
    LOADED_EXTENSION_LIBRARIES.lock().push(lib);

    Ok(molt_bits)
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
