//! End-to-end dlopen smoke test for Molt's CPython ABI bridge.
//!
//! Compiles `hello.c` (a METH_NOARGS extension with a single `greet` function)
//! against `libmolt_cpython_abi.dylib`, then exercises the full import path:
//!
//!   1. Init Molt's runtime (so RuntimeHooks are registered with the bridge).
//!   2. Pre-load the bridge dylib with RTLD_GLOBAL so the extension's
//!      undefined symbols (`PyModule_Create2`, `PyUnicode_FromString`, …)
//!      resolve to the dylib's globally-initialised state — not a separate
//!      uninitialised rlib copy.
//!   3. Re-register the runtime hooks against the dylib's hook slot so the
//!      bridge running inside the extension uses the live runtime state.
//!   4. dlopen `hello.so` and call `PyInit_hello`.
//!   5. Look up the resulting Molt module's `greet` attribute and call it
//!      through `molt_call_func0`.
//!   6. Assert the returned string is `"hello from C"`.
//!
//! The test is gated on the `cext_loader` feature.  Skipped on wasm32.

#![cfg(all(feature = "cext_loader", not(target_arch = "wasm32")))]

use molt_obj_model::MoltObject;
// Importing items from `molt_runtime` ensures the linker pulls in the rlib so
// our `extern "C"` declarations of `molt_call_func_dispatch` etc. resolve.
use molt_runtime::lifecycle as _runtime_lifecycle;
use std::ffi::CString;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Once;

// The runtime expects these symbols from the compiled Python module.
// Provide stubs so integration tests can link.
#[unsafe(no_mangle)]
pub extern "C" fn molt_isolate_bootstrap() -> u64 {
    MoltObject::none().bits()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_isolate_import(_name_bits: u64) -> u64 {
    MoltObject::none().bits()
}

unsafe extern "C" {
    fn molt_exception_clear() -> u64;
    fn molt_object_getattr_bytes(obj_bits: u64, name_ptr: *const u8, name_len: u64) -> u64;
    fn molt_call_func_dispatch(
        func_bits: u64,
        args_ptr_bits: u64,
        nargs: u64,
        code_id: u64,
    ) -> u64;
    fn molt_string_as_ptr(string_bits: u64, out_len: *mut u64) -> *const u8;
}

static INIT: Once = Once::new();

fn init_runtime() {
    INIT.call_once(|| {
        _runtime_lifecycle::init();
        // Register the cpython-abi hook table BEFORE any extension loads so
        // both copies of the bridge (rlib in this test, dylib pre-loaded
        // below) share the same runtime backend.
        molt_runtime::cpython_abi_hooks::register_cpython_hooks();
    });
    let _ = unsafe { molt_exception_clear() };
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root")
        .to_path_buf()
}

fn cargo_target_dir() -> PathBuf {
    std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| repo_root().join("target"))
}

fn cargo_target_release_dir() -> PathBuf {
    let target = cargo_target_dir();
    // The test binary lives at `<target>/<profile>/deps/`; `<profile>` is the
    // sibling we need.  Walk up from the current exe to discover it without
    // hard-coding profile names — that way `--release`, `--profile dev`, and
    // `--profile release-fast` all locate the bridge dylib next to the test
    // binary instead of binding to a stale build under a different profile.
    if let Ok(exe) = std::env::current_exe() {
        // exe is .../<profile>/deps/<binary>
        if let Some(profile_dir) = exe.parent().and_then(|p| p.parent()) {
            let candidate = profile_dir.join(if cfg!(target_os = "macos") {
                "libmolt_cpython_abi.dylib"
            } else {
                "libmolt_cpython_abi.so"
            });
            if candidate.exists() {
                return profile_dir.to_path_buf();
            }
        }
    }
    // Fallback: scan known profile names if current_exe layout is unfamiliar.
    for sub in &["release-fast", "release", "debug"] {
        let candidate = target.join(sub);
        if candidate.join("libmolt_cpython_abi.dylib").exists()
            || candidate.join("libmolt_cpython_abi.so").exists()
        {
            return candidate;
        }
    }
    target.join("release")
}

fn dylib_path() -> PathBuf {
    let dir = cargo_target_release_dir();
    #[cfg(target_os = "macos")]
    {
        dir.join("libmolt_cpython_abi.dylib")
    }
    #[cfg(not(target_os = "macos"))]
    {
        dir.join("libmolt_cpython_abi.so")
    }
}

