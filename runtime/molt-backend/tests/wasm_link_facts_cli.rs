#![cfg(feature = "wasm-backend")]

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use wasm_encoder::{CodeSection, Function, FunctionSection, Instruction, Module, TypeSection};

struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn temp_wasm(name: &str, bytes: &[u8]) -> TempFile {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "molt-wasm-link-facts-{name}-{}-{nonce}.wasm",
        std::process::id()
    ));
    fs::write(&path, bytes).expect("write wasm fixture");
    TempFile(path)
}

fn clean_module() -> Vec<u8> {
    let mut module = Module::new();
    let mut types = TypeSection::new();
    types.ty().function([], []);
    module.section(&types);
    let mut functions = FunctionSection::new();
    functions.function(0);
    functions.function(0);
    module.section(&functions);
    let mut code = CodeSection::new();
    let mut body = Function::new([]);
    body.instruction(&Instruction::Call(1));
    body.instruction(&Instruction::End);
    code.function(&body);
    let mut callee = Function::new([]);
    callee.instruction(&Instruction::Nop);
    callee.instruction(&Instruction::End);
    code.function(&callee);
    module.section(&code);
    module.finish()
}

#[test]
fn cli_emits_versioned_success_json() {
    let fixture = temp_wasm("ok", &clean_module());

    let output = Command::new(env!("CARGO_BIN_EXE_molt-backend"))
        .arg("--scan-wasm-link-facts")
        .arg(&fixture.0)
        .output()
        .expect("run facts CLI");

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: Value = serde_json::from_slice(&output.stdout).expect("parse success JSON");
    assert_eq!(payload["schema_version"], 7);
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["facts"]["schema_version"], 7);
    assert_eq!(payload["facts"]["code_body_count"], 2);
    assert_eq!(payload["facts"]["defined_memory_count"], 0);
    assert_eq!(
        payload["facts"]["custom_section_names"],
        serde_json::json!([])
    );
    assert_eq!(
        payload["facts"]["reachable_function_indices"],
        serde_json::json!([])
    );
    assert_eq!(
        payload["facts"]["referenced_function_indices"],
        serde_json::json!([1])
    );
    assert!(payload["facts"].get("function_references").is_none());
}

#[test]
fn cli_emits_versioned_error_json_and_nonzero_exit() {
    let fixture = temp_wasm("bad", b"\0asm\x01\0\0");

    let output = Command::new(env!("CARGO_BIN_EXE_molt-backend"))
        .arg("--scan-wasm-link-facts")
        .arg(&fixture.0)
        .output()
        .expect("run facts CLI");

    assert_eq!(output.status.code(), Some(2));
    let payload: Value = serde_json::from_slice(&output.stdout).expect("parse error JSON");
    assert_eq!(payload["schema_version"], 7);
    assert_eq!(payload["ok"], false);
    assert!(
        payload["error"]
            .as_str()
            .is_some_and(|error| !error.is_empty())
    );
}

#[test]
fn cli_missing_path_is_a_structured_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_molt-backend"))
        .arg("--scan-wasm-link-facts")
        .output()
        .expect("run facts CLI");

    assert_eq!(output.status.code(), Some(2));
    let payload: Value = serde_json::from_slice(&output.stdout).expect("parse missing-path JSON");
    assert_eq!(payload["ok"], false);
    assert!(
        payload["error"]
            .as_str()
            .is_some_and(|error| error.contains("requires a wasm path"))
    );
}

#[test]
fn cli_publishes_and_validates_rust_owned_attestation() {
    let fixture = temp_wasm("publish-input", &clean_module());
    let published = temp_wasm("publish-output", b"");

    let output = Command::new(env!("CARGO_BIN_EXE_molt-backend"))
        .arg("--publish-wasm-link-facts")
        .arg(&fixture.0)
        .arg("--output")
        .arg(&published.0)
        .output()
        .expect("publish facts");

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: Value = serde_json::from_slice(&output.stdout).expect("parse publication JSON");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["facts"]["callable_table_attestation_present"], true);
    assert!(fs::metadata(&published.0).expect("published wasm").len() > 8);
    let rescanned = Command::new(env!("CARGO_BIN_EXE_molt-backend"))
        .arg("--scan-wasm-link-facts")
        .arg(&published.0)
        .output()
        .expect("rescan separate published output");
    assert!(rescanned.status.success());
    let actual: Value = serde_json::from_slice(&rescanned.stdout).expect("parse rescan JSON");
    assert_eq!(payload["facts"], actual["facts"]);
}

