use std::env;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let source = manifest.join("l7_overlay_probe.c");
    let identity = manifest.join("type_identity_probe.c");
    let allocation = manifest.join("allocation_probe.c");
    let type_factory = manifest.join("type_factory_probe.c");
    let linked_include = manifest.join("../molt-cpython-abi/include");
    let shared_abi_include = manifest.join("../../include/molt/shared");
    let public_include = manifest.join("../../include");
    cc::Build::new()
        .file(&source)
        .include(&linked_include)
        .include(&shared_abi_include)
        .include(&public_include)
        .opt_level(3)
        .flag_if_supported("-fno-semantic-interposition")
        .compile("molt_l7_overlay_probe");
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
            .define(symbol, Some(name))
            .opt_level(3)
            .flag_if_supported("-fno-semantic-interposition");
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
    ] {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}