fn build_hello_extension() -> PathBuf {
    let root = repo_root();
    let src = root.join("runtime/molt-cpython-abi/tests/c_extensions/hello.c");
    let out_dir = std::env::temp_dir().join("molt-cext-dlopen-smoke");
    std::fs::create_dir_all(&out_dir).expect("create out dir");

    let cargo_target = cargo_target_dir();
    let release_dir = cargo_target_release_dir();
    // Derive the profile name from the dylib's directory so build-cext.sh
    // links against the same bridge copy the loader will dlopen.
    let profile_name = release_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("release")
        .to_string();
    let status = Command::new("bash")
        .arg(root.join("tools/scripts/build-cext.sh"))
        .arg(&src)
        .arg(&out_dir)
        .env("CARGO_TARGET_DIR", cargo_target.display().to_string())
        .env("BUILD_PROFILE", &profile_name)
        .status()
        .expect("build-cext.sh spawn");
    assert!(status.success(), "build-cext.sh failed: {status}");

    #[cfg(target_os = "macos")]
    let suffix = "hello.cpython-312-darwin.so";
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    let suffix = "hello.cpython-312-aarch64-linux-gnu.so";
    #[cfg(all(target_os = "linux", not(target_arch = "aarch64")))]
    let suffix = "hello.cpython-312-x86_64-linux-gnu.so";

    let so = out_dir.join(suffix);
    assert!(so.exists(), "compiled extension not found at {so:?}");
    so
}

unsafe fn preload_bridge_dylib_and_register_hooks() {
    let path = dylib_path();
    if !path.exists() {
        // The bridge dylib is built when the workspace is built with
        // `--features extension-loader` (or via cargo build -p molt-lang-cpython-abi).
        // Skip the test instead of failing if the dylib is absent — CI must
        // arrange the build, but a bare `cargo test` without preparation
        // should not error obscurely.
        eprintln!("skipping cext smoke: bridge dylib missing at {path:?}");
        return;
    }
    let path_cstr = CString::new(path.to_str().expect("utf-8 path")).unwrap();
    #[cfg(target_os = "macos")]
    let rtld_global: libc::c_int = 0x8;
    #[cfg(target_os = "linux")]
    let rtld_global: libc::c_int = 0x100;

    let handle = unsafe { libc::dlopen(path_cstr.as_ptr(), libc::RTLD_LAZY | rtld_global) };
    assert!(!handle.is_null(), "dlopen failed for {path:?}");

    // Initialise static type table inside the dylib copy.
    let init_sym = CString::new("molt_cpython_abi_init").unwrap();
    let init_ptr = unsafe { libc::dlsym(handle, init_sym.as_ptr()) };
    assert!(!init_ptr.is_null(), "molt_cpython_abi_init missing in dylib");
    let init_fn: extern "C" fn() = unsafe { std::mem::transmute(init_ptr) };
    init_fn();

    // Push the live runtime hooks into the dylib's hook slot.  Without this,
    // the bridge running inside the extension would use the no-op stubs.
    let reg_sym = CString::new("molt_cpython_abi_register_hooks").unwrap();
    let reg_ptr = unsafe { libc::dlsym(handle, reg_sym.as_ptr()) };
    assert!(
        !reg_ptr.is_null(),
        "molt_cpython_abi_register_hooks missing in dylib"
    );
    type RegFn =
        unsafe extern "C" fn(*const molt_cpython_abi::RuntimeHooks);
    let reg_fn: RegFn = unsafe { std::mem::transmute(reg_ptr) };
    let hooks = molt_cpython_abi::hooks().expect("runtime hooks must be registered");
    unsafe { reg_fn(hooks as *const _) };
}

#[test]
fn hello_extension_load_and_greet() {
    init_runtime();
    let so_path = build_hello_extension();

    if !dylib_path().exists() {
        eprintln!(
            "skipping: build the bridge dylib first via \
             `cargo build --release -p molt-lang-cpython-abi --features extension-loader`"
        );
        return;
    }
    unsafe { preload_bridge_dylib_and_register_hooks() };

    // SAFETY: hello.so was compiled by us against this exact ABI version.
    let module_bits =
        unsafe { molt_cpython_abi::loader::load_cpython_extension(&so_path, "hello") }
            .unwrap_or_else(|err| panic!("loader failed: {err:?}"));

    assert_ne!(module_bits, 0, "loader returned null module bits");
    assert_ne!(
        module_bits,
        MoltObject::none().bits(),
        "loader returned None placeholder instead of a real module"
    );

    // Look up `greet` on the module and invoke it.
    let greet_name = b"greet";
    let greet_bits = unsafe {
        molt_object_getattr_bytes(module_bits, greet_name.as_ptr(), greet_name.len() as u64)
    };
    assert_ne!(
        greet_bits,
        MoltObject::none().bits(),
        "module.greet missing from extension module"
    );

    let args: [u64; 0] = [];
    let result_bits = unsafe {
        molt_call_func_dispatch(greet_bits, args.as_ptr() as u64, 0, 0)
    };
    let mut result_len: u64 = 0;
    let result_ptr = unsafe { molt_string_as_ptr(result_bits, &mut result_len) };
    assert!(
        !result_ptr.is_null(),
        "result of greet() is not a Molt string (bits=0x{result_bits:x})"
    );
    let bytes = unsafe { std::slice::from_raw_parts(result_ptr, result_len as usize) };
    let s = std::str::from_utf8(bytes).expect("greet result is valid UTF-8");
    assert_eq!(s, "hello from C");
}
