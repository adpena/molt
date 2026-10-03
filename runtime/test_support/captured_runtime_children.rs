// Shared capture for runtime tests that re-execute their own Cargo test image.
// The owning test keeps its semantic assertions. This helper retains the
// child's complete streams in Cargo test-image custody and publishes one typed,
// source/image-bound record on the owner's stderr for the receipt consumers in
// tools/runtime_descendant_receipts.py, which derive the mandatory roster from
// the owner's completed libtest rows rather than from records that happen to
// be present.
//
// The record is written straight to the stderr handle: libtest output capture
// covers only the print macros, so the record survives captured runs, and the
// stdout libtest protocol is never interleaved with it.

use super::cargo_test_artifacts::CargoTestArtifacts;
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, ExitStatus, Output};

const RECORD_PREFIX: &str = "MOLT_RUNTIME_DESCENDANT_RECEIPT ";
const RECORD_SCHEMA: &str = "molt.runtime-descendant.v1";
const SOURCE_IDENTITY_ENV: &str = "MOLT_TEST_SOURCE_IDENTITY_JSON";
const STREAM_LABEL: &str = "runtime-descendant";

fn hex(digest: &[u8]) -> String {
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(encoded, "{byte:02x}").expect("formatting into String cannot fail");
    }
    encoded
}

fn file_sha256(path: &Path) -> String {
    let mut file = std::fs::File::open(path).expect("open owner test image");
    let mut digest = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = file.read(&mut buffer).expect("read owner test image");
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    hex(digest.finalize().as_ref())
}

fn utf8(value: &OsStr) -> String {
    value
        .to_str()
        .expect("runtime descendant argv must be UTF-8")
        .to_owned()
}

// One typed vocabulary with tools/cargo_test_binary_runner.py: a POSIX signal
// is never an exit code, and Windows reports NTSTATUS terminations (abort is a
// fast-fail STATUS_STACK_BUFFER_OVERRUN) through the 32-bit exit code.
fn termination(status: ExitStatus) -> serde_json::Value {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return serde_json::json!({"kind": "signal", "signal": signal});
        }
    }
    let code = status
        .code()
        .expect("runtime descendant terminated without an exit status");
    #[cfg(windows)]
    let (kind, code) = {
        let raw = code as u32;
        let kind = if raw & 0xC000_0000 == 0xC000_0000 {
            "windows-exception"
        } else {
            "exit"
        };
        (kind, i64::from(raw))
    };
    #[cfg(not(windows))]
    let (kind, code) = ("exit", i64::from(code));
    serde_json::json!({"kind": kind, "code": code})
}

fn retain(owner: &CargoTestArtifacts, stream: &str, bytes: &[u8]) -> serde_json::Value {
    let path = owner.path().join(format!("{stream}.log"));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .expect("exclusive runtime descendant stream");
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .expect("retain complete runtime descendant stream");
    serde_json::json!({
        "path": path,
        "bytes": bytes.len(),
        "sha256": hex(Sha256::digest(bytes).as_ref()),
    })
}

/// Run an exact self-image child and publish its descendant record.
///
/// `role` names the owning family and `mode` the child variant; the owning
/// test identity comes from libtest's test-thread name, not from the caller.
pub fn capture(command: &mut Command, role: &str, mode: &str) -> Output {
    let image = std::env::current_exe()
        .and_then(|path| path.canonicalize())
        .expect("canonical owner test image");
    let program = command.get_program();
    assert_eq!(
        Path::new(program)
            .canonicalize()
            .expect("canonical runtime descendant executable"),
        image,
        "runtime descendants must re-execute their owner's own test image",
    );
    let mut argv = vec![utf8(program)];
    argv.extend(command.get_args().map(utf8));
    let child_test = argv
        .windows(2)
        .find(|pair| pair[0] == "--exact")
        .map(|pair| pair[1].clone())
        .expect("runtime descendants require one exact child selection");
    let parent_test = std::thread::current()
        .name()
        .expect("runtime descendants require their libtest-named owning test")
        .to_owned();
    let source_identity = match std::env::var(SOURCE_IDENTITY_ENV) {
        Ok(value) => serde_json::from_str::<serde_json::Value>(&value)
            .expect("admitted source identity must be JSON"),
        Err(std::env::VarError::NotPresent) => serde_json::Value::Null,
        Err(error) => panic!("invalid source identity transport: {error}"),
    };
    let executable_sha256 = file_sha256(&image);
    let output = command.output().expect("execute runtime descendant");
    let owner = CargoTestArtifacts::new(STREAM_LABEL)
        .expect("exclusive Cargo test custody for runtime descendant streams");
    let record = serde_json::json!({
        "schema": RECORD_SCHEMA,
        "role": role,
        "parent_test": parent_test,
        "child_test": child_test,
        "mode": mode,
        "source_identity": source_identity,
        "executable": image,
        "executable_sha256": executable_sha256,
        "argv": argv,
        "termination": termination(output.status),
        "coordinate_authority": "unavailable",
        "stdout": retain(&owner, "stdout", &output.stdout),
        "stderr": retain(&owner, "stderr", &output.stderr),
    });
    let line = format!("\n{RECORD_PREFIX}{record}\n");
    let mut stderr = std::io::stderr().lock();
    stderr
        .write_all(line.as_bytes())
        .and_then(|()| stderr.flush())
        .expect("publish runtime descendant record");
    output
}
