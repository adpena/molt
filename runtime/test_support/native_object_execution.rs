// Final-link execution of one generated native object (Cranelift or LLVM)
// against a no_std Rust provider crate and a Rust harness. Include beside
// `cargo_test_artifacts`; every input, archive and image stays in Cargo image
// custody.

use super::cargo_test_artifacts::CargoTestArtifacts;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Resolve the fixture compiler; actual compilation must fail if it is unavailable.
pub fn rustc_path() -> PathBuf {
    std::env::var_os("RUSTC")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("rustc"))
}

fn run_checked(command: &mut Command, purpose: &str) {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("{purpose}: failed to start: {error}"));
    assert!(
        output.status.success(),
        "{purpose}: status={}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Compile `provider_source_text` to a Rust archive, final-link the generated
/// object with it into `harness_source_text`, and run the resulting binary.
pub fn link_and_run_native_object(
    rustc: &Path,
    artifact_name: &str,
    object_bytes: Vec<u8>,
    provider_source_text: &str,
    harness_source_text: &str,
    purpose: &str,
) {
    let artifacts = CargoTestArtifacts::new(artifact_name).unwrap_or_else(|error| {
        panic!("create {purpose} outputs within Cargo image custody: {error:?}")
    });
    let temp = artifacts.path();
    let app_object = temp.join("native_callable_app.o");
    let provider_source = temp.join("provider.rs");
    let provider_archive = temp.join("libnative_callable_provider.rlib");
    let harness_source = temp.join("harness.rs");
    let executable = temp.join(if cfg!(windows) {
        "native_callable_execution.exe"
    } else {
        "native_callable_execution"
    });
    fs::write(&app_object, object_bytes).expect("write generated app object");
    fs::write(&provider_source, provider_source_text).expect("write native provider source");
    run_checked(
        artifacts
            .command(rustc)
            .expect("resolve fixture compiler")
            .arg("--edition=2021")
            .arg("--crate-name=native_callable_provider")
            .arg("--crate-type=rlib")
            .arg("-Cpanic=abort")
            .arg(artifacts.argument("", &provider_source).unwrap())
            .arg("-o")
            .arg(artifacts.argument("", &provider_archive).unwrap()),
        &format!("compile {purpose} provider Rust archive"),
    );
    // Let rustc order the provider before its transitive core/compiler-builtins
    // dependencies. A raw link-arg archive arrives after those libraries and
    // leaves AArch64 outlined atomics unresolved. This explicit crate use also
    // retains providers reached only from the generated object's C ABI calls.
    fs::write(
        &harness_source,
        format!("{harness_source_text}\nextern crate native_callable_provider;\n"),
    )
    .expect("write native execution harness");
    run_checked(
        artifacts
            .command(rustc)
            .expect("resolve fixture compiler")
            .arg("--edition=2021")
            .arg("-Cpanic=abort")
            .arg(artifacts.argument("", &harness_source).unwrap())
            .arg("--extern")
            .arg(
                artifacts
                    .argument("native_callable_provider=", &provider_archive)
                    .unwrap(),
            )
            .arg("-C")
            .arg(artifacts.argument("link-arg=", &app_object).unwrap())
            .arg("-o")
            .arg(artifacts.argument("", &executable).unwrap()),
        &format!("final-link generated object with {purpose} provider archive"),
    );
    run_checked(
        &mut Command::new(&executable),
        &format!("execute final-linked {purpose} binary"),
    );
}
