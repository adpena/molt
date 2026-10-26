//! Real compiler/runtime consumer proof, separate from synthetic host ABI tests.
//!
//! Build the WASI runtime libtest with `-C link-arg=--export-table`, then provide
//! its immutable artifact path and SHA-256. Cargo builds the real host binary
//! for this integration test. A current manifest binds each copied libtest to its
//! explicit WASI command lifecycle; it does not add Molt application startup.
//! One libtest invocation owns all exact filters: compilation is shared, while
//! the runtime's test transactions own serial per-case state isolation.

use molt_wasm_facts::WASM_EXECUTION_MANIFEST_VERSION;
use molt_wasm_host::sha256_hex;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Instant;

#[path = "support/mod.rs"]
mod support;

use support::host;

// This retained-artifact owner also exposes external-compiler utilities.
#[allow(dead_code)]
mod cargo_test_artifacts {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test_support/cargo_test_artifacts.rs"
    ));
}

const RUNTIME_CASES: [&str; 9] = [
    "concurrency::execution::tests::wasm_shutdown_custody_owns_c_state_without_an_execution_frame",
    "concurrency::execution::tests::wasm_logical_gil_and_execution_nesting_are_distinct",
    // wasm32-wasip1 uses panic=abort. C-state cleanup is observable here;
    // catch_unwind recovery belongs to the native unwind test lane.
    "concurrency::execution::tests::cleanup_owned_entry_destroys_retained_c_thread_state",
    "concurrency::execution::tests::ordinary_scoped_entries_preserve_the_c_error_indicator",
    "cpython_abi_hooks::native_deallocation_tests::shutdown_record_survives_post_drain_native_deallocation_and_republished_c_owners",
    // Exercise the same metadata admission/transport through the production
    // WASI host. No host filesystem or symlink privilege is part of this cell.
    "builtins::io::tests::vfs_metadata_admission_and_call_budget",
    "builtins::io::tests::vfs_metadata_aliases_and_errno_domain",
    "builtins::io::tests::vfs_metadata_samefile_owner_identity",
    "builtins::io::tests::physical_write_blocking_errno_preserves_target_public_domain",
];

const PLATFORM_CASES: [&str; 3] = [
    "synthetic_errno::io_error_tests::raw_wasi_parser_and_guest_projection_preserve_separate_numeric_domains",
    "synthetic_errno::io_error_tests::rawless_io_errors_use_one_declared_guest_policy",
    "synthetic_errno::io_error_tests::wasi_sdk_io_errors_preserve_raw_and_guest_domain_distinction",
];

// Keep the production phase breakdown without per-function compiler traces.
const PHASE_LOG: &str = "off,molt_wasm_host::precompiled=debug";

#[test]
#[ignore = "requires an actual WASI runtime libtest artifact and its SHA-256"]
fn actual_wasi_runtime_lifecycle_cases_through_production_host() {
    run_actual_wasi_cases("wasi-runtime-lifecycle", &RUNTIME_CASES);
}

#[test]
#[ignore = "requires an actual WASI platform libtest artifact and its SHA-256"]
fn actual_wasi_platform_errno_cases_through_production_host() {
    run_actual_wasi_cases("wasi-platform-errno", &PLATFORM_CASES);
}

