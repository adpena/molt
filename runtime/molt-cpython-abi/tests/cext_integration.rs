//! Integration test: compile _testmolt.c, dlopen it, call PyInit__testmolt.
//!
//! Run with:
//!   cargo test --release -p molt-lang-cpython-abi --features extension-loader \
//!       --test cext_integration -- --nocapture
//!
//! Architecture note
//! -----------------
//! The test binary links `molt_cpython_abi` as an **rlib** (separate copy of statics).
//! The compiled extension links against `libmolt_cpython_abi.dylib` (its own copy).
//! To avoid two independent uninitialised static pools, we:
//!   1. `dlopen` the dylib with RTLD_GLOBAL so its symbols win at link-resolution time.
//!   2. Resolve and call `molt_cpython_abi_init` from the dylib handle.
//!   3. Only then call `load_cpython_extension`, which loads the extension into the
//!      same address space — the extension's undefined symbols resolve to the dylib.

#![cfg(all(feature = "extension-loader", not(target_arch = "wasm32")))]

use std::ffi::CString;
use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR = runtime/molt-cpython-abi
    // → go up 2 levels to reach the Molt repo root.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root")
        .to_path_buf()
}

fn dylib_path() -> PathBuf {
    let target = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| repo_root().join("target"));
    target.join("release").join("libmolt_cpython_abi.dylib")
}

fn build_extension() -> PathBuf {
    let root = repo_root();
    let src = root.join("runtime/molt-cpython-abi/tests/c_extensions/_testmolt.c");
    let out_dir = std::env::temp_dir().join("molt-cext-integration-test");
    std::fs::create_dir_all(&out_dir).expect("create out dir");

    let cargo_target = std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| {
        repo_root().join("target").display().to_string()
    });

    let status = Command::new("bash")
        .arg(root.join("scripts/build-cext.sh"))
        .arg(&src)
        .arg(&out_dir)
        .env("CARGO_TARGET_DIR", &cargo_target)
        .status()
        .expect("build-cext.sh spawn");

    assert!(status.success(), "build-cext.sh failed with {status}");

    #[cfg(target_os = "macos")]
    let so = out_dir.join("_testmolt.cpython-312-darwin.so");
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    let so = out_dir.join("_testmolt.cpython-312-aarch64-linux-gnu.so");
    #[cfg(all(target_os = "linux", not(target_arch = "aarch64")))]
    let so = out_dir.join("_testmolt.cpython-312-x86_64-linux-gnu.so");

    assert!(so.exists(), "compiled extension not found at {so:?}");
    so
}

/// Preload `libmolt_cpython_abi.dylib` with RTLD_GLOBAL so the extension's
/// undefined symbols resolve to the dylib's address space, then call its init.
unsafe fn preload_and_init_dylib() {
    let path = dylib_path();
    assert!(
        path.exists(),
        "libmolt_cpython_abi dylib not found at {path:?}. \
         Run: CARGO_TARGET_DIR=<dir> cargo build --release -p molt-lang-cpython-abi"
    );

    let path_cstr = CString::new(path.to_str().expect("valid UTF-8 path")).unwrap();

    #[cfg(target_os = "macos")]
    let rtld_global: libc::c_int = 0x8;
    #[cfg(target_os = "linux")]
    let rtld_global: libc::c_int = 0x100;

    let handle = unsafe { libc::dlopen(path_cstr.as_ptr(), libc::RTLD_LAZY | rtld_global) };
    assert!(!handle.is_null(), "dlopen failed for {path:?}");

    let sym_name = CString::new("molt_cpython_abi_init").unwrap();
    let sym = unsafe { libc::dlsym(handle, sym_name.as_ptr()) };
    assert!(
        !sym.is_null(),
        "molt_cpython_abi_init not found in dylib — rebuild with latest bridge.rs"
    );

    let init_fn: extern "C" fn() = unsafe { std::mem::transmute(sym) };
    init_fn();
    println!("✓ molt_cpython_abi_init called on dylib");
}

#[test]
fn test_load_testmolt_extension() {
    let so_path = build_extension();

    // Preload the dylib so the extension resolves ABI symbols from the dylib's
    // address space (the one we just initialised), not an uninitialised rlib copy.
    unsafe { preload_and_init_dylib() };

    // SAFETY: we compiled this ourselves; it links against libmolt_cpython_abi.
    let result = unsafe {
        molt_cpython_abi::loader::load_cpython_extension(&so_path, "_testmolt")
    };

    match result {
        Ok(bits) => {
            println!("✓ _testmolt loaded, module bits = 0x{bits:016x}");
            assert_ne!(bits, 0, "module handle should be non-zero");
        }
        Err(e) => panic!("Failed to load _testmolt extension: {e:?}"),
    }
}
