//! Exercise the actual CLI logger: unit-test binaries do not run host main().

#[path = "support/mod.rs"]
mod support;

use molt_wasm_facts::WASM_EXECUTION_MANIFEST_VERSION;
use support::host;
use support::launch::deny_launch;

// This retained-artifact owner also exposes external-compiler utilities.
#[allow(dead_code)]
mod cargo_test_artifacts {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test_support/cargo_test_artifacts.rs"
    ));
}

/// Artifacts retained under the actual Cargo test image, including failures.
struct ArtifactFixture(std::path::PathBuf);

impl ArtifactFixture {
    fn new() -> Self {
        let owner = cargo_test_artifacts::CargoTestArtifacts::new("wasm-host-cli").unwrap();
        let path = owner.path().join("inputs");
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
    fn write(&self, name: &str, content: &[u8]) -> std::path::PathBuf {
        let path = self.path().join(name);
        std::fs::write(&path, content).unwrap();
        path
    }
    fn command_manifest(
        &self,
        name: &str,
        content: &[u8],
    ) -> (std::path::PathBuf, std::path::PathBuf) {
        let source = self.write(name, content);
        let manifest = serde_json::json!({
            "version": WASM_EXECUTION_MANIFEST_VERSION,
            "mode": "linked",
            "modules": {"linked": {"path": name, "size": content.len(), "sha256": molt_wasm_host::sha256_hex(content)}},
            "entry": {"module": "linked", "function": "_start"},
            "launch": deny_launch(),
        });
        let manifest = self.write("manifest.json", &serde_json::to_vec(&manifest).unwrap());
        (source, manifest)
    }
}

#[test]
fn actual_precompile_command_roundtrips_through_source_bound_loader() {
    let fixture = ArtifactFixture::new();
    let source = fixture.write("command.wat", b"(module (func (export \"_start\")))");
    let artifact = fixture.path().join("explicit.molt.cwasm");
    let output = host()
        .args(["--precompile", "--wasi-command"])
        .arg(&source)
        .env("MOLT_WASM_PRECOMPILED_PATH", &artifact)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(receipt["version"], 1);
    assert_eq!(receipt["kind"], "molt-wasm-precompile");
    assert_eq!(receipt["artifacts"].as_object().unwrap().len(), 1);
    let main = &receipt["artifacts"]["main"];
    assert_eq!(main["path"].as_str().unwrap(), artifact.to_str().unwrap());
    let original = std::fs::read(&artifact).unwrap();
    assert_eq!(main["sha256"], molt_wasm_host::sha256_hex(&original));
    assert_eq!(main["size"], original.len());
    assert_eq!(std::fs::read_dir(fixture.path()).unwrap().count(), 2);
    let run = || {
        host()
            .arg("--wasi-command")
            .arg(&source)
            .env("MOLT_WASM_PRECOMPILED", "1")
            .env("MOLT_WASM_PRECOMPILED_PATH", &artifact)
            .output()
            .unwrap()
    };
    let result = run();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(result.stdout.is_empty());
    let mut corrupt = original.clone();
    *corrupt.last_mut().unwrap() ^= 1;
    std::fs::write(&artifact, &corrupt).unwrap();
    let result = run();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("payload SHA-256 mismatch"));
    std::fs::write(&artifact, &original).unwrap();
    std::fs::write(&source, b"(module (func (export \"_start\") nop))").unwrap();
    let result = run();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("source size mismatch"));
    assert_eq!(
        std::fs::read(&artifact).unwrap(),
        original,
        "execution must never rewrite artifacts"
    );
}

