use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

struct TestTempDir {
    path: PathBuf,
}

impl TestTempDir {
    fn new() -> Self {
        static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);
        let temp_id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock must be after unix epoch")
            .as_nanos();
        // The platform temp directory, never the checkout: a run context
        // points it at the run scratch root.
        let path = std::env::temp_dir().join(format!(
            "native-batch-worker-spawn-{}-{temp_id}-{nonce}",
            std::process::id(),
        ));
        std::fs::create_dir_all(&path).expect("create native batch worker temp dir");
        Self { path }
    }
}

impl Drop for TestTempDir {
    fn drop(&mut self) {
        if self.path.exists()
            && let Err(err) = std::fs::remove_dir_all(&self.path)
        {
            eprintln!(
                "MOLT_TEST: failed to remove native batch worker temp dir '{}': {err}",
                self.path.display()
            );
        }
    }
}

mod cargo_test_artifacts {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test_support/cargo_test_artifacts.rs"
    ));
}

fn assert_linked_frame_slot(application: &Path, stdlib: &Path, expected: u64) {
    let artifacts = cargo_test_artifacts::CargoTestArtifacts::new("shared-object-slot-binding")
        .expect("retain linked observer under actual Cargo image custody");
    let app = artifacts.path().join("app.a");
    let shared = artifacts.path().join("stdlib.a");
    std::fs::copy(application, &app).unwrap();
    std::fs::copy(stdlib, &shared).unwrap();
    let source = artifacts.path().join("slot_observer.rs");
    let binding = molt_ir::code_slots::code_slot_symbol("molt_init_sys");
    std::fs::write(
        &source,
        format!(
            r#"
use std::sync::atomic::{{AtomicU64, Ordering}};
static OBSERVED: AtomicU64 = AtomicU64::new(u64::MAX);
static EXITS: AtomicU64 = AtomicU64::new(0);
#[unsafe(no_mangle)] pub extern "C" fn molt_trace_enter_slot(slot: u64) -> u64 {{
    OBSERVED.store(slot, Ordering::SeqCst); 0
}}
#[unsafe(no_mangle)] pub extern "C" fn molt_trace_exit() -> u64 {{
    EXITS.fetch_add(1, Ordering::SeqCst); 0
}}
#[unsafe(no_mangle)] pub extern "C" fn molt_recursion_guard_enter() -> i64 {{ 1 }}
#[unsafe(no_mangle)] pub extern "C" fn molt_recursion_guard_exit() {{}}
#[unsafe(no_mangle)] pub extern "C" fn molt_exception_pending_fast() -> u64 {{ 0 }}
#[used] #[unsafe(export_name = "{abi}")] pub static ABI: u8 = 0;
unsafe extern "C" {{
    fn molt_init_sys();
    #[link_name = "{binding}"] static BINDING: u64;
}}
fn main() {{
    unsafe {{ assert_eq!(BINDING, {expected}); molt_init_sys(); }}
    assert_eq!(OBSERVED.load(Ordering::SeqCst), {expected});
    assert_eq!(EXITS.load(Ordering::SeqCst), 1);
}}
"#,
            abi = molt_codegen_abi::GENERATED_OBJECT_ABI_SYMBOL
        ),
    )
    .unwrap();
    let executable = artifacts
        .path()
        .join(format!("slot_observer{}", std::env::consts::EXE_SUFFIX));
    let linked = artifacts
        .command(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
        .unwrap()
        .args(["--edition=2024"])
        .arg(artifacts.argument("", &source).unwrap())
        .arg("-C")
        .arg(artifacts.argument("link-arg=", &app).unwrap())
        .arg("-C")
        .arg(artifacts.argument("link-arg=", &shared).unwrap())
        .arg("-o")
        .arg(artifacts.argument("", &executable).unwrap())
        .output()
        .expect("link actual shared and application objects");
    assert!(
        linked.status.success(),
        "slot observer link failed: {}",
        String::from_utf8_lossy(&linked.stderr)
    );
    let observed = artifacts
        .command(&executable)
        .unwrap()
        .output()
        .expect("execute linked slot observer");
    assert!(
        observed.status.success(),
        "actual frame used wrong program binding: {}",
        String::from_utf8_lossy(&observed.stderr)
    );
}

