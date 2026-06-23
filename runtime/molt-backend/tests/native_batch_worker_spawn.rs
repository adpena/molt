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
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("molt-backend manifest must live under runtime/molt-backend");
        let temp_id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock must be after unix epoch")
            .as_nanos();
        let path = repo_root.join("tmp").join(format!(
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

#[test]
fn native_batch_worker_spawn_path_compiles_materialized_batches() {
    let tmp = TestTempDir::new();
    let ir_path = tmp.path.join("two_live_functions.json");
    let output_path = tmp.path.join("out.o");
    std::fs::write(
        &ir_path,
        r#"{
  "functions": [
    {
      "name": "molt_main",
      "params": [],
      "ops": [
        {"kind": "call_internal", "s_value": "helper", "out": "x"},
        {"kind": "ret", "var": "x"}
      ],
      "param_types": null,
      "source_file": null,
      "is_extern": false
    },
    {
      "name": "helper",
      "params": [],
      "ops": [
        {"kind": "const", "value": 2, "out": "y"},
        {"kind": "ret", "var": "y"}
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

    std::fs::remove_dir_all(&tmp.path).expect("remove native batch worker temp dir");
}

#[test]
fn native_batch_worker_spawn_path_batches_shared_stdlib_cache_object() {
    let tmp = TestTempDir::new();
    let ir_path = tmp.path.join("stdlib_split.json");
    let output_path = tmp.path.join("out.o");
    let stdlib_path = tmp.path.join("stdlib_shared.o");
    let runtime_symbols_path = tmp.path.join("runtime_intrinsic_symbols.txt");
    std::fs::write(
        &runtime_symbols_path,
        "molt_main\nmolt_host_init\nmolt_init_demo\nmolt_init_sys\nsys__helper\n",
    )
    .expect("write runtime intrinsic symbol set");
    std::fs::write(
        &ir_path,
        r#"{
  "functions": [
    {
      "name": "molt_main",
      "params": [],
      "ops": [
        {"kind": "call", "s_value": "molt_init_demo", "value": 0},
        {"kind": "ret_void"}
      ],
      "param_types": null,
      "source_file": null,
      "is_extern": false
    },
    {
      "name": "molt_host_init",
      "params": [],
      "ops": [
        {"kind": "call", "s_value": "molt_init_demo", "value": 0},
        {"kind": "ret_void"}
      ],
      "param_types": null,
      "source_file": null,
      "is_extern": false
    },
    {
      "name": "molt_init_demo",
      "params": [],
      "ops": [
        {"kind": "call", "s_value": "demo__molt_module_chunk_1", "value": 0},
        {"kind": "call", "s_value": "molt_init_sys", "value": 0},
        {"kind": "ret_void"}
      ],
      "param_types": null,
      "source_file": null,
      "is_extern": false
    },
    {
      "name": "demo__molt_module_chunk_1",
      "params": [],
      "ops": [
        {"kind": "ret_void"}
      ],
      "param_types": null,
      "source_file": null,
      "is_extern": false
    },
    {
      "name": "molt_isolate_bootstrap",
      "params": [],
      "ops": [
        {"kind": "ret_void"}
      ],
      "param_types": null,
      "source_file": null,
      "is_extern": false
    },
    {
      "name": "molt_isolate_import",
      "params": ["p0"],
      "ops": [
        {"kind": "ret_void"}
      ],
      "param_types": null,
      "source_file": null,
      "is_extern": false
    },
    {
      "name": "molt_init_sys",
      "params": [],
      "ops": [
        {"kind": "call", "s_value": "sys__helper", "value": 0},
        {"kind": "ret_void"}
      ],
      "param_types": null,
      "source_file": null,
      "is_extern": false
    },
    {
      "name": "sys__helper",
      "params": [],
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

    let output = Command::new(env!("CARGO_BIN_EXE_molt-backend"))
        .arg("--ir-file")
        .arg(&ir_path)
        .arg("--output")
        .arg(&output_path)
        .env("MOLT_ENTRY_MODULE", "demo")
        .env("MOLT_STDLIB_OBJ", &stdlib_path)
        .env("MOLT_STDLIB_CACHE_KEY", "stdlib-batch-key")
        .env(
            "MOLT_STDLIB_CACHE_MANIFEST",
            "{\"cache_key\":\"stdlib-batch-key\"}",
        )
        .env("MOLT_STDLIB_MODULE_SYMBOLS", "[\"sys\"]")
        .env("MOLT_RUNTIME_INTRINSIC_SYMBOLS", &runtime_symbols_path)
        .env("MOLT_BACKEND_BATCH_SIZE", "1")
        .env("MOLT_BACKEND_BATCH_OP_BUDGET", "8000")
        .output()
        .expect("spawn production molt-backend binary");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "native stdlib batch worker binary path failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        stderr
    );
    assert!(
        stderr.contains("first build")
            && stderr.contains("caching 2 stdlib functions")
            && stderr.contains("stdlib batch 1/2")
            && stderr.contains("stdlib batch 2/2")
            && stderr.contains("compiling materialized stdlib batch 1/2")
            && stderr.contains("compiling materialized stdlib batch 2/2"),
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
        "stdlib-batch-key"
    );
    assert_eq!(
        std::fs::read_to_string(stdlib_path.with_extension("manifest.json"))
            .expect("shared stdlib manifest sidecar")
            .trim(),
        "{\"cache_key\":\"stdlib-batch-key\"}"
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

    std::fs::remove_dir_all(&tmp.path).expect("remove native batch worker temp dir");
}

#[test]
fn native_batch_worker_spawn_failure_preserves_replay_artifacts() {
    let tmp = TestTempDir::new();
    let ir_path = tmp.path.join("failing_batch.json");
    let output_path = tmp.path.join("out.o");
    let debug_artifact_dir = tmp.path.join("debug-artifacts");
    std::fs::write(
        &ir_path,
        r#"{
  "functions": [
    {
      "name": "molt_main",
      "params": [],
      "ops": [
        {"kind": "call", "s_value": "helper", "value": 0},
        {"kind": "ret_void"}
      ],
      "param_types": null,
      "source_file": null,
      "is_extern": false
    },
    {
      "name": "helper",
      "params": [],
      "ops": [
        {"kind": "call_internal", "value": 0},
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
    let module_context_path = artifact_dir.join("module_context.json");
    assert!(manifest_path.exists(), "failure manifest must be preserved");
    assert!(
        module_context_path.exists(),
        "batch module context must be copied beside the replay job"
    );
    let copied_job_path = artifact_dir.join("batch_1.json");
    assert!(
        copied_job_path.exists(),
        "failing batch job must be copied for replay"
    );
    let copied_job = std::fs::read_to_string(&copied_job_path).expect("read copied batch job");
    let copied_job_json: serde_json::Value =
        serde_json::from_str(&copied_job).expect("parse copied batch job");
    let replay_module_context_path = copied_job_json
        .get("module_context_path")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from)
        .expect("copied replay job module_context_path");
    assert!(
        replay_module_context_path == module_context_path,
        "copied replay job must reference the preserved module context: {copied_job}"
    );
    assert!(
        replay_module_context_path.exists(),
        "copied replay job module context path must exist"
    );
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

    std::fs::remove_dir_all(&tmp.path).expect("remove native batch worker temp dir");
}
