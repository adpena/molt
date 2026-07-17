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
    assert_eq!(payload["schema_version"], 3);
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["facts"]["schema_version"], 3);
    assert_eq!(payload["facts"]["code_body_count"], 2);
    assert_eq!(payload["facts"]["function_references"][0][0], 0);
    assert_eq!(
        payload["facts"]["function_references"][0]
            .as_array()
            .map(Vec::len),
        Some(3)
    );
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
    assert_eq!(payload["schema_version"], 3);
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
    let payload: Value =
        serde_json::from_slice(&rescanned.stdout).expect("parse in-place publication JSON");
    assert_eq!(payload["facts"]["callable_table_attestation_present"], true);
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
