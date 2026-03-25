//! C extension loader — dlopen a real CPython `.so` and call `PyInit_<name>()`.
//!
//! ## What this does
//!
//! When Molt encounters `import numpy` (for example), instead of failing it
//! can try to load the system's `numpy.cpython-312-darwin.so`. That `.so` was
//! compiled against CPython 3.12's C API, but our `molt-cpython-abi` crate
//! provides compatible implementations of all the C API symbols via normal
//! dynamic linking (the loader binary links against this crate, which exports
//! `PyLong_FromLong`, `PyDict_New`, etc.).
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
use std::path::Path;

/// Error type for extension loading failures.
#[derive(Debug)]
pub enum LoadError {
    /// `dlopen` failed — library not found or not a valid shared library.
    DlopenFailed(libloading::Error),
    /// `PyInit_<name>` symbol not found in the library.
    InitSymbolMissing { lib_path: String, symbol: String },
    /// `PyInit_<name>()` returned NULL — initialization error.
    InitReturnedNull { name: String },
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

    // Convert the returned *mut PyObject to a Molt handle.
    let molt_bits = {
        let bridge = GLOBAL_BRIDGE.lock();
        bridge
            .pyobj_to_handle(module_ptr)
            .unwrap_or_else(|| molt_lang_obj_model::MoltObject::none().bits())
    };

    // Keep the library alive by leaking it — modules are kept for the lifetime
    // of the process. Production code would store it in a registry.
    std::mem::forget(lib);

    Ok(molt_bits)
}

/// Search standard CPython extension paths for `name`.
///
/// Tries in order:
/// 1. `MOLT_EXTENSION_PATH` env var (colon-separated directories)
/// 2. Site-packages of the active Python (from `python3 -c "import site"`)
/// 3. `/usr/local/lib/python3.*/lib-dynload/`
/// 4. `/usr/lib/python3/dist-packages/`
pub fn find_extension(name: &str) -> Option<std::path::PathBuf> {
    let candidates = extension_candidate_paths(name);
    candidates.into_iter().find(|p| p.exists())
}

fn extension_candidate_paths(name: &str) -> Vec<std::path::PathBuf> {
    use std::path::PathBuf;

    let mut out = Vec::new();

    // MOLT_EXTENSION_PATH
    if let Ok(env_path) = std::env::var("MOLT_EXTENSION_PATH") {
        for dir in env_path.split(':') {
            let dir = PathBuf::from(dir);
            // Try common suffixes.
            for suffix in cpython_so_suffixes(name) {
                out.push(dir.join(&suffix));
            }
        }
    }

    // Probe python3 for site-packages.
    if let Ok(output) = std::process::Command::new("python3")
        .args([
            "-c",
            "import site; print('\\n'.join(site.getsitepackages()))",
        ])
        .output()
    {
        if output.status.success() {
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                let dir = PathBuf::from(line.trim());
                for suffix in cpython_so_suffixes(name) {
                    out.push(dir.join(name).join(&suffix));
                    out.push(dir.join(&suffix));
                }
            }
        }
    }

    // System fallbacks.
    for base in [
        "/usr/local/lib/python3.12/lib-dynload",
        "/usr/local/lib/python3.11/lib-dynload",
        "/usr/lib/python3/dist-packages",
    ] {
        for suffix in cpython_so_suffixes(name) {
            out.push(PathBuf::from(base).join(&suffix));
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
        #[cfg(target_os = "linux")]
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
    let path = find_extension(name).ok_or_else(|| LoadError::InitSymbolMissing {
        lib_path: format!("<search paths for '{name}'>"),
        symbol: format!("PyInit_{name}"),
    })?;
    unsafe { load_cpython_extension(&path, name) }
}