fn run_actual_wasi_cases(label: &str, cases: &[&str]) {
    let artifact = PathBuf::from(
        std::env::var_os("MOLT_WASI_LIBTEST_ARTIFACT")
            .expect("MOLT_WASI_LIBTEST_ARTIFACT must name the compiled WASI libtest"),
    );
    let expected = std::env::var("MOLT_WASI_LIBTEST_SHA256")
        .expect("MOLT_WASI_LIBTEST_SHA256 must bind that exact compiler artifact");
    let bytes = std::fs::read(&artifact).expect("read compiler-produced WASI libtest");
    let actual = sha256_hex(&bytes);
    assert_eq!(actual, expected, "WASI artifact content changed");

    // Compile once through the actual host producer, then execute its container
    // through the actual loader. This replaces the former JIT-only proof; it
    // does not add a second compilation of this large retained WASI module.
    let owned = cargo_test_artifacts::CargoTestArtifacts::new(label)
        .expect("retain lifecycle inputs under the actual Cargo test image");
    let source = owned.path().join("lifecycle.wasm");
    molt_artifact_publish::write_bytes_atomically(&source, &bytes)
        .expect("publish captured lifecycle bytes");
    let manifest = owned.path().join("manifest.json");
    let deployment = serde_json::json!({
        "version": WASM_EXECUTION_MANIFEST_VERSION,
        "mode": "linked",
        "modules": {"linked": {"path": "lifecycle.wasm", "size": bytes.len(), "sha256": actual}},
        "entry": {"module": "linked", "function": "_start"},
    });
    molt_artifact_publish::write_bytes_atomically(
        &manifest,
        &serde_json::to_vec(&deployment).expect("encode lifecycle manifest"),
    )
    .expect("publish lifecycle manifest after its captured module");
    drop(bytes);
    println!(
        "wasi-lifecycle retained_inputs={} manifest={deployment}",
        owned.path().display()
    );
    let native = owned.path().join("lifecycle.molt.cwasm");
    let started = Instant::now();
    let precompiled = host()
        .env("MOLT_WASM_HOST_LOG", PHASE_LOG)
        .args(["--precompile", "--wasi-command-manifest"])
        .arg(&manifest)
        .env("MOLT_WASM_PRECOMPILED_PATH", &native)
        .stderr(Stdio::inherit())
        .output()
        .expect("execute production host precompile operation");
    assert!(
        precompiled.status.success(),
        "WASI precompilation failed: status={} stdout={} output={}",
        precompiled.status,
        String::from_utf8_lossy(&precompiled.stdout),
        native.display()
    );
    let receipt: serde_json::Value =
        serde_json::from_slice(&precompiled.stdout).expect("decode actual producer receipt");
    let member = &receipt["artifacts"]["main"];
    assert_eq!(receipt["kind"], "molt-wasm-precompile");
    assert_eq!(member["source_sha256"], actual);
    assert_eq!(member["source"].as_str().unwrap(), source.to_str().unwrap());
    assert_eq!(member["path"].as_str().unwrap(), native.to_str().unwrap());
    let native_bytes = std::fs::read(&native).expect("read actual native container");
    assert_eq!(member["sha256"], sha256_hex(&native_bytes));
    assert_eq!(member["size"], native_bytes.len());
    drop(native_bytes);
    println!("wasi-lifecycle precompile_receipt={receipt}");
    let precompile_elapsed = started.elapsed();
    // Rust libtest accepts multiple exact filters, so all cases share one
    // loader and one runtime instance. Stderr stays live for failure evidence.
    let output = host()
        .env("MOLT_WASM_HOST_LOG", PHASE_LOG)
        .env("MOLT_WASM_PRECOMPILED", "1")
        .env("MOLT_WASM_PRECOMPILED_PATH", &native)
        .arg("--wasi-command-manifest")
        .arg(&manifest)
        .args(["--", "--exact"])
        .args(cases)
        .args(["--test-threads=1", "--color", "never", "--nocapture"])
        .stderr(Stdio::inherit())
        .output()
        .expect("execute the production host against the actual WASI libtest");
    let elapsed = started.elapsed();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "WASI lifecycle cases failed: status={} (host stderr streamed above)\nstdout:\n{stdout}",
        output.status
    );
    for name in cases {
        let expected = format!("test {name} ... ok");
        assert_eq!(
            stdout.lines().filter(|line| *line == expected).count(),
            1,
            "WASI case {name} did not pass exactly once:\n{stdout}"
        );
    }
    let expected_summary = format!(
        "test result: ok. {} passed; 0 failed; 0 ignored; 0 measured; ",
        cases.len()
    );
    assert_eq!(
        stdout
            .lines()
            .filter(|line| line.starts_with(&expected_summary))
            .count(),
        1,
        "WASI lifecycle run did not execute exactly the selected cases:\n{stdout}"
    );
    for name in cases {
        println!("wasi-lifecycle passed case={name} artifact_sha256={actual}");
    }
    println!(
        "wasi-lifecycle shared-host cases={} precompile_s={:.3} total_s={:.3}",
        cases.len(),
        precompile_elapsed.as_secs_f64(),
        elapsed.as_secs_f64()
    );
}