#[test]
fn native_batch_worker_spawn_path_compiles_materialized_batches() {
    let tmp = TestTempDir::new();
    let ir_path = tmp.path.join("two_live_functions.json");
    let output_path = tmp.path.join("out.a");
    std::fs::write(
        &ir_path,
        r#"{
  "functions": [
    {"return_abi": "value",
      "name": "molt_main",
      "params": [],
      "ops": [
        {"kind": "call_internal", "s_value": "helper", "out": "x"},
        {"kind": "ret", "args": ["x"]}
      ],
      "param_types": null,
      "source_file": null,
      "is_extern": false
    },
    {"return_abi": "value",
      "name": "helper",
      "params": [],
      "ops": [
        {"kind": "const", "value": 2, "out": "y"},
        {"kind": "ret", "args": ["y"]}
      ],
      "param_types": null,
      "source_file": null,
      "is_extern": false
    }
  ],
  "profile": null
}
"#,
    )
    .expect("write native batch worker test IR");

    let output = Command::new(env!("CARGO_BIN_EXE_molt-backend"))
        .args(["--native-output-kind", "archive"])
        .arg("--ir-file")
        .arg(&ir_path)
        .arg("--output")
        .arg(&output_path)
        .env("MOLT_BACKEND_BATCH_SIZE", "1")
        .env("MOLT_BACKEND_BATCH_OP_BUDGET", "8000")
        .output()
        .expect("spawn production molt-backend binary");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "native batch worker binary path failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        stderr
    );
    assert!(
        stderr.contains("compiling materialized batch 1/2")
            && stderr.contains("compiling materialized batch 2/2")
            && stderr.contains("2 functions, 2 batches"),
        "expected production worker path to materialize and compile two batches; stderr:\n{stderr}"
    );
    assert!(
        output_path
            .metadata()
            .expect("native batch output object")
            .len()
            > 0,
        "native batch worker path must write a non-empty object"
    );

    // Explicit invalid custody must fail before publication instead of silently
    // allocating in the process's unrelated platform temp directory.
    let nondirectory = tmp.path.join("scratch-file");
    std::fs::write(&nondirectory, b"not a directory").expect("write invalid scratch root");
    for (index, invalid_root) in [
        PathBuf::from("relative-scratch"),
        tmp.path.join("missing-scratch"),
        nondirectory,
    ]
    .iter()
    .enumerate()
    {
        let denied_output = tmp.path.join(format!("denied-{index}.a"));
        let denied = Command::new(env!("CARGO_BIN_EXE_molt-backend"))
            .args(["--native-output-kind", "archive"])
            .arg("--ir-file")
            .arg(&ir_path)
            .arg("--output")
            .arg(&denied_output)
            .env("MOLT_BACKEND_BATCH_SIZE", "1")
            .env("MOLT_BACKEND_BATCH_OP_BUDGET", "8000")
            .env("MOLT_GUARD_SCRATCH_ROOT", invalid_root)
            .output()
            .expect("spawn backend with invalid scratch selector");
        assert!(
            !denied.status.success(),
            "invalid scratch silently accepted: {invalid_root:?}"
        );
        assert!(
            !denied_output.exists(),
            "denied compilation published an archive"
        );
    }
}

