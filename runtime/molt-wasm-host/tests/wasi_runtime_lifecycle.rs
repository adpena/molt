//! Real compiler/runtime consumer proof, separate from synthetic host ABI tests.
//!
//! Build the WASI runtime libtest with `-C link-arg=--export-table`, then provide
//! its immutable artifact path and SHA-256. Cargo builds the real host binary
//! for this integration test; no second host or synthetic Molt manifest exists.
//! One libtest invocation owns all exact filters: compilation is shared, while
//! the runtime's test transactions own serial per-case state isolation.

use molt_wasm_host::sha256_hex;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Instant;

#[path = "support/mod.rs"]
mod support;

use support::host;

const CASES: [&str; 2] = [
    "concurrency::execution::tests::wasm_shutdown_custody_attaches_callbacks_without_an_execution_frame_or_c_state",
    "concurrency::execution::tests::wasm_logical_gil_and_execution_nesting_are_distinct",
];

// Keep the production phase breakdown without per-function compiler traces.
const PHASE_LOG: &str = "off,molt_wasm_host::precompiled=debug";

#[test]
#[ignore = "requires an actual WASI runtime libtest artifact and its SHA-256"]
fn actual_wasi_runtime_lifecycle_cases_through_production_host() {
    let artifact = PathBuf::from(
        std::env::var_os("MOLT_WASI_LIBTEST_ARTIFACT")
            .expect("MOLT_WASI_LIBTEST_ARTIFACT must name the compiled WASI libtest"),
    );
    let expected = std::env::var("MOLT_WASI_LIBTEST_SHA256")
        .expect("MOLT_WASI_LIBTEST_SHA256 must bind that exact compiler artifact");
    let bytes = std::fs::read(&artifact).expect("read compiler-produced WASI libtest");
    let actual = sha256_hex(&bytes);
    assert_eq!(actual, expected, "WASI artifact content changed");
    drop(bytes);

    // Compile once through the actual host producer, then execute its container
    // through the actual loader. This replaces the former JIT-only proof; it
    // does not add a second compilation of this large retained WASI module.
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let native = std::env::temp_dir().join(format!(
        "molt-wasi-lifecycle-{}-{nonce}.molt.cwasm",
        std::process::id()
    ));
    let started = Instant::now();
    let precompiled = host()
        .env("MOLT_WASM_HOST_LOG", PHASE_LOG)
        .args(["--precompile", "--wasi-command"])
        .arg(&artifact)
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
    assert_eq!(member["path"].as_str().unwrap(), native.to_str().unwrap());
    let native_bytes = std::fs::read(&native).expect("read actual native container");
    assert_eq!(member["sha256"], sha256_hex(&native_bytes));
    assert_eq!(member["size"], native_bytes.len());
    drop(native_bytes);
    println!("wasi-lifecycle precompile_receipt={receipt}");
    let precompile_elapsed = started.elapsed();
    // Rust libtest accepts multiple exact filters, so both cases share one
    // loader and one runtime instance. Stderr stays live for failure evidence.
    let output = host()
        .env("MOLT_WASM_HOST_LOG", PHASE_LOG)
        .env("MOLT_WASM_PRECOMPILED", "1")
        .env("MOLT_WASM_PRECOMPILED_PATH", &native)
        .arg("--wasi-command")
        .arg(&artifact)
        .args(["--", "--exact"])
        .args(CASES)
        .args(["--test-threads=1", "--color", "never"])
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
    for name in CASES {
        let expected = format!("test {name} ... ok");
        assert_eq!(
            stdout.lines().filter(|line| *line == expected).count(),
            1,
            "WASI case {name} did not pass exactly once:\n{stdout}"
        );
    }
    let expected_summary = format!(
        "test result: ok. {} passed; 0 failed; 0 ignored; 0 measured; ",
        CASES.len()
    );
    assert_eq!(
        stdout
            .lines()
            .filter(|line| line.starts_with(&expected_summary))
            .count(),
        1,
        "WASI lifecycle run did not execute exactly the selected cases:\n{stdout}"
    );
    for name in CASES {
        println!("wasi-lifecycle passed case={name} artifact_sha256={actual}");
    }
    std::fs::remove_file(&native).expect("remove owned passed lifecycle container");
    println!(
        "wasi-lifecycle shared-host cases={} precompile_s={:.3} total_s={:.3}",
        CASES.len(),
        precompile_elapsed.as_secs_f64(),
        elapsed.as_secs_f64()
    );
}
