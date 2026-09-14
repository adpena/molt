//! Exercise the actual CLI logger: unit-test binaries do not run host main().

#[path = "support/mod.rs"]
mod support;

use support::host;

struct ArtifactFixture(std::path::PathBuf);

impl ArtifactFixture {
    fn new() -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "molt-precompile-cli-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn write(&self, name: &str, content: &[u8]) -> std::path::PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, content).unwrap();
        path
    }
}

impl Drop for ArtifactFixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).expect("remove owned CLI fixture");
    }
}

#[test]
fn actual_precompile_command_roundtrips_through_source_bound_loader() {
    let fixture = ArtifactFixture::new();
    let source = fixture.write("command.wat", b"(module (func (export \"_start\")))");
    let artifact = fixture.0.join("explicit.molt.cwasm");
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
    assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 2);
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
            "version":2,"mode":"split-runtime","modules":{
                "app":descriptor(&app, app_bytes),"runtime":descriptor(&runtime,runtime_bytes)
            }
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
        std::fs::read_dir(&fixture.0).unwrap().count(),
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
    assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 1);
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
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "molt-host-compiler-profile-{}-{nonce}.wat",
        std::process::id()
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .expect("create unique profiling fixture");
    std::io::Write::write_all(&mut file, b"(module (func (export \"_start\")))")
        .expect("write profiling fixture");
    drop(file);
    let result = host()
        .args(["--wasi-command"])
        .arg(&path)
        .env("RUST_LOG", "off")
        .env(
            "MOLT_WASM_HOST_LOG",
            "off,wasmtime_internal_cranelift::compiler=trace",
        )
        .output();
    std::fs::remove_file(&path).expect("remove owned profiling fixture");
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