#[test]
fn actual_manifest_precompile_resolves_every_member_without_executing_guest() {
    let fixture = ArtifactFixture::new();
    let app_bytes = b"(module (func $start unreachable) (start $start))";
    let runtime_bytes = b"(module)";
    let app = fixture.write("app.wat", app_bytes);
    let runtime = fixture.write("runtime.wat", runtime_bytes);
    let descriptor = |path: &std::path::Path, bytes: &[u8]| {
        serde_json::json!({
            "path":path.file_name().unwrap().to_str().unwrap(),
            "size":bytes.len(),"sha256":molt_wasm_host::sha256_hex(bytes)
        })
    };
    let manifest = fixture.write(
        "manifest.json",
        &serde_json::to_vec(&serde_json::json!({
            "version":WASM_EXECUTION_MANIFEST_VERSION,"mode":"split-runtime","modules":{
                "app":descriptor(&app, app_bytes),"runtime":descriptor(&runtime,runtime_bytes)
            },
            "launch": deny_launch(),
        }))
        .unwrap(),
    );
    let output = host().arg("--precompile").arg(&manifest).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(receipt["artifacts"].as_object().unwrap().len(), 2);
    for role in ["main", "runtime"] {
        let member = &receipt["artifacts"][role];
        let bytes = std::fs::read(member["path"].as_str().unwrap()).unwrap();
        assert_eq!(member["sha256"], molt_wasm_host::sha256_hex(&bytes));
    }
    assert_eq!(
        std::fs::read_dir(fixture.path()).unwrap().count(),
        5,
        "no custody sidecars"
    );
    std::fs::write(&runtime, b"different").unwrap();
    let failed = host().arg("--precompile").arg(&manifest).output().unwrap();
    assert!(!failed.status.success());
    assert!(
        failed.stdout.is_empty(),
        "failed admission cannot emit a success receipt"
    );
    assert!(String::from_utf8_lossy(&failed.stderr).contains("runtime size mismatch"));
}

#[test]
fn actual_precompile_rejects_source_alias_and_retired_write_mode() {
    let fixture = ArtifactFixture::new();
    let content = b"(module (func (export \"_start\")))";
    let source = fixture.write("command.wat", content);
    let output = host()
        .args(["--precompile", "--wasi-command"])
        .arg(&source)
        .env("MOLT_WASM_PRECOMPILED_PATH", &source)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("collides"));
    assert_eq!(std::fs::read(&source).unwrap(), content);
    let output = host()
        .arg("--wasi-command")
        .arg(&source)
        .env("MOLT_WASM_PRECOMPILED_WRITE", "1")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("is retired"));
    assert_eq!(std::fs::read_dir(fixture.path()).unwrap().count(), 1);
}

#[test]
fn host_diagnostics_are_explicit_stderr_only_and_ignore_rust_log() {
    for (debug, filter, rust_log, expected) in [
        (None, None, "trace", false),
        (Some(""), None, "off", true),
        (Some("0"), None, "off", true),
        (None, Some("off,molt_wasm_host=debug"), "off", true),
        (Some("1"), Some("off"), "trace", false),
    ] {
        let mut command = host();
        command.arg("--help").env("RUST_LOG", rust_log);
        if let Some(value) = debug {
            command.env("MOLT_WASM_HOST_DEBUG", value);
        }
        if let Some(value) = filter {
            command.env("MOLT_WASM_HOST_LOG", value);
        }
        let output = command.output().expect("run production host help");
        assert!(output.status.success());
        assert!(
            output.stdout.is_empty(),
            "diagnostics must not contaminate stdout"
        );
        let stderr = String::from_utf8(output.stderr).expect("UTF-8 host diagnostics");
        assert_eq!(
            stderr.contains("starting"),
            expected,
            "debug={debug:?} filter={filter:?} RUST_LOG={rust_log}: {stderr}"
        );
    }
}

#[test]
fn upstream_compiler_timings_are_available_through_the_actual_host() {
    let mut file = tempfile::Builder::new()
        .prefix("molt-host-compiler-profile-")
        .suffix(".wat")
        .tempfile()
        .expect("create unique profiling fixture");
    std::io::Write::write_all(file.as_file_mut(), b"(module (func (export \"_start\")))")
        .expect("write profiling fixture");
    let path = file.path().to_path_buf();
    let result = host()
        .args(["--wasi-command"])
        .arg(&path)
        .env("RUST_LOG", "off")
        .env(
            "MOLT_WASM_HOST_LOG",
            "off,wasmtime_internal_cranelift::compiler=trace",
        )
        .output();
    drop(file);
    let output = result.expect("run production compiler profiling");
    let stderr = String::from_utf8(output.stderr).expect("UTF-8 compiler diagnostics");
    assert!(output.status.success(), "host failed: {stderr}");
    assert!(output.stdout.is_empty());
    assert!(
        stderr.contains("timing info"),
        "missing upstream timings: {stderr}"
    );
    assert!(
        stderr.contains("Compilation passes"),
        "missing upstream pass breakdown: {stderr}"
    );
}