#[test]
fn native_batch_worker_spawn_path_batches_shared_stdlib_cache_object() {
    let tmp = TestTempDir::new();
    let ir_path = tmp.path.join("stdlib_split.json");
    let output_path = tmp.path.join("out.a");
    let initial_key = "a".repeat(64);
    let other_key = "b".repeat(64);
    let emitter_key = "c".repeat(64);
    let changed_key = "d".repeat(64);
    let cache_root = tmp.path.join("cache");
    let archive_for = |key: &str| cache_root.join("shared-stdlib").join(key).join("stdlib.a");
    let required_archive_for = |output: &Path| output.with_extension("stdlib.a");
    let stdlib_path = required_archive_for(&output_path);
    let runtime_symbols_path = tmp.path.join("runtime_callable_symbols.txt");
    std::fs::write(
        &runtime_symbols_path,
        "molt_main\nmolt_host_init\nmolt_init_demo\nmolt_init_sys\nsys__helper\n",
    )
    .expect("write runtime callable symbol set");
    std::fs::write(
        &ir_path,
        r#"{
  "code_slots": {"molt_init_sys": 0},
  "functions": [
    {"return_abi": "void",
      "name": "molt_main",
      "params": [],
      "ops": [
        {"kind": "call", "s_value": "molt_init_demo"},
        {"kind": "ret_void"}
      ],
      "param_types": null,
      "source_file": null,
      "is_extern": false
    },
    {"return_abi": "void",
      "name": "molt_host_init",
      "params": [],
      "ops": [
        {"kind": "call", "s_value": "molt_init_demo"},
        {"kind": "ret_void"}
      ],
      "param_types": null,
      "source_file": null,
      "is_extern": false
    },
    {"return_abi": "void",
      "name": "molt_init_demo",
      "params": [],
      "ops": [
        {"kind": "call", "s_value": "demo__molt_module_chunk_1"},
        {"kind": "call", "s_value": "molt_init_sys"},
        {"kind": "ret_void"}
      ],
      "param_types": null,
      "source_file": null,
      "is_extern": false
    },
    {"return_abi": "void",
      "name": "demo__molt_module_chunk_1",
      "params": [],
      "ops": [
        {"kind": "ret_void"}
      ],
      "param_types": null,
      "source_file": null,
      "is_extern": false
    },
    {"return_abi": "void",
      "name": "molt_isolate_bootstrap",
      "params": [],
      "ops": [
        {"kind": "ret_void"}
      ],
      "param_types": null,
      "source_file": null,
      "is_extern": false
    },
    {"return_abi": "void",
      "name": "molt_isolate_import",
      "params": ["p0"],
      "ops": [
        {"kind": "ret_void"}
      ],
      "param_types": null,
      "source_file": null,
      "is_extern": false
    },
    {"return_abi": "void",
      "name": "molt_init_sys",
      "params": [],
      "execution_context": "local",
      "ops": [
        {"kind": "trace_enter_slot", "s_value": "molt_init_sys"},
        {"kind": "call_internal", "s_value": "sys__helper", "passes_execution_context": true},
        {"kind": "trace_exit"},
        {"kind": "ret_void"}
      ],
      "param_types": null,
      "source_file": null,
      "is_extern": false
    },
    {"return_abi": "void",
      "name": "sys__helper",
      "params": [],
      "execution_context": "inherited",
      "ops": [
        {"kind": "ret_void"}
      ],
      "param_types": null,
      "source_file": null,
      "is_extern": false
    }
  ],
  "profile": null
}
"#,
    )
    .expect("write stdlib split native batch worker test IR");

    let manifest_for = |cache_key: &str, opt_level: &str| {
        serde_json::json!({
        "schema": "stdlib-manifest-v2-archive", "artifact_kind": "archive", "cache_key": cache_key,
        "cache_variant": format!("profile=test;codegen_env=test-{opt_level}"),
        "compiler_fingerprint": "test", "target_triple": null,
    }).to_string()
    };
    let run_backend =
        |destination: &Path, cache_key: &str, opt_level: &str, inline_exc_disabled: &str| {
            Command::new(env!("CARGO_BIN_EXE_molt-backend"))
                .args(["--native-output-kind", "archive"])
                .arg("--ir-file")
                .arg(&ir_path)
                .arg("--output")
                .arg(destination)
                .env("MOLT_ENTRY_MODULE", "demo")
                .env("MOLT_STDLIB_OBJ", required_archive_for(destination))
                .env("MOLT_STDLIB_CACHE_ENTRY", archive_for(cache_key))
                .env("MOLT_STDLIB_CACHE_KEY", cache_key)
                .env("MOLT_BACKEND_OPT_LEVEL", opt_level)
                .env("MOLT_BACKEND_INLINE_EXC_DISABLED", inline_exc_disabled)
                .env(
                    "MOLT_STDLIB_CACHE_MANIFEST",
                    manifest_for(cache_key, opt_level),
                )
                .env("MOLT_STDLIB_MODULE_SYMBOLS", "[\"sys\"]")
                .env("MOLT_RUNTIME_CALLABLE_SYMBOLS", &runtime_symbols_path)
                .env(
                    "MOLT_RUNTIME_CALLABLE_SYMBOLS_SHA256",
                    "7001af054760bae1bd40827bc8d71908d728584511d2a419f376e490ba1c405d",
                )
                .env("MOLT_BACKEND_BATCH_SIZE", "1")
                .env("MOLT_BACKEND_BATCH_OP_BUDGET", "8000")
                .output()
                .expect("spawn production molt-backend binary")
        };
    let output = run_backend(&output_path, initial_key.as_str(), "speed", "0");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "native stdlib batch worker binary path failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        stderr
    );
    assert!(
        stderr.contains("materializing 2 stdlib functions")
            && stderr.contains("stdlib object reuse: 0 hits, 2 misses")
            && stderr.contains("stdlib batch 1/2")
            && stderr.contains("stdlib batch 2/2")
            && stderr.contains("compiling stdlib object miss 1/2")
            && stderr.contains("compiling stdlib object miss 2/2"),
        "expected production worker path to materialize and compile two stdlib cache batches; stderr:\n{stderr}"
    );
    assert!(
        output_path
            .metadata()
            .expect("native batch output object")
            .len()
            > 0,
        "native batch worker path must write a non-empty application object"
    );
    assert!(
        stdlib_path
            .metadata()
            .expect("shared stdlib cache object")
            .len()
            > 0,
        "native batch worker path must publish a non-empty shared stdlib object"
    );
    assert_eq!(
        std::fs::read_to_string(stdlib_path.with_extension("count"))
            .expect("shared stdlib count sidecar")
            .trim(),
        "2"
    );
    assert_eq!(
        std::fs::read_to_string(stdlib_path.with_extension("key"))
            .expect("shared stdlib key sidecar")
            .trim(),
        initial_key.as_str()
    );
    assert_eq!(
        std::fs::read_to_string(stdlib_path.with_extension("manifest.json"))
            .expect("shared stdlib manifest sidecar")
            .trim(),
        manifest_for(initial_key.as_str(), "speed")
    );
    let partition_manifest = std::fs::read_to_string(stdlib_path.with_extension("partition.json"))
        .expect("shared stdlib partition manifest sidecar");
    assert!(
        partition_manifest.contains("\"function_count\":2")
            && partition_manifest.contains("\"molt_init_sys\"")
            && partition_manifest.contains("\"sys__helper\""),
        "shared stdlib partition manifest must describe both batched stdlib functions: {partition_manifest}"
    );
    let cached_digest = std::fs::read_to_string(stdlib_path.with_extension("sha256"))
        .expect("shared stdlib object digest sidecar");
    assert!(
        !cached_digest.trim().is_empty(),
        "shared stdlib object digest sidecar must be populated"
    );

    let required_bytes = std::fs::read(&stdlib_path).expect("required archive bytes");
    assert_eq!(
        std::fs::read(archive_for(&initial_key)).expect("optional cache archive"),
        required_bytes
    );

    let warm_output_path = tmp.path.join("out-warm.a");
    let warm_output = run_backend(&warm_output_path, initial_key.as_str(), "speed", "0");
    let warm_stderr = String::from_utf8_lossy(&warm_output.stderr);
    let warm_required = required_archive_for(&warm_output_path);
    assert_eq!(
        std::fs::read(&warm_required).expect("warm owned archive"),
        required_bytes
    );
    for extension in ["count", "key", "manifest.json", "partition.json", "sha256"] {
        assert_eq!(
            std::fs::read(warm_required.with_extension(extension)).expect("warm metadata"),
            std::fs::read(stdlib_path.with_extension(extension)).expect("cold metadata")
        );
    }
    // Eviction after successful cache handoff cannot invalidate either build.
    std::fs::remove_dir_all(archive_for(&initial_key).parent().expect("cache entry"))
        .expect("evict optional archive generation");
    assert_eq!(
        std::fs::read(&stdlib_path).expect("cold output survives eviction"),
        required_bytes
    );
    assert_eq!(
        std::fs::read(&warm_required).expect("warm output survives eviction"),
        required_bytes
    );
    assert!(
        warm_output.status.success(),
        "warm shared-stdlib reuse failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
        warm_output.status,
        String::from_utf8_lossy(&warm_output.stdout),
        warm_stderr
    );
    assert!(
        warm_stderr.contains("incremental -- compiling")
            && warm_stderr.contains("2 stdlib extern from")
            && warm_stderr.contains("compiling materialized batch 6/6")
            && warm_stderr.contains("6 functions, 6 batches")
            && !warm_stderr.contains("compiling 0 functions"),
        "warm forced-batch run must reuse the cache and schedule body batches only; stderr:\n{warm_stderr}"
    );
    assert!(
        warm_output_path
            .metadata()
            .expect("warm native batch output object")
            .len()
            > 0,
        "warm native batch worker path must write a non-empty application object"
    );

    // Force a whole-archive miss with a different program key. Constituent
    // admission must avoid BOTH production workers and reproduce the archive.
    let original_archive = std::fs::read(&stdlib_path).expect("read original stdlib archive");
    let constituent_output = run_backend(
        &tmp.path.join("other-program.a"),
        other_key.as_str(),
        "speed",
        "0",
    );
    let constituent_stderr = String::from_utf8_lossy(&constituent_output.stderr);
    assert!(
        constituent_output.status.success(),
        "constituent reuse failed: {constituent_stderr}"
    );
    assert!(
        constituent_stderr.contains("stdlib object reuse: 2 hits, 0 misses"),
        "archive miss must restore both exact objects: {constituent_stderr}"
    );
    assert!(
        !constituent_stderr.contains("compiling stdlib object miss"),
        "cache hits must not invoke stdlib workers: {constituent_stderr}"
    );
    assert_eq!(std::fs::read(&stdlib_path).unwrap(), original_archive);
    assert_eq!(
        std::fs::read(archive_for(&other_key)).unwrap(),
        original_archive
    );
    assert_ne!(archive_for(&initial_key), archive_for(&other_key));
    assert!(cache_root.join("native-stdlib-objects-v1").is_dir());
    assert!(
        !archive_for(&other_key)
            .parent()
            .unwrap()
            .join("native-stdlib-objects-v1")
            .exists()
    );

    // Adding an application body shifts the dense program assignment, but
    // unchanged stdlib objects must remain reusable and bind to the new ID.
    let original_input = std::fs::read(&ir_path).unwrap();
    let mut shifted: serde_json::Value = serde_json::from_slice(&original_input).unwrap();
    shifted["code_slots"] = serde_json::json!({"app_extra": 0, "molt_init_sys": 1});
    shifted["functions"][0]["ops"]
        .as_array_mut()
        .unwrap()
        .insert(
            0,
            serde_json::json!({"kind": "call", "s_value": "app_extra"}),
        );
    shifted["functions"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "name": "app_extra", "return_abi": "void", "params": [],
            "ops": [{"kind": "ret_void"}]
        }));
    std::fs::write(&ir_path, serde_json::to_vec(&shifted).unwrap()).unwrap();
    let shifted_output_path = tmp.path.join("shifted-program.a");
    let shifted_key = "e".repeat(64);
    let shifted_output = run_backend(&shifted_output_path, &shifted_key, "speed", "0");
    let shifted_stderr = String::from_utf8_lossy(&shifted_output.stderr);
    assert!(
        shifted_output.status.success(),
        "shifted program failed: {shifted_stderr}"
    );
    assert!(
        shifted_stderr.contains("stdlib object reuse: 2 hits, 0 misses"),
        "program bindings must not fragment shared object identity: {shifted_stderr}"
    );
    assert!(!shifted_stderr.contains("compiling stdlib object miss"));
    let shifted_stdlib_path = required_archive_for(&shifted_output_path);
    assert_eq!(
        std::fs::read(&shifted_stdlib_path).unwrap(),
        original_archive
    );
    assert_linked_frame_slot(&output_path, &stdlib_path, 0);
    assert_linked_frame_slot(&shifted_output_path, &shifted_stdlib_path, 1);
    std::fs::write(&ir_path, original_input).unwrap();

    // This switch was absent from the old CLI digest. Keep the same manifest
    // variant to prove that the actual inherited worker input invalidates reuse.
    let emitter_output = run_backend(
        &tmp.path.join("changed-emitter.a"),
        emitter_key.as_str(),
        "speed",
        "1",
    );
    let emitter_stderr = String::from_utf8_lossy(&emitter_output.stderr);
    assert!(
        emitter_output.status.success(),
        "changed emitter failed: {emitter_stderr}"
    );
    assert!(
        emitter_stderr.contains("stdlib object reuse: 0 hits, 2 misses"),
        "actual emitter switch must invalidate both constituent keys: {emitter_stderr}"
    );

    // Effective codegen settings and their canonical manifest change together.
    // Identical IR cannot admit the prior speed-optimized object generation.
    let changed_output = run_backend(
        &tmp.path.join("changed-codegen.a"),
        changed_key.as_str(),
        "none",
        "0",
    );
    let changed_stderr = String::from_utf8_lossy(&changed_output.stderr);
    assert!(
        changed_output.status.success(),
        "changed codegen failed: {changed_stderr}"
    );
    assert!(
        changed_stderr.contains("stdlib object reuse: 0 hits, 2 misses"),
        "codegen authority changes must recompile both objects: {changed_stderr}"
    );
}