#[test]
fn cli_publishes_in_place_through_one_atomic_commit() {
    let fixture = temp_wasm("publish-in-place", &clean_module());

    let output = Command::new(env!("CARGO_BIN_EXE_molt-backend"))
        .arg("--publish-wasm-link-facts")
        .arg(&fixture.0)
        .arg("--output")
        .arg(&fixture.0)
        .output()
        .expect("publish facts in place");

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rescanned = Command::new(env!("CARGO_BIN_EXE_molt-backend"))
        .arg("--scan-wasm-link-facts")
        .arg(&fixture.0)
        .output()
        .expect("rescan in-place publication");
    assert!(rescanned.status.success());
    let publication: Value =
        serde_json::from_slice(&output.stdout).expect("parse publication JSON");
    let payload: Value =
        serde_json::from_slice(&rescanned.stdout).expect("parse in-place publication JSON");
    assert_eq!(payload["facts"]["callable_table_attestation_present"], true);
    assert_eq!(publication["facts"], payload["facts"]);
}

#[test]
fn failed_publication_never_exposes_partial_destination() {
    let mut malformed = clean_module();
    malformed.pop();
    let fixture = temp_wasm("publish-malformed", &malformed);
    let destination = temp_wasm("publish-existing", b"sentinel-generation");

    let output = Command::new(env!("CARGO_BIN_EXE_molt-backend"))
        .arg("--publish-wasm-link-facts")
        .arg(&fixture.0)
        .arg("--output")
        .arg(&destination.0)
        .output()
        .expect("run failing publication");

    assert!(!output.status.success());
    assert_eq!(
        fs::read(&destination.0).expect("read preserved destination"),
        b"sentinel-generation"
    );
}

#[test]
fn final_publication_rejects_scannable_linker_metadata_atomically() {
    for (name, metadata) in [
        ("linking", b"\0\x09\x07linking\x02".as_slice()),
        // Type/function/code are ordinals 0/1/2; the empty symbol table
        // supplies the scanner's authority for the zero-entry code relocation.
        (
            "reloc.CODE",
            b"\0\x0c\x07linking\x02\x08\x01\x00\0\x0d\x0areloc.CODE\x02\x00".as_slice(),
        ),
        // A different relocation kind independently exercises the prefix guard.
        ("reloc.TYPE", b"\0\x0d\x0areloc.TYPE\x00\x00".as_slice()),
    ] {
        let mut module = clean_module();
        module.extend_from_slice(metadata);
        let fixture = temp_wasm(name, &module);
        let destination = temp_wasm("linked-generation", b"sentinel-generation");
        let scan = Command::new(env!("CARGO_BIN_EXE_molt-backend"))
            .arg("--scan-wasm-link-facts")
            .arg(&fixture.0)
            .output()
            .expect("scan valid linker metadata");
        assert!(
            scan.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&scan.stdout)
        );

        let publication = Command::new(env!("CARGO_BIN_EXE_molt-backend"))
            .arg("--publish-wasm-link-facts")
            .arg(&fixture.0)
            .arg("--output")
            .arg(&destination.0)
            .output()
            .expect("refuse final publication of linker metadata");
        assert_eq!(publication.status.code(), Some(2));
        let error: Value = serde_json::from_slice(&publication.stdout).expect("structured refusal");
        assert_eq!(error["ok"], false);
        assert!(
            error["error"]
                .as_str()
                .expect("error diagnostic")
                .contains("requires a linked module without linker metadata")
        );
        assert_eq!(
            fs::read(&destination.0).expect("prior output"),
            b"sentinel-generation"
        );
        assert_eq!(fs::read(&fixture.0).expect("original input"), module);
    }
}
