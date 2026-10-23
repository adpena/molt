use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
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

fn publish(module: &[u8], output: &std::path::Path) -> Output {
    run_facts(
        &[
            "--publish-wasm-link-facts".as_ref(),
            "--output".as_ref(),
            output.as_os_str(),
        ],
        module,
    )
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
    let output = scan(b"\0asm\x01\0\0");

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

#[test]
fn cli_publishes_and_validates_rust_owned_attestation() {
    let published = temp_wasm("publish-output", b"");

    let output = publish(&clean_module(), &published.0);

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: Value = serde_json::from_slice(&output.stdout).expect("parse publication JSON");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["facts"]["callable_table_attestation_present"], true);
    let published_bytes = fs::read(&published.0).expect("published wasm");
    assert!(published_bytes.len() > 8);
    let rescanned = scan(&published_bytes);
    assert!(rescanned.status.success());
    let actual: Value = serde_json::from_slice(&rescanned.stdout).expect("parse rescan JSON");
    assert_eq!(payload["facts"], actual["facts"]);
}

#[test]
fn cli_publication_replaces_an_existing_generation_in_one_commit() {
    let destination = temp_wasm("publish-replace", b"previous-generation");

    let output = publish(&clean_module(), &destination.0);

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rescanned = scan(&fs::read(&destination.0).expect("replaced generation"));
    assert!(rescanned.status.success());
    let publication: Value =
        serde_json::from_slice(&output.stdout).expect("parse publication JSON");
    let payload: Value =
        serde_json::from_slice(&rescanned.stdout).expect("parse replaced publication JSON");
    assert_eq!(payload["facts"]["callable_table_attestation_present"], true);
    assert_eq!(publication["facts"], payload["facts"]);
    let directory = destination.0.parent().expect("fixture directory");
    let name = destination
        .0
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(
        fs::read_dir(directory)
            .unwrap()
            .filter_map(Result::ok)
            .all(|entry| !entry
                .file_name()
                .to_string_lossy()
                .starts_with(&format!(".{name}"))),
        "a committed publication leaves no private stage"
    );
}

#[test]
fn failed_publication_never_exposes_partial_destination() {
    let mut malformed = clean_module();
    malformed.pop();
    let destination = temp_wasm("publish-existing", b"sentinel-generation");

    let output = publish(&malformed, &destination.0);

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
        let destination = temp_wasm("linked-generation", b"sentinel-generation");
        let scanned = scan(&module);
        assert!(
            scanned.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&scanned.stdout)
        );

        let publication = publish(&module, &destination.0);
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
    }
}

#[test]
fn publication_without_output_refuses_before_reading_input() {
    let output = run_facts(&["--publish-wasm-link-facts".as_ref()], &clean_module());
    assert_eq!(output.status.code(), Some(2));
    let payload: Value = serde_json::from_slice(&output.stdout).expect("structured refusal");
    assert_eq!(payload["ok"], false);
    assert!(
        payload["error"]
            .as_str()
            .is_some_and(|error| error.contains("requires --output"))
    );
}