#[test]
fn native_batch_worker_spawn_failure_preserves_replay_artifacts() {
    let tmp = TestTempDir::new();
    let ir_path = tmp.path.join("failing_batch.json");
    let output_path = tmp.path.join("out.a");
    let debug_artifact_dir = tmp.path.join("debug-artifacts");
    std::fs::write(
        &ir_path,
        r#"{
  "functions": [
    {"return_abi": "void",
      "name": "molt_main",
      "params": [],
      "ops": [
        {"kind": "call", "s_value": "helper"},
        {"kind": "ret_void"}
      ],
      "param_types": null,
      "source_file": null,
      "is_extern": false
    },
    {"return_abi": "void",
      "name": "helper",
      "params": [],
      "ops": [
        {"kind": "call_internal"},
        {"kind": "ret_void"}
      ],
      "param_types": null,
      "source_file": null,
      "is_extern": false
    }
  ],
  "profile": null
}
"#,
    )
    .expect("write failing native batch worker test IR");

    let output = Command::new(env!("CARGO_BIN_EXE_molt-backend"))
        .args(["--native-output-kind", "archive"])
        .arg("--ir-file")
        .arg(&ir_path)
        .arg("--output")
        .arg(&output_path)
        .env("MOLT_BACKEND_BATCH_SIZE", "1")
        .env("MOLT_BACKEND_BATCH_OP_BUDGET", "8000")
        .env("MOLT_DEBUG_ARTIFACT_DIR", &debug_artifact_dir)
        .output()
        .expect("spawn production molt-backend binary");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "native batch worker failure fixture must fail before artifact-preservation can be proven\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        stderr
    );
    let marker = "preserved replayable native application batch worker artifacts at '";
    let artifact_start = stderr
        .find(marker)
        .map(|idx| idx + marker.len())
        .unwrap_or_else(|| {
            panic!(
                "expected preserved replay artifact directory in worker failure stderr:\n{stderr}"
            )
        });
    let artifact_end = stderr[artifact_start..]
        .find('\'')
        .map(|idx| artifact_start + idx)
        .expect("artifact path terminator");
    let artifact_dir = PathBuf::from(&stderr[artifact_start..artifact_end]);
    assert!(
        artifact_dir.starts_with(&debug_artifact_dir),
        "worker failure artifacts must live under the configured debug artifact root: {}",
        artifact_dir.display()
    );
    let manifest_path = artifact_dir.join("manifest.json");
    assert!(manifest_path.exists(), "failure manifest must be preserved");
    let copied_job_path = artifact_dir.join("batch_1.json");
    assert!(
        copied_job_path.exists(),
        "failing batch job must be copied for replay"
    );
    let copied_job = std::fs::read_to_string(&copied_job_path).expect("read copied batch job");
    let copied_job_json: serde_json::Value =
        serde_json::from_str(&copied_job).expect("parse copied batch job");
    assert!(
        copied_job_json
            .get("module_context")
            .is_some_and(serde_json::Value::is_object),
        "replay must retain the exact projected context inline: {copied_job}"
    );
    assert!(copied_job_json.get("module_context_path").is_none());
    let manifest = std::fs::read_to_string(&manifest_path).expect("read failure manifest");
    let manifest_json: serde_json::Value =
        serde_json::from_str(&manifest).expect("parse failure manifest");
    let manifest_copied_job_path = manifest_json
        .get("copied_job_path")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from)
        .expect("failure manifest copied_job_path");
    let replay_argv = manifest_json
        .pointer("/replay/argv")
        .and_then(serde_json::Value::as_array)
        .expect("failure manifest replay argv");
    let replay_args: Vec<_> = replay_argv
        .iter()
        .map(|value| value.as_str().expect("replay argv entries are strings"))
        .collect();
    let replay_flag_index = replay_args
        .iter()
        .position(|arg| *arg == "--native-batch-job-file")
        .expect("replay command references native batch job file");
    let replay_job_path = replay_args
        .get(replay_flag_index + 1)
        .map(PathBuf::from)
        .expect("replay command has native batch job file argument");
    assert!(
        manifest_copied_job_path == copied_job_path
            && replay_job_path == copied_job_path
            && replay_args.iter().any(|arg| arg.ends_with("replay.o")),
        "failure manifest must describe a replayable worker command: {manifest}"
    );
}
