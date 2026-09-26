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
use libloading::Library;
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
    /// The shared initialization transaction failed; the C error indicator
    /// retains the actual exception. This may precede or follow PyInit.
    InitializationFailed { name: String },
    /// `PyInit_<name>()` violated the result/error-indicator contract.
    InitContractViolation { name: String, detail: String },
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
            Self::InitializationFailed { name } => {
                write!(
                    f,
                    "extension {name} initialization failed (C exception pending)"
                )
            }
            Self::InitContractViolation { name, detail } => {
                write!(f, "PyInit_{name}() {detail}")
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
    unsafe {
        load_extension(
            path,
            name,
            molt_lang_obj_model::MoltObject::none().bits(),
            false,
        )
    }
}

/// Create, but do not execute or publish, an extension using its real import
/// spec. Returns one owned runtime module for importlib.module_from_spec.
///
/// # Safety
/// Same requirements as `load_cpython_extension`; `spec_bits` must be retained
/// by the caller throughout the call.
pub unsafe fn create_cpython_extension(
    path: &Path,
    name: &str,
    spec_bits: u64,
) -> Result<u64, LoadError> {
    unsafe { load_extension(path, name, spec_bits, true) }
}

unsafe fn load_extension(
    path: &Path,
    name: &str,
    spec_bits: u64,
    create_only: bool,
) -> Result<u64, LoadError> {
    unsafe { crate::abi_types::init_static_types() };
    crate::bridge::init_tag_table();
    let Some(h) = crate::hooks::hooks() else {
        return Err(LoadError::InitContractViolation {
            name: name.to_owned(),
            detail: "requires registered runtime extension-initialization hooks".into(),
        });
    };
    let origin = path.to_string_lossy();
    let name_bits = unsafe { (h.alloc_str)(name.as_ptr(), name.len()) };
    if name_bits == 0 {
        crate::api::imports::propagate_hook_error(c"extension name allocation failed");
        return Err(LoadError::InitializationFailed {
            name: name.to_owned(),
        });
    }
    let name_owner = unsafe { crate::bridge::RuntimeValue::from_owned(name_bits) };
    let origin_bits = unsafe { (h.alloc_str)(origin.as_ptr(), origin.len()) };
    if origin_bits == 0 {
        crate::api::imports::propagate_hook_error(c"extension origin allocation failed");
        return Err(LoadError::InitializationFailed {
            name: name.to_owned(),
        });
    }
    let origin_owner = unsafe { crate::bridge::RuntimeValue::from_owned(origin_bits) };
    let lib = unsafe { Library::new(path) }.map_err(LoadError::DlopenFailed)?;
    let leaf = name.rsplit('.').next().unwrap_or(name);
    let symbol_name = format!("PyInit_{leaf}");
    let init_fn: unsafe extern "C" fn() -> *mut PyObject = unsafe {
        *lib.get::<unsafe extern "C" fn() -> *mut PyObject>(symbol_name.as_bytes())
            .map_err(|_| LoadError::InitSymbolMissing {
                lib_path: path.display().to_string(),
                symbol: symbol_name.clone(),
            })?
    };
    // Once extension code runs, callbacks/types may escape even if init fails.
    // Never unload their executable storage on a failed transaction.
    LOADED_EXTENSION_LIBRARIES.lock().push(lib);
    match unsafe {
        (h.initialize_extension)(
            init_fn,
            name_owner.bits(),
            origin_owner.bits(),
            spec_bits,
            create_only,
        )
    }
    .decode()
    {
        crate::hooks::DecodedHandleResult::Ok(bits) => Ok(bits),
        crate::hooks::DecodedHandleResult::Missing | crate::hooks::DecodedHandleResult::Error => {
            crate::api::imports::propagate_hook_error(
                c"extension initialization failed without an exception",
            );
            Err(LoadError::InitializationFailed {
                name: name.to_owned(),
            })
        }
    }
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
    extension_candidates_in(name, std::env::var_os("MOLT_EXTENSION_PATH").as_deref())
}

fn extension_candidates_in(name: &str, paths: Option<&std::ffi::OsStr>) -> Vec<std::path::PathBuf> {
    let Some(paths) = paths else {
        return Vec::new();
    };
    let suffixes = cpython_so_suffixes(name);
    std::env::split_paths(paths)
        .filter(|dir| !dir.as_os_str().is_empty())
        .flat_map(|dir| suffixes.iter().map(move |suffix| dir.join(suffix)))
        .collect()
}

fn cpython_so_suffixes(name: &str) -> Vec<String> {
    // Order matches CPython's import machinery search order.
    vec![
        // CPython 3.12 ABI tag — most common on modern systems.
        #[cfg(target_os = "macos")]
        format!("{name}.cpython-312-darwin.so"),
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        format!("{name}.cpython-312-x86_64-linux-gnu.so"),
        #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
        format!("{name}.cpython-312-aarch64-linux-gnu.so"),
        #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
        format!("{name}.cp312-win_amd64.pyd"),
        #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
        format!("{name}.cp312-win_arm64.pyd"),
        #[cfg(target_os = "windows")]
        format!("{name}.pyd"),
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
    use super::{LoadError, extension_candidates_in};

    #[test]
    fn extension_search_uses_only_explicit_env_roots() {
        let roots = [
            std::path::PathBuf::from("explicit").join("a"),
            std::path::PathBuf::from("explicit").join("b"),
        ];
        let path = std::env::join_paths(&roots).unwrap();
        let candidates = extension_candidates_in("demoext", Some(&path));
        assert!(!candidates.is_empty());
        assert!(candidates.iter().all(|candidate| {
            roots
                .iter()
                .any(|root| candidate.parent() == Some(root.as_path()))
        }));
        assert!(extension_candidates_in("demoext", None).is_empty());
    }

    #[test]
    fn extension_not_found_error_is_explicit() {
        let error = LoadError::ExtensionNotFound {
            name: "demoext".to_string(),
        };
        assert!(error.to_string().contains("MOLT_EXTENSION_PATH"));
    }
}
