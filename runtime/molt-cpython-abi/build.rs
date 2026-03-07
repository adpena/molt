use std::env;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let shim = manifest.join("shims/pyarg_variadic.c");

    // Compile the C variadic shim into the crate's static library.
    cc::Build::new()
        .file(&shim)
        .opt_level(3)
        // Auto-vectorisation hints for clang/gcc.
        .flag_if_supported("-fvectorize")
        .flag_if_supported("-fslp-vectorize")
        // Allow undefined Rust symbols at build time — resolved at link time.
        .flag_if_supported("-fno-semantic-interposition")
        .compile("molt_pyarg_shims");

    println!("cargo:rerun-if-changed={}", shim.display());
    println!("cargo:rerun-if-changed=build.rs");
}
