use molt_codegen_abi::{
    ATTACHED_EXECUTION_CONTEXT_ABI_SUFFIX, GENERATED_OBJECT_ABI_FREE_THREADED_SYMBOL,
    GENERATED_OBJECT_ABI_GIL_SYMBOL, GENERATED_OBJECT_ABI_SYMBOL,
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

#[test]
fn incompatible_pointer_record_refcount_and_context_revisions_cannot_link() {
    let artifacts = cargo_test_artifacts::CargoTestArtifacts::new("pointer-abi-link")
        .expect("create pointer ABI link-contract outputs within Cargo image custody");
    for (mode, old_symbol, new_symbol) in [
        (
            "gil_context_v7",
            "molt_generated_object_abi_69bab6cda2adac1b_gil_v7",
            GENERATED_OBJECT_ABI_GIL_SYMBOL,
        ),
        (
            "free_context_v7",
            "molt_generated_object_abi_69bab6cda2adac1b_free_threaded_v7",
            GENERATED_OBJECT_ABI_FREE_THREADED_SYMBOL,
        ),
        (
            "gil",
            "molt_generated_object_abi_bf06a9269171acab_gil_v3",
            GENERATED_OBJECT_ABI_GIL_SYMBOL,
        ),
        (
            "free",
            "molt_generated_object_abi_bf06a9269171acab_free_threaded_v3",
            GENERATED_OBJECT_ABI_FREE_THREADED_SYMBOL,
        ),
        (
            "gil_refcount_v7",
            "molt_generated_object_abi_69bab6cda2adac1b_gil_v7",
            GENERATED_OBJECT_ABI_GIL_SYMBOL,
        ),
        (
            "free_refcount_v7",
            "molt_generated_object_abi_69bab6cda2adac1b_free_threaded_v7",
            GENERATED_OBJECT_ABI_FREE_THREADED_SYMBOL,
        ),
        (
            "gil_frame_location_v7",
            "molt_generated_object_abi_16b12518d4094b67_gil_v7",
            GENERATED_OBJECT_ABI_GIL_SYMBOL,
        ),
        (
            "free_frame_location_v7",
            "molt_generated_object_abi_16b12518d4094b67_free_threaded_v7",
            GENERATED_OBJECT_ABI_FREE_THREADED_SYMBOL,
        ),
        (
            "gil_refcount_v6",
            "molt_generated_object_abi_edf88c8f320447e7_gil_v6",
            GENERATED_OBJECT_ABI_GIL_SYMBOL,
        ),
        (
            "free_refcount_v6",
            "molt_generated_object_abi_edf88c8f320447e7_free_threaded_v6",
            GENERATED_OBJECT_ABI_FREE_THREADED_SYMBOL,
        ),
        (
            "gil_record_v5",
            "molt_generated_object_abi_cc3e3a1cd01ab8fb_gil_v5",
            GENERATED_OBJECT_ABI_GIL_SYMBOL,
        ),
        (
            "free_record_v5",
            "molt_generated_object_abi_cc3e3a1cd01ab8fb_free_threaded_v5",
            GENERATED_OBJECT_ABI_FREE_THREADED_SYMBOL,
        ),
    ] {
        for (direction, expected, actual) in [
            ("old_new", old_symbol, new_symbol),
            ("new_old", new_symbol, old_symbol),
        ] {
            let label = format!("{mode}_{direction}");
            let backend = compile_backend(&artifacts, &label, expected);
            let result = link_runtime(&artifacts, &label, &backend, actual);
            assert!(
                !result.status.success(),
                "{label} must reject mixed generated-code contracts"
            );
            assert!(
                String::from_utf8_lossy(&result.stderr).contains(expected),
                "{label} must fail on missing ABI witness {expected}: {}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum ArchiveContract {
    Current,
    PreContextEntry,
    PreContextRuntimeCall,
}

fn compile_selected_archive(
    artifacts: &cargo_test_artifacts::CargoTestArtifacts,
    contract: ArchiveContract,
) -> PathBuf {
    let suffix = ATTACHED_EXECUTION_CONTEXT_ABI_SUFFIX;
    let current_entry = format!("context_archive_entry{suffix}");
    let body = match contract {
        ArchiveContract::Current => format!(
            r#"
unsafe extern "C" {{
    #[link_name = "molt_int_from_i64{suffix}"]
    fn runtime_step(context: u64, value: i64) -> u64;
}}
#[unsafe(export_name = "{current_entry}")]
pub unsafe extern "C" fn archived_entry(context: u64, value: i64) -> u64 {{
    unsafe {{ runtime_step(context, value) }}
}}
"#
        ),
        ArchiveContract::PreContextEntry => r#"
#[unsafe(export_name = "context_archive_entry")]
pub extern "C" fn archived_entry(value: i64) -> u64 { value as u64 }
"#
        .to_owned(),
        ArchiveContract::PreContextRuntimeCall => format!(
            r#"
unsafe extern "C" {{
    #[link_name = "molt_int_from_i64"]
    fn runtime_step(value: i64) -> u64;
}}
#[unsafe(export_name = "{current_entry}")]
pub unsafe extern "C" fn archived_entry(_context: u64, value: i64) -> u64 {{
    unsafe {{ runtime_step(value) }}
}}
"#
        ),
    };
    let source = artifacts.path().join(format!("archive_{contract:?}.rs"));
    let archive = artifacts
        .path()
        .join(format!("libarchive_{contract:?}.rlib"));
    write(
        &source,
        &format!(
            r#"
{body}
// This boundary selects the archive member independently of its Python entry.
#[unsafe(no_mangle)]
pub extern "C" fn select_archive_member() -> u64 {{ 0x198 }}
unsafe extern "C" {{ fn intentionally_unprovided_dead_branch(); }}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn unreferenced_dead_branch() {{
    unsafe {{ intentionally_unprovided_dead_branch() }};
}}
"#
        ),
    );
    let result = rustc(artifacts)
        .args([
            "--edition=2024",
            "--crate-name=archived",
            "--crate-type=rlib",
            "-Ccodegen-units=1",
        ])
        .arg(artifacts.argument("", &source).unwrap())
        .arg("-o")
        .arg(artifacts.argument("", &archive).unwrap())
        .output()
        .expect("compile actual selected archive member");
    assert!(
        result.status.success(),
        "{contract:?}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    archive
}

fn link_selected_archive(
    artifacts: &cargo_test_artifacts::CargoTestArtifacts,
    label: &str,
    backend: &Path,
    archive: &Path,
) -> (Output, PathBuf) {
    let suffix = ATTACHED_EXECUTION_CONTEXT_ABI_SUFFIX;
    let witness = GENERATED_OBJECT_ABI_SYMBOL;
    let source = artifacts.path().join(format!("mixed_{label}.rs"));
    let executable = artifacts
        .path()
        .join(format!("mixed_{label}{}", std::env::consts::EXE_SUFFIX));
    write(
        &source,
        &format!(
            r#"
extern crate backend;
extern crate archived;
#[used]
#[unsafe(export_name = "{witness}")]
pub static CURRENT_WITNESS: u8 = 0;
#[repr(C)]
struct ContextHeader {{ thread_record: u64 }}
static RECORD: u64 = 0x670;
static mut CONTEXT: ContextHeader = ContextHeader {{ thread_record: 0 }};
#[unsafe(export_name = "molt_int_from_i64{suffix}")]
pub unsafe extern "C" fn current_runtime_step(context: u64, value: i64) -> u64 {{
    assert_eq!(context, core::ptr::addr_of!(CONTEXT) as u64);
    assert_eq!(unsafe {{ *(context as *const u64) }}, core::ptr::addr_of!(RECORD) as u64);
    assert_eq!(value, 42);
    0x7ff9_0000_0000_002a
}}
unsafe extern "C" {{
    fn select_archive_member() -> u64;
    #[link_name = "context_archive_entry{suffix}"]
    fn archived_entry(context: u64, value: i64) -> u64;
}}
fn main() {{
    // An independently compiled, current object contributes the normal
    // current witness. It cannot qualify this selected archive member's ABI.
    backend::backend_entry();
    assert_eq!(unsafe {{ select_archive_member() }}, 0x198);
    unsafe {{ CONTEXT.thread_record = core::ptr::addr_of!(RECORD) as u64; }}
    let context = core::ptr::addr_of!(CONTEXT) as u64;
    assert_eq!(unsafe {{ archived_entry(context, 42) }}, 0x7ff9_0000_0000_002a);
    println!("selected archive forwarded exact context and value");
}}
"#
        ),
    );
    let mut command = rustc(artifacts);
    command
        .args(["--edition=2024", "-Clto=off", "-Clink-dead-code=no"])
        .arg(artifacts.argument("", &source).unwrap())
        .arg("--extern")
        .arg(artifacts.argument("backend=", backend).unwrap())
        .arg("--extern")
        .arg(artifacts.argument("archived=", archive).unwrap())
        .arg("-o")
        .arg(artifacts.argument("", &executable).unwrap());
    if cfg!(target_os = "macos") {
        command.arg("-Clink-arg=-Wl,-dead_strip");
    } else if cfg!(all(target_os = "windows", target_env = "msvc")) {
        command.arg("-Clink-arg=/OPT:REF");
    } else {
        command.arg("-Clink-arg=-Wl,--gc-sections");
    }
    (
        command
            .output()
            .expect("final-link selected archive with unrelated current witness"),
        executable,
    )
}

#[test]
fn current_witness_cannot_qualify_stale_archive_entries_or_runtime_calls_under_dead_strip() {
    let artifacts = cargo_test_artifacts::CargoTestArtifacts::new("context-archive-link")
        .expect("retain mixed-object context ABI fixture custody");
    let backend = compile_backend(&artifacts, "current_witness", GENERATED_OBJECT_ABI_SYMBOL);
    for (contract, missing) in [
        (ArchiveContract::Current, None),
        (
            ArchiveContract::PreContextEntry,
            Some(format!(
                "context_archive_entry{ATTACHED_EXECUTION_CONTEXT_ABI_SUFFIX}"
            )),
        ),
        (
            ArchiveContract::PreContextRuntimeCall,
            Some("molt_int_from_i64".to_owned()),
        ),
    ] {
        let archive = compile_selected_archive(&artifacts, contract);
        let label = format!("{contract:?}");
        let (linked, executable) = link_selected_archive(&artifacts, &label, &backend, &archive);
        let stderr = String::from_utf8_lossy(&linked.stderr);
        if let Some(missing) = missing {
            assert!(
                !linked.status.success(),
                "{contract:?} was qualified by another object's current witness"
            );
            assert!(
                stderr.contains(&missing),
                "{contract:?}: missing physical contract {missing}: {stderr}"
            );
        } else {
            // Its unreachable unresolved branch makes this a real dead-strip
            // control, while the executed selector proves archive extraction.
            assert!(
                linked.status.success(),
                "current mixed archive failed: {stderr}"
            );
            let output = artifacts
                .command(&executable)
                .expect("owned executable")
                .output()
                .expect("execute current mixed-object ABI fixture");
            assert!(
                output.status.success(),
                "current archive execution failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                String::from_utf8_lossy(&output.stdout).trim(),
                "selected archive forwarded exact context and value"
            );
        }
    }
}