#[test]
fn actual_manifest_command_sdk_and_indirect_dispatch_keep_command_exit_semantics() {
    let fixture = ArtifactFixture::new();
    let bytes = br#"(module
      (import "env" "molt_os_close_host" (func $close (param i64) (result i32)))
      (import "env" "molt_call_indirect0" (func $dispatch (param i64) (result i64)))
      (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
      (memory (export "memory") 1)
      (table (export "__indirect_function_table") 2 funcref)
      (func $target (result i64) i64.const 42) (elem (i32.const 1) $target)
      (func (export "_start")
        i64.const -1 call $close i32.const -9 i32.ne if unreachable end
        i64.const 1 call $dispatch i64.const 42 i64.ne if unreachable end
        i32.const 23 call $exit))"#;
    let (source, manifest) = fixture.command_manifest("command.wat", bytes);
    let artifact = fixture.path().join("command.molt.cwasm");
    let compiled = host()
        .args(["--precompile", "--wasi-command-manifest"])
        .arg(&manifest)
        .env("MOLT_WASM_PRECOMPILED_PATH", &artifact)
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let receipt: serde_json::Value = serde_json::from_slice(&compiled.stdout).unwrap();
    assert_eq!(
        receipt["artifacts"]["main"]["source_sha256"],
        molt_wasm_host::sha256_hex(bytes)
    );
    let run = host()
        .arg("--wasi-command-manifest")
        .arg(&manifest)
        .env("MOLT_WASM_PRECOMPILED", "1")
        .env("MOLT_WASM_PRECOMPILED_PATH", &artifact)
        .output()
        .unwrap();
    assert_eq!(
        run.status.code(),
        Some(23),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let raw = host()
        .arg("--wasi-command")
        .arg(&source)
        .env("MOLT_WASM_PRECOMPILED", "1")
        .env("MOLT_WASM_PRECOMPILED_PATH", &artifact)
        .output()
        .unwrap();
    assert!(!raw.status.success());
    let diagnostic = String::from_utf8_lossy(&raw.stderr);
    assert!(
        diagnostic.contains("Molt host imports require a current deployment manifest"),
        "{diagnostic}"
    );
    assert!(
        diagnostic.contains("env.molt_os_close_host"),
        "{diagnostic}"
    );
    assert!(!diagnostic.contains("missing precompiled"), "{diagnostic}");
}

#[test]
fn actual_manifest_command_rejects_unattested_versions_before_module_reads() {
    let current = WASM_EXECUTION_MANIFEST_VERSION;
    for version in [
        None,
        Some(serde_json::json!(current - 1)),
        Some(serde_json::json!(current + 1)),
        Some(serde_json::json!(true)),
        Some(serde_json::json!(current.to_string())),
        Some(serde_json::json!(f64::from(current))),
        Some(serde_json::Value::Null),
    ] {
        let fixture = ArtifactFixture::new();
        let (source, manifest) = fixture.command_manifest("command.wat", b"(module)");
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
        if let Some(version) = version {
            value["version"] = version;
        } else {
            value.as_object_mut().unwrap().remove("version");
        }
        std::fs::write(&manifest, serde_json::to_vec(&value).unwrap()).unwrap();
        std::fs::remove_file(&source).unwrap();
        for precompile in [false, true] {
            let mut child = host();
            if precompile {
                child.arg("--precompile");
            }
            let output = child
                .arg("--wasi-command-manifest")
                .arg(&manifest)
                .output()
                .unwrap();
            assert!(!output.status.success());
            assert!(output.stdout.is_empty());
            let diagnostic = String::from_utf8_lossy(&output.stderr);
            assert!(
                diagnostic.contains(&format!(
                    "Molt deployment manifest version must be {current}"
                )),
                "{diagnostic}"
            );
        }
        assert_eq!(std::fs::read_dir(fixture.path()).unwrap().count(), 1);
    }
}

#[test]
fn actual_manifest_command_requires_linked_start_and_exact_descriptor() {
    let fixture = ArtifactFixture::new();
    let (_, manifest) =
        fixture.command_manifest("command.wat", b"(module (func (export \"_start\")))");
    let original: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
    let mut cases = Vec::new();
    for entry in [
        serde_json::Value::Null,
        serde_json::json!({"module":"linked","function":"molt_main"}),
        serde_json::json!({"module":"app","function":"_start"}),
    ] {
        let mut value = original.clone();
        value["entry"] = entry;
        cases.push((value, "requires entry.module=linked"));
    }
    for role in ["runtime", "app", "unselected"] {
        let mut value = original.clone();
        value["modules"][role] = original["modules"]["linked"].clone();
        cases.push((value, "requires one linked module"));
    }
    let mut value = original.clone();
    value["mode"] = serde_json::json!("split-runtime");
    cases.push((value, "requires one linked module"));
    let mut value = original.clone();
    value["modules"]["linked"]["sha256"] = serde_json::json!("0".repeat(64));
    cases.push((value, "linked SHA-256 mismatch"));
    let mut value = original.clone();
    value["modules"]["linked"]["size"] = serde_json::json!(0);
    cases.push((value, "linked size mismatch"));
    let mut value = original.clone();
    value["modules"]["linked"]["path"] = serde_json::json!("../command.wat");
    cases.push((value, "must name an adjacent file"));
    for (value, expected) in cases {
        std::fs::write(&manifest, serde_json::to_vec(&value).unwrap()).unwrap();
        let output = host()
            .arg("--wasi-command-manifest")
            .arg(&manifest)
            .output()
            .unwrap();
        assert!(!output.status.success());
        let diagnostic = String::from_utf8_lossy(&output.stderr);
        assert!(diagnostic.contains(expected), "{expected}: {diagnostic}");
    }
}

#[test]
fn actual_manifest_command_precompile_preserves_collisions_and_never_runs_core_start() {
    let fixture = ArtifactFixture::new();
    let (source, manifest) = fixture.command_manifest(
        "command.wat",
        br#"(module (func $s unreachable) (start $s) (func (export "_start")))"#,
    );
    let original = std::fs::read(&manifest).unwrap();
    for destination in [&source, &manifest] {
        let output = host()
            .args(["--precompile", "--wasi-command-manifest"])
            .arg(&manifest)
            .env("MOLT_WASM_PRECOMPILED_PATH", destination)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("collides"));
        assert_eq!(std::fs::read(&manifest).unwrap(), original);
    }
    let artifact = fixture.path().join("command.molt.cwasm");
    let output = host()
        .args(["--precompile", "--wasi-command-manifest"])
        .arg(&manifest)
        .env("MOLT_WASM_PRECOMPILED_PATH", &artifact)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = host()
        .arg("--wasi-command-manifest")
        .arg(&manifest)
        .env("MOLT_WASM_PRECOMPILED", "1")
        .env("MOLT_WASM_PRECOMPILED_PATH", &artifact)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unreachable"));
}

