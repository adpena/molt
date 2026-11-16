use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use molt_wasm_facts::WASM_LINK_FACTS_SCHEMA_VERSION;
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

/// Run the facts tool with `args` and the module bytes on stdin.
fn run_facts(args: &[&std::ffi::OsStr], module: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_molt-wasm-facts"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn facts CLI");
    // A refused command may exit before it reads its input.
    let _ = child.stdin.take().expect("stdin").write_all(module);
    child.wait_with_output().expect("run facts CLI")
}

fn scan(module: &[u8]) -> Output {
    run_facts(&["--scan-wasm-link-facts".as_ref()], module)
}

#[test]
fn cli_emits_versioned_success_json() {
    let output = scan(&clean_module());

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: Value = serde_json::from_slice(&output.stdout).expect("parse success JSON");
    assert_eq!(payload["schema_version"], WASM_LINK_FACTS_SCHEMA_VERSION);
    assert_eq!(payload["ok"], true);
    assert_eq!(
        payload["facts"]["schema_version"],
        WASM_LINK_FACTS_SCHEMA_VERSION
    );
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
    let output = scan(b"\0asm\x01\0\0");

    assert_eq!(output.status.code(), Some(2));
    let payload: Value = serde_json::from_slice(&output.stdout).expect("parse error JSON");
    assert_eq!(payload["schema_version"], WASM_LINK_FACTS_SCHEMA_VERSION);
    assert_eq!(payload["ok"], false);
    assert!(
        payload["error"]
            .as_str()
            .is_some_and(|error| !error.is_empty())
    );
}

#[test]
fn cli_input_path_is_a_structured_error() {
    // The module arrives on stdin; a path argument is a protocol error, not
    // a file the tool opens.
    let fixture = temp_wasm("path-argument", &clean_module());
    let output = run_facts(
        &["--scan-wasm-link-facts".as_ref(), fixture.0.as_os_str()],
        &clean_module(),
    );

    assert_eq!(output.status.code(), Some(2));
    let payload: Value = serde_json::from_slice(&output.stdout).expect("parse argument JSON");
    assert_eq!(payload["ok"], false);
    assert!(
        payload["error"]
            .as_str()
            .is_some_and(|error| error.contains("reads the module from stdin"))
    );
}
