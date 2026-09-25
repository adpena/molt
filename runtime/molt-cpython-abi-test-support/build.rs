use std::env;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let source = manifest.join("l7_overlay_probe.c");
    let shared_abi_include = manifest.join("../../include/molt/shared");
    cc::Build::new()
        .file(&source)
        .include(manifest.join("../molt-cpython-abi/include"))
        .include(&shared_abi_include)
        .include(manifest.join("../../include"))
        .opt_level(3)
        .flag_if_supported("-fno-semantic-interposition")
        .compile("molt_l7_overlay_probe");
    println!("cargo:rerun-if-changed={}", source.display());
    println!("cargo:rerun-if-changed={}", shared_abi_include.display());
}
