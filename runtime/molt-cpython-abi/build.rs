use std::env;
use std::path::PathBuf;

#[path = "../build_support/unicode_tables.rs"]
mod unicode_tables;
#[path = "../build_support/wasi_sysroot.rs"]
mod wasi_sysroot;

fn resolve_build_python() -> String {
    println!("cargo:rerun-if-env-changed=MOLT_BUILD_PYTHON");
    println!("cargo:rerun-if-env-changed=PYTHON");
    for key in ["MOLT_BUILD_PYTHON", "PYTHON"] {
        if let Ok(value) = env::var(key) {
            let value = value.trim();
            if !value.is_empty() {
                return value.to_string();
            }
        }
    }
    if cfg!(windows) {
        "python".to_string()
    } else {
        "python3".to_string()
    }
}

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let shim = manifest.join("shims/pyarg_variadic.c");
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    unicode_tables::emit_cpython_abi_unicode_tables(&out_dir, &resolve_build_python());

    // Compile the C variadic shim into a static library.
    let mut build = cc::Build::new();
    if target_arch == "wasm32" {
        build.cargo_metadata(false);
    }
    build
        .file(&shim)
        .opt_level(3)
        // Auto-vectorisation hints for clang/gcc.
        .flag_if_supported("-fvectorize")
        .flag_if_supported("-fslp-vectorize");

    // -fno-semantic-interposition is useful on GCC/Linux but triggers a
    // warning on Apple clang; skip it on macOS.
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "macos" {
        build.flag_if_supported("-fno-semantic-interposition");
    }
    if target_arch == "wasm32" {
        let sysroot = wasi_sysroot::resolve_wasi_sysroot().unwrap_or_else(|| {
            panic!(
                "WASI sysroot not found: set MOLT_WASI_SYSROOT, WASI_SYSROOT, \
                 WASI_SDK_PATH, WASI_SDK_PREFIX, or MOLT_TARGET_ROOT so \
                 wasm32-wasip1 CPython ABI provider shims can compile."
            )
        });
        build.flag(sysroot.sysroot_flag());
        if let Some(include_dir) = sysroot.include_dir() {
            build.include(include_dir);
        }
    }

    build.compile("molt_pyarg_shims");
    if target_arch == "wasm32" {
        println!("cargo:rustc-link-search=native={}", out_dir.display());
    }

    // Single-authority layout enforcement. Compile a tiny translation unit that
    // pulls in <Python.h> (and, transitively, the generated
    // include/_molt_abi_layout.generated.h parity block). Its _Static_assert
    // checks pin every traditional-representation struct's sizeof/offsetof to the
    // Rust `#[repr(C)]` authority in src/abi_types.rs, so a drift between the C
    // header and the Rust structs the dylib operates on fails THIS crate's build
    // rather than corrupting memory at runtime.
    //
    // The generated assertions encode LP64/LLP64 native sizes (8-byte pointers /
    // Py_ssize_t). wasm32 is ILP32 (4-byte pointers), and the standalone-link ABI
    // tier that this header serves is a native-only surface, so we scope the
    // layout assertion TU to non-wasm targets.
    if target_arch != "wasm32" {
        let layout_assert = manifest.join("shims/abi_layout_assert.c");
        let abi_include = manifest.join("include");
        cc::Build::new()
            .file(&layout_assert)
            .include(&abi_include)
            .opt_level(0)
            .compile("molt_abi_layout_assert");
        println!("cargo:rerun-if-changed={}", layout_assert.display());
        println!(
            "cargo:rerun-if-changed={}",
            abi_include.join("_molt_abi_layout.generated.h").display()
        );
        println!(
            "cargo:rerun-if-changed={}",
            abi_include.join("Python.h").display()
        );
    }

    // Force the static shim's symbols into the cdylib output so that
    // PyArg_ParseTuple / PyArg_ParseTupleAndKeywords are exported even
    // though no Rust code calls them directly.
    //
    // macOS: -force_load <path> includes every object file in the archive.
    // Linux: --whole-archive / --no-whole-archive does the same.
    let lib_path = out_dir.join("libmolt_pyarg_shims.a");
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    match target_os.as_str() {
        "macos" => {
            println!(
                "cargo:rustc-cdylib-link-arg=-Wl,-force_load,{}",
                lib_path.display()
            );
            // Apple targets emit SUBSECTIONS_VIA_SYMBOLS, so the section-level dead-stripper
            // can still remove symbols from -force_load archives if nothing calls them.
            // Explicitly export the variadic shim symbols to pin them in the dylib exports trie.
            for sym in &[
                "_PyArg_ParseTuple",
                "_PyArg_ParseTupleAndKeywords",
                "_PyArg_VaParseTupleAndKeywords",
                "_PyArg_UnpackTuple",
                "_PyTuple_Pack",
                "_PyObject_CallFunction",
                "_PyObject_CallFunctionObjArgs",
                "_PyObject_CallMethodObjArgs",
                "_PyObject_CallMethod",
                "_Py_BuildValue",
                "__Py_BuildValue_SizeT",
                "_Py_VaBuildValue",
                "_PyUnicode_FromFormat",
                "_PyUnicode_FromFormatV",
                "_PyOS_snprintf",
                "_PyOS_vsnprintf",
                "_PyOS_string_to_double",
                "_PyOS_strtol",
                "_PyOS_strtoul",
                "_PyErr_WarnFormat",
                "_PyErr_Format",
                "_PyErr_FormatV",
                "_PyErr_FormatUnraisable",
                "_PySys_WriteStderr",
            ] {
                println!("cargo:rustc-cdylib-link-arg=-Wl,-exported_symbol,{sym}");
            }
        }
        "linux" => {
            println!("cargo:rustc-cdylib-link-arg=-Wl,--whole-archive");
            println!("cargo:rustc-cdylib-link-arg={}", lib_path.display());
            println!("cargo:rustc-cdylib-link-arg=-Wl,--no-whole-archive");
        }
        "windows" if target_env == "msvc" => {
            println!(
                "cargo:rustc-cdylib-link-arg=/WHOLEARCHIVE:{}",
                lib_path.display()
            );
        }
        _ => {}
    }

    println!("cargo:rerun-if-changed={}", shim.display());
    println!("cargo:rerun-if-changed=../build_support/unicode_tables.rs");
    println!("cargo:rerun-if-changed=src/api/strings.rs");
    // The layout-assertion TU is generated from the Rust repr(C) authority; rebuild
    // it if that authority changes so a drift is caught immediately.
    println!("cargo:rerun-if-changed=src/abi_types.rs");
    println!("cargo:rerun-if-changed=build.rs");
}
