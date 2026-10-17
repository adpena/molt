use std::env;
use std::fs;
use std::path::PathBuf;

#[expect(
    dead_code,
    reason = "Errno is a data object, not an optimized C routine"
)]
#[path = "../build_support/c_codegen.rs"]
mod c_codegen;
#[path = "../build_support/target_errno.rs"]
mod target_errno;

fn required(name: &str) -> String {
    env::var(name).unwrap_or_else(|_| panic!("missing Cargo {name}"))
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // Molt's synthetic wasm32 errno has its own in-place declaration. Neither
    // actual WASI C errno nor this native public module changes that domain.
    if required("CARGO_CFG_TARGET_ARCH") == "wasm32" {
        return;
    }
    let out = PathBuf::from(required("OUT_DIR"));
    let target = required("TARGET");
    let manifest = PathBuf::from(required("CARGO_MANIFEST_DIR"));
    let declaration_path = manifest.join("errno_symbols.tsv");
    let declaration_text = fs::read_to_string(&declaration_path).expect("read errno declaration");
    let declaration = target_errno::Declaration::parse(&declaration_text)
        .expect("valid native errno declaration");
    let shape = target_errno::NativeTarget::from_cargo(
        &required("CARGO_CFG_TARGET_ARCH"),
        &required("CARGO_CFG_TARGET_OS"),
        &required("CARGO_CFG_TARGET_ENV"),
        &required("CARGO_CFG_TARGET_ABI"),
        &required("CARGO_CFG_TARGET_POINTER_WIDTH"),
        &required("CARGO_CFG_TARGET_ENDIAN"),
    )
    .expect("native errno target encoding");
    // Reuse Cargo/cc's selected target compiler and flags. Never use host
    // Python, run a target binary, choose another compiler after failure, or
    // require a WASI SDK/capture for a native build.
    let mut build = cc::Build::new();
    build.target(&target);
    let values = target_errno::evaluate(&declaration, &shape, &mut build, &out)
        .expect("selected native C errno evaluation");
    fs::write(
        out.join("errno_constants.rs"),
        declaration
            .rust_source(&values)
            .expect("render native errno publication"),
    )
    .expect("write native errno publication");
    // Actual-C controls use the same Cargo-selected host/target coordinate.
    fs::write(out.join("errno_target.txt"), &target).expect("write errno test target");
    for (name, value) in [
        ("arch", required("CARGO_CFG_TARGET_ARCH")),
        ("os", required("CARGO_CFG_TARGET_OS")),
        ("environment", required("CARGO_CFG_TARGET_ENV")),
        ("abi", required("CARGO_CFG_TARGET_ABI")),
        ("pointer_width", required("CARGO_CFG_TARGET_POINTER_WIDTH")),
        ("endian", required("CARGO_CFG_TARGET_ENDIAN")),
    ] {
        fs::write(out.join(format!("errno_{name}.txt")), value)
            .expect("write errno test coordinate");
    }
    for path in [
        declaration_path,
        manifest.join("../build_support/target_errno.rs"),
        manifest.join("../build_support/errno_feature_visibility.h"),
        manifest.join("../build_support/c_codegen.rs"),
    ] {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    // cc owns CC/CFLAGS/target variants and its SDK environment reads. These
    // ambient inputs also affect subprocess lookup/preprocessing on bare Cargo.
    for name in [
        "PATH",
        "CPATH",
        "C_INCLUDE_PATH",
        "INCLUDE",
        "CL",
        "_CL_",
        "COMPILER_PATH",
        "GCC_EXEC_PREFIX",
        "SDKROOT",
        "DEVELOPER_DIR",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }
}
