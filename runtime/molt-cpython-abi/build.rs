use std::env;
use std::path::PathBuf;

#[path = "../build_support/unicode_tables.rs"]
mod unicode_tables;

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

fn wasi_sdk_sysroot_candidates(raw: &str) -> Vec<PathBuf> {
    let sdk_root = PathBuf::from(raw);
    vec![
        sdk_root.clone(),
        sdk_root.join("share").join("wasi-sysroot"),
        sdk_root.join("wasi-sysroot"),
    ]
}

fn wasi_sysroot_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    for key in ["MOLT_WASI_SYSROOT", "WASI_SYSROOT"] {
        if let Ok(value) = env::var(key) {
            candidates.push(PathBuf::from(value));
        }
    }
    for key in ["WASI_SDK_PATH", "WASI_SDK_PREFIX"] {
        if let Ok(value) = env::var(key) {
            candidates.extend(wasi_sdk_sysroot_candidates(&value));
        }
    }
    if let Ok(value) = env::var("MOLT_TARGET_ROOT") {
        let target_root = PathBuf::from(value);
        candidates.extend([
            target_root.join("toolchains").join("wasi-sysroot"),
            target_root
                .join("toolchains")
                .join("wasi-sdk")
                .join("share")
                .join("wasi-sysroot"),
            target_root
                .join("toolchains")
                .join("wasi-sdk")
                .join("wasi-sysroot"),
            target_root.join("wasi-sysroot"),
            target_root
                .join("wasi-sdk")
                .join("share")
                .join("wasi-sysroot"),
            target_root.join("wasi-sdk").join("wasi-sysroot"),
        ]);
        if let Ok(entries) = std::fs::read_dir(target_root.join("toolchains")) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
                    if name.starts_with("wasi-sysroot") {
                        candidates.push(path.clone());
                    }
                    if name.starts_with("wasi-sdk") {
                        candidates.push(path.join("share").join("wasi-sysroot"));
                        candidates.push(path.join("wasi-sysroot"));
                    }
                }
            }
        }
    }
    candidates
}

fn normalize_wasi_sysroot(path: PathBuf) -> Option<PathBuf> {
    let mut roots = vec![path.clone()];
    if path.file_name().and_then(|name| name.to_str()) == Some("include") {
        if let Some(parent) = path.parent() {
            roots.push(parent.to_path_buf());
        }
    }
    if path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        == Some("include")
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("wasm32-"))
    {
        if let Some(parent) = path.parent().and_then(|parent| parent.parent()) {
            roots.push(parent.to_path_buf());
        }
    }
    for root in roots {
        if root.join("include").join("errno.h").exists()
            || root
                .join("include")
                .join("wasm32-wasip1")
                .join("errno.h")
                .exists()
        {
            return Some(root);
        }
    }
    None
}

fn resolve_wasi_sysroot() -> Option<PathBuf> {
    for key in [
        "MOLT_WASI_SYSROOT",
        "WASI_SYSROOT",
        "WASI_SDK_PATH",
        "WASI_SDK_PREFIX",
        "MOLT_TARGET_ROOT",
    ] {
        println!("cargo:rerun-if-env-changed={key}");
    }
    wasi_sysroot_candidates()
        .into_iter()
        .find_map(normalize_wasi_sysroot)
}

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let shim = manifest.join("shims/pyarg_variadic.c");
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    unicode_tables::emit_cpython_abi_unicode_tables(&out_dir, &resolve_build_python());

    // Compile the C variadic shim into a static library.
    let mut build = cc::Build::new();
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
    if env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        let sysroot = resolve_wasi_sysroot().unwrap_or_else(|| {
            panic!(
                "WASI sysroot not found: set MOLT_WASI_SYSROOT, WASI_SYSROOT, \
                 WASI_SDK_PATH, WASI_SDK_PREFIX, or MOLT_TARGET_ROOT so \
                 wasm32-wasip1 CPython ABI provider shims can compile."
            )
        });
        build.flag(format!("--sysroot={}", sysroot.display()));
    }

    build.compile("molt_pyarg_shims");

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
    println!("cargo:rerun-if-changed=build.rs");
}
