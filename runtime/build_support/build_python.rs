//! Python execution authority for Cargo's standard-library table generators.

use std::env;
use std::io::Write;
use std::process::{Command, Stdio};

pub(crate) fn resolve() -> String {
    println!("cargo:rerun-if-changed=../build_support/build_python.rs");
    for key in ["MOLT_BUILD_PYTHON", "PYTHON"] {
        println!("cargo:rerun-if-env-changed={key}");
    }
    for key in ["MOLT_BUILD_PYTHON", "PYTHON"] {
        match env::var(key) {
            Ok(value) if !value.trim().is_empty() => return value.trim().to_owned(),
            Ok(_) | Err(env::VarError::NotPresent) => {}
            Err(error) => panic!("invalid build Python selector {key}: {error}"),
        }
    }
    if cfg!(windows) { "python" } else { "python3" }.to_owned()
}

pub(crate) fn run_script(build_python: &str, script: &str, purpose: &str) -> String {
    // Match the content-attested interpreter's isolated, no-site capture.
    // Cargo's cwd and PYTHONPATH/PYTHONHOME/site startup cannot supply imports.
    let mut child = Command::new(build_python)
        .args(["-B", "-I", "-S", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| {
            panic!("failed to run build Python `{build_python}` for {purpose}: {error}")
        });
    let write_result = child
        .stdin
        .take()
        .expect("piped build Python stdin")
        .write_all(script.as_bytes());
    // Reap even if Python exited before accepting the script; preserve its
    // diagnostic instead of abandoning the child on a broken pipe.
    let output = child.wait_with_output().unwrap_or_else(|error| {
        panic!("failed to wait for build Python `{build_python}` for {purpose}: {error}")
    });
    if !output.status.success() {
        panic!(
            "build Python `{build_python}` {purpose} failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    write_result.unwrap_or_else(|error| {
        panic!("failed to send {purpose} to build Python `{build_python}`: {error}")
    });
    String::from_utf8(output.stdout).unwrap_or_else(|error| {
        panic!("build Python `{build_python}` {purpose} emitted invalid UTF-8: {error}")
    })
}
