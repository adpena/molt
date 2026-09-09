use super::*;

#[test]
fn daemon_native_path_written_output_skips_oversized_memory_cache() {
    let _env_guard = TestEnvGuard::clear(DAEMON_REQUEST_ENV_KEYS);
    unsafe {
        std::env::set_var("MOLT_BACKEND_BATCH_SIZE", "1");
        std::env::remove_var("MOLT_STDLIB_OBJ");
        std::env::remove_var("MOLT_STDLIB_CACHE_KEY");
        std::env::remove_var("MOLT_STDLIB_CACHE_MANIFEST");
        std::env::remove_var("MOLT_STDLIB_MODULE_SYMBOLS");
        std::env::remove_var("MOLT_RUNTIME_CALLABLE_SYMBOLS");
        std::env::remove_var("MOLT_ENTRY_MODULE");
    }

    let tmp_dir = std::env::temp_dir().join(format!(
        "molt-daemon-native-cache-budget-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
    let output = tmp_dir.join("out.a");
    let job = DaemonJobRequest {
        id: "job0".to_string(),
        is_wasm: false,
        target_triple: None,
        native_output_kind: NativeArtifactKind::Archive,
        wasm_link: false,
        wasm_data_base: None,
        wasm_table_base: None,
        wasm_split_runtime_app_table_base: None,
        output: output.to_string_lossy().to_string(),
        cache_key: "module-cache".to_string(),
        function_cache_key: Some("function-cache".to_string()),
        skip_module_output_if_synced: false,
        skip_function_output_if_synced: false,
        probe_cache_only: false,
        ir: Some(molt_backend::BackendIrDocument {
            module_registry: None,
            ir: SimpleIR {
                functions: vec![
                    FunctionIR {
                        name: "molt_main".to_string(),
                        params: vec![],
                        ops: vec![
                            OpIR {
                                kind: "call".to_string(),
                                s_value: Some("helper".to_string()),
                                value: Some(0),
                                ..OpIR::default()
                            },
                            OpIR {
                                kind: "ret_void".to_string(),
                                ..OpIR::default()
                            },
                        ],
                        param_types: None,
                        source_file: None,
                        is_extern: false,
                        codegen_partition: false,
                        execution_context: Default::default(),
                    },
                    FunctionIR {
                        name: "helper".to_string(),
                        params: vec![],
                        ops: vec![OpIR {
                            kind: "ret_void".to_string(),
                            ..OpIR::default()
                        }],
                        param_types: None,
                        source_file: None,
                        is_extern: false,
                        codegen_partition: false,
                        execution_context: Default::default(),
                    },
                ],
                profile: None,
            },
        }),
        ir_path: None,
    };
    let mut cache = DaemonCache::new(Some(1));

    let result = compile_single_job(job, &mut cache);

    assert!(result.ok, "daemon compile failed: {:?}", result.message);
    assert!(output.exists(), "path-written daemon output missing");
    assert!(
        cache.entries.is_empty(),
        "oversized object must not enter daemon memory cache"
    );
    assert!(
        result
            .warnings
            .iter()
            .any(|warning| warning.contains("skipped daemon memory cache")),
        "missing cache-budget warning: {:?}",
        result.warnings
    );

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[cfg(feature = "native-backend")]
#[test]
fn native_batch_temp_cleanup_reports_non_directory_path() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("molt-backend-cleanup-file-{nonce}"));
    std::fs::write(&path, b"not-a-directory").expect("write cleanup sentinel");

    let err = remove_native_batch_temp_dir(&path, "native batch cleanup test")
        .expect_err("file path must not be silently accepted as cleaned temp dir");

    assert!(
        err.to_string()
            .contains("failed to remove native batch cleanup test"),
        "unexpected cleanup error: {err}"
    );
    let _ = std::fs::remove_file(path);
}

#[cfg(feature = "native-backend")]
#[test]
fn native_batch_failure_artifact_rewrites_context_path_for_replay() {
    let _env_guard = TestEnvGuard::capture(&["MOLT_DEBUG_ARTIFACT_DIR"]);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "molt-batch-failure-artifact-test-{}-{nonce}",
        std::process::id()
    ));
    let source_dir = root.join("source");
    let debug_dir = root.join("debug");
    std::fs::create_dir_all(&source_dir).expect("create source batch dir");
    unsafe { std::env::set_var("MOLT_DEBUG_ARTIFACT_DIR", &debug_dir) };

    let module_context_path = source_dir.join("module_context.json");
    write_json_artifact(
        &module_context_path,
        &NativeBatchModuleMetadata {
            module_context: molt_backend::NativeBackendModuleContext::default(),
        },
    )
    .expect("write source module context");
    let job_path = source_dir.join("batch_7.json");
    write_json_artifact(
        &job_path,
        &NativeBatchObjectJob {
            ir: SimpleIR {
                functions: vec![],
                profile: None,
            },
            module_context_path: module_context_path.clone(),
            target_triple: None,
            emit_app_callable_resolver: false,
            app_callable_manifest: None,
            external_function_names: std::collections::BTreeSet::new(),
            module_registry: None,
        },
    )
    .expect("write source job");
    let object_path = source_dir.join("batch_7.o");

    let artifact_dir = preserve_native_batch_worker_failure_artifacts(
        "native application batch worker",
        &job_path,
        &object_path,
    )
    .expect("preserve failed worker artifacts");
    std::fs::remove_dir_all(&source_dir).expect("source batch temp dir cleanup");

    let copied_job_path = artifact_dir.join("batch_7.json");
    let copied_job: NativeBatchObjectJob =
        read_json_artifact(&copied_job_path, "copied native batch job")
            .expect("read copied native batch job");
    assert_eq!(
        copied_job.module_context_path,
        artifact_dir.join("module_context.json")
    );
    assert!(
        copied_job.module_context_path.exists(),
        "copied module context must survive source cleanup"
    );
    assert!(
        artifact_dir.join("manifest.json").exists(),
        "artifact manifest must describe replay command"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn daemon_batch_compile_keeps_user_module_chunk_stub_defined() {
    use object::{BinaryFormat, Object, ObjectSymbol, SymbolKind};

    let _env_guard = TestEnvGuard::clear(DAEMON_REQUEST_ENV_KEYS);
    let tmp_dir = std::env::temp_dir().join(format!(
        "molt-daemon-batch-chunk-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
    let output = tmp_dir.join("out.a");
    let stdlib = tmp_dir.join("stdlib.a");
    // The main application object emits the per-app callable resolver, which
    // requires the linked runtime staticlib's `molt_*` callable-symbol set
    // (`MOLT_RUNTIME_CALLABLE_SYMBOLS`). Production always extracts and
    // exposes this before native codegen; replicate that precondition through
    // the daemon's env-passthrough so this test exercises the real resolver
    // path instead of hitting the fail-closed guard. These IR functions take
    // no callable addresses (no candidate ops), so the resolved manifest is
    // empty regardless; the file just satisfies the required-symbol-set
    // contract with the symbols this object actually references.
    let runtime_symbols = tmp_dir.join("runtime_callable_symbols.txt");
    std::fs::write(
        &runtime_symbols,
        "molt_init_sys\nmolt_init_demo\nmolt_main\nmolt_host_init\n",
    )
    .expect("write runtime callable symbol set");
    let request = serde_json::json!({
        "version": BACKEND_DAEMON_PROTOCOL_VERSION,
        "config_digest": "daemon-test",
        "env": {
            "MOLT_ENTRY_MODULE": "demo",
            "MOLT_STDLIB_OBJ": stdlib.to_string_lossy(),
            "MOLT_STDLIB_CACHE_KEY": "daemon-stdlib-key",
            "MOLT_STDLIB_MODULE_SYMBOLS": "[\"sys\"]",
            "MOLT_RUNTIME_CALLABLE_SYMBOLS": runtime_symbols.to_string_lossy(),
            "MOLT_BACKEND_BATCH_SIZE": "1",
        },
        "jobs": [{
            "id": "job0",
            "is_wasm": false,
            "native_output_kind": "archive",
            "output": output.to_string_lossy(),
            "cache_key": "",
            "function_cache_key": "",
            "ir": {
                "functions": [
                    {"name": "molt_main", "params": [], "ops": [
                        {"kind": "call", "s_value": "molt_init_demo", "value": 0},
                        {"kind": "ret_void"}
                    ]},
                    {"name": "molt_host_init", "params": [], "ops": [
                        {"kind": "call", "s_value": "molt_init_demo", "value": 0},
                        {"kind": "ret_void"}
                    ]},
                    {"name": "molt_init_demo", "params": [], "ops": [
                        {"kind": "call", "s_value": "demo__molt_module_chunk_1", "value": 0},
                        {"kind": "ret_void"}
                    ]},
                    {"name": "demo__molt_module_chunk_1", "params": [], "ops": [
                        {"kind": "ret_void"}
                    ]},
                    {"name": "molt_isolate_bootstrap", "params": [], "ops": [{"kind": "ret_void"}]},
                    {"name": "molt_isolate_import", "params": ["p0"], "ops": [{"kind": "ret_void"}]},
                    {"name": "molt_init_sys", "params": [], "ops": [{"kind": "ret_void"}]}
                ],
                "profile": null
            }
        }]
    });

    let request = DaemonRequest::from_json_bytes(
        serde_json::to_string(&request)
            .expect("serialize request")
            .as_bytes(),
    )
    .expect("parse daemon request");
    let job = request.jobs.expect("jobs").into_iter().next().expect("job");
    let mut cache = DaemonCache::new(None);
    let result = compile_single_job(job, &mut cache);

    assert!(result.ok, "daemon compile failed: {:?}", result.message);
    assert!(output.exists(), "application archive missing");

    let bytes = std::fs::read(&output).expect("read daemon application archive");
    let archive = object::read::archive::ArchiveFile::parse(bytes.as_slice())
        .expect("parse daemon native archive");
    let mut definitions = 0;
    for member in archive.members() {
        let member = member.expect("read native archive member");
        let data = member
            .data(bytes.as_slice())
            .expect("read native member bytes");
        let object = object::File::parse(data).expect("parse native member object");
        let expected = if object.format() == BinaryFormat::MachO {
            "_demo__molt_module_chunk_1"
        } else {
            "demo__molt_module_chunk_1"
        };
        definitions += object
            .symbols()
            .filter(|symbol| {
                symbol.name().expect("symbol name") == expected
                    && symbol.kind() == SymbolKind::Text
                    && symbol.is_global()
                    && !symbol.is_weak()
                    && symbol.is_definition()
            })
            .count();
    }
    assert_eq!(
        definitions, 1,
        "ordinary archive members must retain exactly one strong chunk definition"
    );

    let _ = std::fs::remove_dir_all(&tmp_dir);
}
