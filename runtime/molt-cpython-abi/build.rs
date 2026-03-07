use std::env;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let shim = manifest.join("shims/pyarg_variadic.c");
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    // Compile the C variadic shim into a static library.
    cc::Build::new()
        .file(&shim)
        .opt_level(3)
        // Auto-vectorisation hints for clang/gcc.
        .flag_if_supported("-fvectorize")
        .flag_if_supported("-fslp-vectorize")
        .flag_if_supported("-fno-semantic-interposition")
        .compile("molt_pyarg_shims");

    // Force the static shim's symbols into the cdylib output so that
    // PyArg_ParseTuple / PyArg_ParseTupleAndKeywords are exported even
    // though no Rust code calls them directly.
    //
    // macOS: -force_load <path> includes every object file in the archive.
    // Linux: --whole-archive / --no-whole-archive does the same.
    let lib_path = out_dir.join("libmolt_pyarg_shims.a");
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
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
            println!(
                "cargo:rustc-cdylib-link-arg={}",
                lib_path.display()
            );
            println!("cargo:rustc-cdylib-link-arg=-Wl,--no-whole-archive");
        }
        _ => {}
    }

    println!("cargo:rerun-if-changed={}", shim.display());
    println!("cargo:rerun-if-changed=build.rs");
}