#[test]
fn actual_manifest_command_checks_sdk_signature_before_core_start_and_keeps_guest_traps() {
    for (bytes, expected, forbidden) in [
        (br#"(module (import "env" "molt_os_close_host" (func)) (func $s unreachable) (start $s) (func (export "_start")))"#.as_slice(), "incompatible import type", Some("unreachable")),
        (br#"(module (func (export "_start") unreachable))"#.as_slice(), "unreachable", None),
        (br#"(module (func (export "_start") (param i32)))"#.as_slice(), "_start", None),
    ] {
        let fixture = ArtifactFixture::new();
        let (_, manifest) = fixture.command_manifest("command.wat", bytes);
        let output = host().arg("--wasi-command-manifest").arg(&manifest).output().unwrap();
        assert!(!output.status.success());
        let diagnostic = String::from_utf8_lossy(&output.stderr);
        assert!(diagnostic.contains(expected), "{diagnostic}");
        if let Some(forbidden) = forbidden { assert!(!diagnostic.contains(forbidden), "{diagnostic}"); }
    }
}

#[test]
fn actual_manifest_command_core_start_exit_precedes_exported_start() {
    let fixture = ArtifactFixture::new();
    let bytes = wat::parse_str(
        r#"(module
      (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
      (memory (export "memory") 1)
      (func $core i32.const 19 call $exit) (start $core)
      (func (export "_start") unreachable))"#,
    )
    .expect("valid binary WASI command fixture");
    let (_, manifest) = fixture.command_manifest("command.wasm", &bytes);
    let output = host()
        .arg("--wasi-command-manifest")
        .arg(&manifest)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(19),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let positional = host().arg(&manifest).output().unwrap();
    assert!(!positional.status.success());
    let diagnostic = String::from_utf8_lossy(&positional.stderr);
    assert!(diagnostic.contains("molt_main"), "{diagnostic}");
    assert!(!diagnostic.contains("unreachable"), "{diagnostic}");
}

#[test]
fn actual_application_reports_runtime_exit_status_and_finishes_lifetime() {
    for status in [0, 7] {
        let fixture = ArtifactFixture::new();
        // The guest asserts the finite protocol independently: entered, main,
        // reporter, leave, shutdown. A skipped/repeated/reordered call traps.
        let program = wat::parse_str(format!(
            r#"(module
            (import "wasi_snapshot_preview1" "fd_write"
                (func $write (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 32) "reported\0a")
            (data (i32.const 48) "shutdown\0a")
            (global $phase (mut i32) (i32.const 0))
            (global $pending (mut i64) (i64.const 0))
            (func $advance (param $before i32) (param $after i32)
                global.get $phase local.get $before i32.ne if unreachable end
                local.get $after global.set $phase)
            (func (export "molt_isolate_bootstrap") (result i64) unreachable)
            (func (export "molt_isolate_import") (param i64) (result i64) unreachable)
            (func (export "molt_exception_pending") (result i64) global.get $pending)
            (func (export "molt_runtime_execution_enter") (result i64)
                i32.const 0 i32.const 1 call $advance i64.const 41)
            (func (export "molt_main") (result i64)
                i32.const 1 i32.const 2 call $advance
                i64.const 1 global.set $pending i64.const 0)
            (func (export "molt_main_exit_status") (result i64)
                i32.const 2 i32.const 3 call $advance
                global.get $pending i64.const 1 i64.ne if unreachable end
                i64.const 0 global.set $pending
                i32.const 0 i32.const 32 i32.store
                i32.const 4 i32.const 9 i32.store
                i32.const 2 i32.const 0 i32.const 1 i32.const 16 call $write
                if unreachable end
                i64.const {status})
            (func (export "molt_runtime_execution_leave") (param i64)
                local.get 0 i64.const 41 i64.ne if unreachable end
                i32.const 3 i32.const 4 call $advance)
            (func (export "molt_runtime_shutdown") (result i64)
                i32.const 4 i32.const 5 call $advance
                i32.const 0 i32.const 48 i32.store
                i32.const 4 i32.const 9 i32.store
                i32.const 2 i32.const 0 i32.const 1 i32.const 16 call $write
                if unreachable end
                i64.const 1))"#
        ))
        .unwrap();
        fixture.write("application.wasm", &program);
        let manifest = fixture.write(
            "manifest.json",
            &serde_json::to_vec(&serde_json::json!({
                "version": WASM_EXECUTION_MANIFEST_VERSION,
                "mode": "linked",
                "modules": {"linked": {"path": "application.wasm", "size": program.len(),
                    "sha256": molt_wasm_host::sha256_hex(&program)}},
                "entry": {"module": "linked", "function": "molt_main"},
                "launch": deny_launch(),
            }))
            .unwrap(),
        );
        let output = host()
            .arg(&manifest)
            .current_dir(fixture.path())
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(status),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.is_empty());
        assert_eq!(output.stderr, b"reported\nshutdown\n");
    }
}
