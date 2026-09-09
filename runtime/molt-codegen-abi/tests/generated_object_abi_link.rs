use molt_codegen_abi::{
    GENERATED_OBJECT_ABI_FREE_THREADED_SYMBOL, GENERATED_OBJECT_ABI_GIL_SYMBOL,
};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod cargo_test_artifacts {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test_support/cargo_test_artifacts.rs"
    ));
}

fn rustc(artifacts: &cargo_test_artifacts::CargoTestArtifacts) -> Command {
    artifacts
        .command(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
        .expect("resolve fixture compiler")
}

fn write(path: &Path, source: &str) {
    std::fs::write(path, source).expect("write link-contract fixture");
}

fn compile_backend(
    artifacts: &cargo_test_artifacts::CargoTestArtifacts,
    label: &str,
    expected_symbol: &str,
) -> PathBuf {
    let root = artifacts.path();
    let source = root.join(format!("backend_{label}.rs"));
    let output = root.join(format!("libbackend_{label}.rlib"));
    write(
        &source,
        &format!(
            r#"
#[repr(transparent)]
struct SyncPtr(*const u8);
unsafe impl Sync for SyncPtr {{}}
unsafe extern "C" {{
    #[link_name = "{expected_symbol}"]
    static WITNESS: u8;
}}
#[used]
static ANCHOR: SyncPtr = SyncPtr(core::ptr::addr_of!(WITNESS));
pub fn backend_entry() {{ std::hint::black_box(&ANCHOR); }}
"#,
        ),
    );
    let result = rustc(artifacts)
        .args([
            "--edition=2024",
            "--crate-name",
            "backend",
            "--crate-type",
            "rlib",
        ])
        .arg(artifacts.argument("", &source).unwrap())
        .arg("-o")
        .arg(artifacts.argument("", &output).unwrap())
        .output()
        .expect("run rustc for backend fixture");
    assert!(
        result.status.success(),
        "backend fixture failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    output
}

fn link_runtime(
    artifacts: &cargo_test_artifacts::CargoTestArtifacts,
    label: &str,
    backend: &Path,
    actual_symbol: &str,
) -> Output {
    let root = artifacts.path();
    let source = root.join(format!("runtime_{label}.rs"));
    let output = root.join(format!("runtime_{label}{}", std::env::consts::EXE_SUFFIX));
    write(
        &source,
        &format!(
            r#"
extern crate backend;
#[used]
#[unsafe(export_name = "{actual_symbol}")]
pub static ACTUAL: u8 = 0;
fn main() {{ backend::backend_entry(); }}
"#,
        ),
    );
    rustc(artifacts)
        .args(["--edition=2024"])
        .arg(artifacts.argument("", &source).unwrap())
        .arg("--extern")
        .arg(artifacts.argument("backend=", backend).unwrap())
        .arg("-o")
        .arg(artifacts.argument("", &output).unwrap())
        .output()
        .expect("run rustc for runtime fixture")
}

#[test]
fn split_backend_runtime_generated_object_abi_fails_closed_in_both_directions() {
    let artifacts = cargo_test_artifacts::CargoTestArtifacts::new("generated-object-link")
        .expect("create link-contract outputs within Cargo image custody");
    let gil_backend = compile_backend(&artifacts, "gil", GENERATED_OBJECT_ABI_GIL_SYMBOL);
    let free_backend = compile_backend(
        &artifacts,
        "free",
        GENERATED_OBJECT_ABI_FREE_THREADED_SYMBOL,
    );

    for (label, backend, actual, should_link) in [
        (
            "gil_gil",
            &gil_backend,
            GENERATED_OBJECT_ABI_GIL_SYMBOL,
            true,
        ),
        (
            "free_free",
            &free_backend,
            GENERATED_OBJECT_ABI_FREE_THREADED_SYMBOL,
            true,
        ),
        (
            "gil_free",
            &gil_backend,
            GENERATED_OBJECT_ABI_FREE_THREADED_SYMBOL,
            false,
        ),
        (
            "free_gil",
            &free_backend,
            GENERATED_OBJECT_ABI_GIL_SYMBOL,
            false,
        ),
    ] {
        let result = link_runtime(&artifacts, label, backend, actual);
        assert_eq!(
            result.status.success(),
            should_link,
            "{label} link verdict mismatch:\n{}",
            String::from_utf8_lossy(&result.stderr)
        );
        if !should_link {
            let stderr = String::from_utf8_lossy(&result.stderr);
            let expected = if label == "gil_free" {
                GENERATED_OBJECT_ABI_GIL_SYMBOL
            } else {
                GENERATED_OBJECT_ABI_FREE_THREADED_SYMBOL
            };
            assert!(
                stderr.contains(expected),
                "{label} must name missing ABI symbol {expected}:\n{stderr}"
            );
        }
    }
}
