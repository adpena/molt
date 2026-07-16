use molt_codegen_abi::{
    GENERATED_OBJECT_ABI_FREE_THREADED_SYMBOL, GENERATED_OBJECT_ABI_GIL_SYMBOL,
};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn rustc() -> Command {
    Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
}

fn write(path: &Path, source: &str) {
    std::fs::write(path, source).expect("write link-contract fixture");
}

fn compile_backend(root: &Path, label: &str, expected_symbol: &str) -> PathBuf {
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
    let result = rustc()
        .args([
            "--edition=2024",
            "--crate-name",
            "backend",
            "--crate-type",
            "rlib",
        ])
        .arg(&source)
        .arg("-o")
        .arg(&output)
        .output()
        .expect("run rustc for backend fixture");
    assert!(
        result.status.success(),
        "backend fixture failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    output
}

fn link_runtime(root: &Path, label: &str, backend: &Path, actual_symbol: &str) -> Output {
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
    rustc()
        .args(["--edition=2024"])
        .arg(&source)
        .arg("--extern")
        .arg(format!("backend={}", backend.display()))
        .arg("-o")
        .arg(output)
        .output()
        .expect("run rustc for runtime fixture")
}

#[test]
fn split_backend_runtime_generated_object_abi_fails_closed_in_both_directions() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "molt-generated-object-link-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).expect("create link-contract temp dir");

    let gil_backend = compile_backend(&root, "gil", GENERATED_OBJECT_ABI_GIL_SYMBOL);
    let free_backend = compile_backend(&root, "free", GENERATED_OBJECT_ABI_FREE_THREADED_SYMBOL);

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
        let result = link_runtime(&root, label, backend, actual);
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

    let _ = std::fs::remove_dir_all(root);
}
