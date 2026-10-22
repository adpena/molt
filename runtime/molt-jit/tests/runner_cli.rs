//! The runner's command line. A runner that fails before its program starts
//! leaves no start receipt, so a caller can tell that failure from the
//! program's own exit status.

#[expect(
    dead_code,
    reason = "these tests need only the owned artifact directory"
)]
mod cargo_test_artifacts {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test_support/cargo_test_artifacts.rs"
    ));
}

use std::process::Command;

const RUNNER: &str = env!("CARGO_BIN_EXE_molt-jit");

#[test]
fn a_launch_that_fails_before_the_program_starts_leaves_no_start_receipt() {
    let artifacts = cargo_test_artifacts::CargoTestArtifacts::new("jit-start-receipt")
        .expect("Cargo test artifact root");
    let receipt = artifacts.path().join("started.json");
    let output = Command::new(RUNNER)
        .arg("--start-receipt")
        .arg(&receipt)
        .arg("--image")
        .arg(artifacts.path().join("missing_molt.jit"))
        .arg("program-argument")
        .output()
        .expect("run molt-jit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(125), "{stderr}");
    assert!(stderr.starts_with("molt-jit: "), "{stderr}");
    assert!(
        !receipt.exists(),
        "a failed launch wrote {}",
        receipt.display()
    );
}

#[test]
fn the_runner_needs_an_image_and_a_receipt_path() {
    for args in [
        &["--start-receipt"][..],
        &["--start-receipt", "r.json"][..],
        &["--image"][..],
        &[
            "--start-receipt",
            "r.json",
            "--start-receipt",
            "s.json",
            "--image",
            "a.jit",
        ][..],
        &["program"][..],
    ] {
        let output = Command::new(RUNNER)
            .args(args)
            .output()
            .expect("run molt-jit");
        assert_eq!(output.status.code(), Some(2), "{args:?}");
    }
}
