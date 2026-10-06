#[path = "../build_support/c_codegen.rs"]
mod c_codegen;

use std::env;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("target os");
    let source = manifest.join("l7_overlay_probe.c");
    let identity = manifest.join("type_identity_probe.c");
    let allocation = manifest.join("allocation_probe.c");
    let type_factory = manifest.join("type_factory_probe.c");
    let linked_include = manifest.join("../molt-cpython-abi/include");
    let shared_abi_include = manifest.join("../../include/molt/shared");
    let public_include = manifest.join("../../include");
    let mut overlay = cc::Build::new();
    overlay
        .file(&source)
        .include(&linked_include)
        .include(&shared_abi_include)
        .include(&public_include);
    c_codegen::apply_codegen_policy(&mut overlay, &target_os);
    overlay.compile("molt_l7_overlay_probe");
    // The same consumer is compiled against both distributed header facades.
    for (consumer, symbol, name, public_header) in [
        (
            &identity,
            "MOLT_TYPE_IDENTITY_PROBE",
            "molt_linked_type_identity_probe",
            false,
        ),
        (
            &identity,
            "MOLT_TYPE_IDENTITY_PROBE",
            "molt_public_type_identity_probe",
            true,
        ),
        (
            &allocation,
            "MOLT_ALLOCATION_PROBE",
            "molt_linked_allocation_probe",
            false,
        ),
        (
            &allocation,
            "MOLT_ALLOCATION_PROBE",
            "molt_public_allocation_probe",
            true,
        ),
        (
            &type_factory,
            "MOLT_TYPE_FACTORY_PROBE",
            "molt_linked_type_factory_probe",
            false,
        ),
        (
            &type_factory,
            "MOLT_TYPE_FACTORY_PROBE",
            "molt_public_type_factory_probe",
            true,
        ),
    ] {
        let mut build = cc::Build::new();
        build
            .file(consumer)
            .include(&linked_include)
            .include(&shared_abi_include)
            .include(&public_include)
            .define(symbol, Some(name));
        c_codegen::apply_codegen_policy(&mut build, &target_os);
        if public_header {
            build.define("MOLT_PUBLIC_HEADER_PROBE", None);
        }
        build.compile(name);
    }
    for path in [
        source,
        identity,
        allocation,
        type_factory,
        shared_abi_include,
        linked_include.join("Python.h"),
        public_include.join("molt/Python.h"),
        manifest.join("../build_support/c_codegen.rs"),
    ] {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}
