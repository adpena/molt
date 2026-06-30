use std::env;
use std::path::PathBuf;

fn wasi_sysroot_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    for key in ["MOLT_WASI_SYSROOT", "WASI_SYSROOT"] {
        if let Ok(value) = env::var(key) {
            candidates.push(PathBuf::from(value));
        }
    }
    if let Ok(value) = env::var("WASI_SDK_PATH") {
        let sdk_root = PathBuf::from(value);
        candidates.push(sdk_root.clone());
        candidates.push(sdk_root.join("share").join("wasi-sysroot"));
        candidates.push(sdk_root.join("wasi-sysroot"));
    }
    if let Ok(value) = env::var("MOLT_TARGET_ROOT") {
        let target_root = PathBuf::from(value);
        candidates.push(target_root.join("toolchains").join("wasi-sysroot"));
        candidates.push(
            target_root
                .join("toolchains")
                .join("wasi-sdk")
                .join("share")
                .join("wasi-sysroot"),
        );
        candidates.push(target_root.join("wasi-sysroot"));
        if let Ok(entries) = std::fs::read_dir(target_root.join("toolchains")) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("wasi-sysroot-"))
                {
                    candidates.push(path);
                }
            }
        }
    }
    candidates.extend(
        [
            "/opt/homebrew/opt/wasi-libc/share/wasi-sysroot",
            "/usr/local/opt/wasi-libc/share/wasi-sysroot",
            "/opt/wasi-sdk/share/wasi-sysroot",
            "/opt/wasi-sdk/wasi-sysroot",
            "/usr/share/wasi-sysroot",
            "/usr/local/share/wasi-sysroot",
        ]
        .into_iter()
        .map(PathBuf::from),
    );
    candidates
}

fn normalize_wasi_sysroot(path: PathBuf) -> Option<PathBuf> {
    let roots = [
        path.clone(),
        path.parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| path.clone()),
    ];
    for root in roots {
        if root.join("include").join("errno.h").exists()
            || root
                .join("include")
                .join("wasm32-wasip1")
                .join("errno.h")
                .exists()
            || root
                .join("include")
                .join("wasm32-wasi")
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
        "MOLT_TARGET_ROOT",
    ] {
        println!("cargo:rerun-if-env-changed={key}");
    }
    for candidate in wasi_sysroot_candidates() {
        if let Some(root) = normalize_wasi_sysroot(candidate) {
            return Some(root);
        }
    }
    None
}

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let shim = manifest.join("shims/pyarg_variadic.c");
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

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
    if env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default() == "wasm32" {
        let sysroot = resolve_wasi_sysroot().unwrap_or_else(|| {
            panic!(
                "WASI sysroot not found: set MOLT_WASI_SYSROOT, WASI_SYSROOT, \
                 WASI_SDK_PATH, or MOLT_TARGET_ROOT so wasm32-wasip1 CPython ABI \
                 provider shims can compile."
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
                "_PyArg_UnpackTuple",
            ] {
                println!("cargo:rustc-cdylib-link-arg=-Wl,-exported_symbol,{sym}");
            }
        }
        "linux" => {
            println!("cargo:rustc-cdylib-link-arg=-Wl,--whole-archive");
            println!("cargo:rustc-cdylib-link-arg={}", lib_path.display());
            println!("cargo:rustc-cdylib-link-arg=-Wl,--no-whole-archive");
        }
        _ => {}
    }

    println!("cargo:rerun-if-changed={}", shim.display());
    println!("cargo:rerun-if-changed=build.rs");
}
