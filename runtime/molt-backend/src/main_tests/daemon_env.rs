use super::*;
use molt_ir::backend_environment::{self as backend_env, BackendEnv};
use std::sync::Arc;

#[test]
#[cfg(feature = "native-backend")]
fn daemon_request_with_env_preserves_user_entry_archive() {
    let _env_guard = TestEnvGuard::clear(&DAEMON_REQUEST_ENV_KEYS);
    let tmp_dir = std::env::temp_dir().join(format!(
        "molt-daemon-request-env-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
    let output = tmp_dir.join("out.a");
    let stdlib = tmp_dir.join("stdlib.a");
    let stdlib_key = "a".repeat(64);
    let optional = tmp_dir
        .join("cache/shared-stdlib")
        .join(&stdlib_key)
        .join("stdlib.a");
    let manifest = serde_json::json!({
        "schema": "stdlib-manifest-v2-archive", "artifact_kind": "archive",
        "cache_key": stdlib_key, "cache_variant": "test",
        "compiler_fingerprint": "test", "target_triple": null,
    })
    .to_string();
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
    std::fs::write(&runtime_symbols, "molt_init_sys\nmolt_main\n")
        .expect("write runtime callable symbol set");
    let mut request_json = serde_json::json!({
        "version": BACKEND_DAEMON_PROTOCOL_VERSION,
        "config_digest": "daemon-test",
        "env": {
            "MOLT_ENTRY_MODULE": "demo",
            "MOLT_STDLIB_OBJ": stdlib.to_string_lossy(),
            "MOLT_STDLIB_CACHE_KEY": stdlib_key,
            "MOLT_STDLIB_CACHE_ENTRY": optional.to_string_lossy(),
            "MOLT_STDLIB_CACHE_MANIFEST": manifest,
            "MOLT_STDLIB_MODULE_SYMBOLS": "[\"sys\"]",
            "MOLT_RUNTIME_CALLABLE_SYMBOLS": runtime_symbols.to_string_lossy(),
            "MOLT_RUNTIME_CALLABLE_SYMBOLS_SHA256": "cd85e5b7627ffa5a6821fde44e31407d23a4d3da80a6e07a5b675af8d0925784",
        },
        "jobs": [{
            "id": "job0",
            "is_wasm": false,
            "native_output_kind": "archive",
            "output": output.to_string_lossy(),
            "cache_key": "module",
            "function_cache_key": "function",
            "ir_path": write_daemon_ir_fixture(&tmp_dir.join("input.msgpack"), &serde_json::json!({
                "functions": [
                    {"return_abi": "void", "name": "molt_main", "params": [], "ops": [{"kind": "call", "s_value": "demo__module"}]},
                    {"return_abi": "void", "name": "demo__module", "params": [], "ops": [{"kind": "call_internal", "s_value": "molt_init_sys"}, {"kind": "ret_void"}]},
                    {"return_abi": "void", "name": "molt_isolate_bootstrap", "params": [], "ops": [{"kind": "ret_void"}]},
                    {"return_abi": "void", "name": "molt_isolate_import", "params": ["p0"], "ops": [{"kind": "ret_void"}]},
                    {"return_abi": "void", "name": "molt_init_sys", "params": [], "ops": [{"kind": "ret_void"}]}
                ],
                "profile": null
            }))
        }]
    });

    let request = DaemonRequest::from_json_bytes(
        serde_json::to_string(&request_json)
            .expect("serialize request")
            .as_bytes(),
    )
    .expect("parse daemon request");
    let environment = Arc::clone(&request.environment);
    assert_eq!(environment.get(BackendEnv::MOLT_ENTRY_MODULE), Some("demo"));
    assert_eq!(
        environment.get(BackendEnv::MOLT_STDLIB_OBJ),
        Some(stdlib.to_string_lossy().as_ref())
    );
    assert_eq!(
        environment.get(BackendEnv::MOLT_STDLIB_MODULE_SYMBOLS),
        Some("[\"sys\"]")
    );
    // Parsing installs nothing: the process environment stays untouched.
    assert!(std::env::var_os("MOLT_ENTRY_MODULE").is_none());
    let job = request.jobs.expect("jobs").into_iter().next().expect("job");
    let mut partition_ir = SimpleIR {
        functions: vec![
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "call".to_string(),
                    s_value: Some("demo__module".to_string()),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                parameter_custody: Vec::new(),
                execution_context: Default::default(),
            },
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "demo__module".to_string(),
                params: vec![],
                ops: vec![
                    OpIR {
                        kind: "call_internal".to_string(),
                        s_value: Some("molt_init_sys".to_string()),
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
                parameter_custody: Vec::new(),
                execution_context: Default::default(),
            },
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "molt_isolate_bootstrap".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                parameter_custody: Vec::new(),
                execution_context: Default::default(),
            },
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "molt_isolate_import".to_string(),
                params: vec!["p0".to_string()],
                ops: vec![OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                parameter_custody: Vec::new(),
                execution_context: Default::default(),
            },
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "molt_init_sys".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                parameter_custody: Vec::new(),
                execution_context: Default::default(),
            },
        ],
        profile: None,
    };
    let stdlib_modules = std::collections::BTreeSet::from(["sys".to_string()]);
    let (user_remaining, stdlib_funcs, _module_context) = prune_and_partition_native_stdlib(
        &mut partition_ir,
        "demo",
        Some(&stdlib_modules),
        &std::collections::BTreeSet::new(),
    );
    let user_names: Vec<_> = user_remaining
        .iter()
        .map(|func| func.name.as_str())
        .collect();
    let stdlib_names: Vec<_> = stdlib_funcs.iter().map(|func| func.name.as_str()).collect();
    assert_eq!(
        user_names,
        vec![
            "molt_main",
            "demo__module",
            "molt_isolate_bootstrap",
            "molt_isolate_import"
        ]
    );
    assert_eq!(stdlib_names, vec!["molt_init_sys"]);
    let cache = DaemonCache::new(None);
    let result = {
        let _scope = backend_env::enter(environment);
        compile_single_job(job, &cache)
    };

    assert!(result.ok, "daemon compile failed: {:?}", result.message);
    let bytes = std::fs::read(&output).expect("read application archive");
    assert_native_archive_members(&bytes, 1);
    let stdlib_bytes = std::fs::read(&stdlib).expect("read shared stdlib archive");
    assert_native_archive_members(&stdlib_bytes, 1);
    assert!(!result.cached);
    assert_eq!(std::fs::read(&optional).unwrap(), stdlib_bytes);
    let extensions = ["key", "manifest.json", "partition.json", "sha256"];
    let sidecars: Vec<_> = extensions
        .iter()
        .map(|ext| {
            let bytes = std::fs::read(stdlib.with_extension(ext)).expect("read required sidecar");
            assert_eq!(std::fs::read(optional.with_extension(ext)).unwrap(), bytes);
            bytes
        })
        .collect();

    // A new owned destination forces real warm archive hydration in the same daemon.
    let warm_stdlib = tmp_dir.join("warm-stdlib.a");
    let warm_output = tmp_dir.join("warm-app.a");
    request_json["env"]["MOLT_STDLIB_OBJ"] = warm_stdlib.to_string_lossy().as_ref().into();
    request_json["jobs"][0]["output"] = warm_output.to_string_lossy().as_ref().into();
    let run_request = |value: &serde_json::Value, cache: &DaemonCache| {
        let parsed = DaemonRequest::from_json_bytes(&serde_json::to_vec(value).unwrap())
            .expect("parse repeated daemon request");
        let _scope = backend_env::enter(parsed.environment);
        compile_single_job(parsed.jobs.unwrap().into_iter().next().unwrap(), cache)
    };
    let warm = run_request(&request_json, &cache);
    assert!(warm.ok, "warm compile failed: {:?}", warm.message);
    assert!(!warm.cached);
    assert_eq!(std::fs::read(&warm_stdlib).unwrap(), stdlib_bytes);
    for (ext, expected) in extensions.iter().zip(&sidecars) {
        assert_eq!(
            &std::fs::read(warm_stdlib.with_extension(ext)).unwrap(),
            expected
        );
    }
    // Native daemon cache publication requires an already-admitted required
    // archive at request entry. Compile once with that established owner.
    let admitted_output = tmp_dir.join("admitted-app.a");
    request_json["jobs"][0]["output"] = admitted_output.to_string_lossy().as_ref().into();
    let admitted = run_request(&request_json, &cache);
    assert!(
        admitted.ok && !admitted.cached,
        "admitted compile: {:?}",
        admitted.message
    );
    let warm_app_bytes = std::fs::read(&admitted_output).unwrap();
    assert_native_archive_members(&warm_app_bytes, 1);

    // Returned compile releases cache custody. Evict only the optional generation;
    // probe-only reuse must still produce the real application from daemon memory.
    std::fs::remove_dir_all(optional.parent().unwrap()).expect("evict optional archive");
    request_json["jobs"][0]
        .as_object_mut()
        .unwrap()
        .remove("ir_path");
    request_json["jobs"][0]["probe_cache_only"] = true.into();
    for (name, omit_optional) in [("evicted", false), ("omitted", true)] {
        if omit_optional {
            request_json["env"]
                .as_object_mut()
                .unwrap()
                .remove("MOLT_STDLIB_CACHE_ENTRY");
        }
        let destination = tmp_dir.join(format!("{name}.a"));
        request_json["jobs"][0]["output"] = destination.to_string_lossy().as_ref().into();
        let hit = run_request(&request_json, &cache);
        assert!(
            hit.ok && hit.cached && !hit.needs_ir,
            "{name}: {:?}",
            hit.message
        );
        assert_eq!(std::fs::read(&destination).unwrap(), warm_app_bytes);
        assert_eq!(std::fs::read(&warm_stdlib).unwrap(), stdlib_bytes);
        for (ext, expected) in extensions.iter().zip(&sidecars) {
            assert_eq!(
                &std::fs::read(warm_stdlib.with_extension(ext)).unwrap(),
                expected
            );
        }
        assert!(!optional.exists());
    }
    let _ = std::fs::remove_dir_all(&tmp_dir);
}

/// Parses `request` the way the daemon does.
fn parse_request(request: serde_json::Value) -> Result<DaemonRequest, String> {
    DaemonRequest::from_json_bytes(&serde_json::to_vec(&request).expect("serialize request"))
}

#[test]
fn daemon_request_scope_hides_stale_process_settings() {
    let _env_guard = TestEnvGuard::clear(&DAEMON_REQUEST_ENV_KEYS);
    unsafe {
        std::env::set_var("MOLT_STDLIB_MODULE_SYMBOLS", "[\"stale\"]");
        std::env::set_var("MOLT_ENTRY_MODULE", "stale_entry");
    }

    let parsed = parse_request(serde_json::json!({
        "version": BACKEND_DAEMON_PROTOCOL_VERSION,
        "config_digest": "daemon-clear-test",
        "env": {"MOLT_ENTRY_MODULE": "demo"},
        "jobs": [],
    }))
    .expect("parse daemon request");

    assert_eq!(parsed.version, Some(BACKEND_DAEMON_PROTOCOL_VERSION));
    {
        let _scope = backend_env::enter(parsed.environment);
        assert_eq!(
            backend_env::var(BackendEnv::MOLT_ENTRY_MODULE).as_deref(),
            Ok("demo")
        );
        assert!(backend_env::var(BackendEnv::MOLT_STDLIB_MODULE_SYMBOLS).is_err());
    }
    // The request never wrote the shared process environment.
    assert_eq!(
        std::env::var("MOLT_ENTRY_MODULE").as_deref(),
        Ok("stale_entry")
    );
}

#[test]
fn daemon_requests_do_not_leak_settings_into_each_other() {
    let _env_guard = TestEnvGuard::clear(&DAEMON_REQUEST_ENV_KEYS);
    let keys = [
        BackendEnv::MOLT_BACKEND_MEMORY_AVAILABLE_GB,
        BackendEnv::MOLT_CLI_MEMORY_AVAILABLE_GB,
        BackendEnv::MOLT_MEMORY_AVAILABLE_GB,
        BackendEnv::MOLT_BACKEND_MAX_PROCESS_RSS_GB,
        BackendEnv::MOLT_BACKEND_MEMORY_RESERVE_GB,
        BackendEnv::MOLT_CLI_MEMORY_RESERVE_GB,
        BackendEnv::MOLT_MEMORY_RESERVE_GB,
        BackendEnv::RAYON_NUM_THREADS,
        BackendEnv::MOLT_TIR_TRACE_FUNC,
    ];
    unsafe {
        for key in keys {
            std::env::set_var(key.name(), "stale");
        }
    }

    let first = parse_request(serde_json::json!({
        "version": BACKEND_DAEMON_PROTOCOL_VERSION,
        "config_digest": "daemon-resource-env-set",
        "env": {
            "MOLT_BACKEND_MEMORY_AVAILABLE_GB": "9",
            "MOLT_BACKEND_MEMORY_RESERVE_GB": "1",
            "RAYON_NUM_THREADS": "3",
            "MOLT_TIR_TRACE_FUNC": "target_func",
        },
        "jobs": [],
    }))
    .expect("parse first daemon request");
    let second = parse_request(serde_json::json!({
        "version": BACKEND_DAEMON_PROTOCOL_VERSION,
        "config_digest": "daemon-resource-env-clear",
        "env": {},
        "jobs": [],
    }))
    .expect("parse second daemon request");

    // Both requests hold their scopes at once, as concurrent builds do.
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let observe = |environment: Arc<backend_env::BackendEnvironment>| {
        let barrier = Arc::clone(&barrier);
        std::thread::spawn(move || {
            let _scope = backend_env::enter(environment);
            barrier.wait();
            keys.map(|key| backend_env::var(key).ok())
        })
    };
    let (first, second) = (observe(first.environment), observe(second.environment));
    let first = first.join().expect("first request");
    let second = second.join().expect("second request");
    assert_eq!(first[0].as_deref(), Some("9"));
    assert_eq!(first[4].as_deref(), Some("1"));
    assert_eq!(first[7].as_deref(), Some("3"));
    assert_eq!(first[8].as_deref(), Some("target_func"));
    assert_eq!(
        first[1], None,
        "absent settings read as unset, not the stale process value"
    );
    assert!(
        second.iter().all(Option::is_none),
        "{second:?} leaked into a request without them"
    );
}

#[test]
fn daemon_request_env_rejects_malformed_or_unknown_settings() {
    for (env, expected) in [
        (
            serde_json::json!({"MOLT_STDLIB_MODULE_SYMBOLS": "not-json"}),
            "MOLT_STDLIB_MODULE_SYMBOLS must be a JSON array of strings",
        ),
        (
            serde_json::json!({"MOLT_NOT_A_BACKEND_SETTING": "1"}),
            "unknown backend environment variable MOLT_NOT_A_BACKEND_SETTING",
        ),
        (
            serde_json::json!({"MOLT_ENTRY_MODULE": 1}),
            "MOLT_ENTRY_MODULE must be a string",
        ),
        (
            serde_json::json!(["MOLT_ENTRY_MODULE"]),
            "request.env must be an object",
        ),
    ] {
        let err = parse_request(serde_json::json!({
            "version": BACKEND_DAEMON_PROTOCOL_VERSION,
            "config_digest": "daemon-bad-env-test",
            "env": env,
            "jobs": [],
        }))
        .expect_err("malformed request settings must fail closed");
        assert!(err.contains(expected), "unexpected error message: {err}");
    }
}

#[test]
#[cfg(feature = "native-backend")]
fn concurrent_builds_of_one_shared_stdlib_entry_compile_it_once() {
    let _env_guard = TestEnvGuard::clear(&DAEMON_REQUEST_ENV_KEYS);
    const BUILDS: usize = 4;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before unix epoch")
        .as_nanos();
    let tmp_dir = std::env::temp_dir().join(format!(
        "molt-stdlib-producer-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
    // A key no other test uses, so the materialization count is this test's.
    let stdlib_key = format!("{nanos:064x}");
    let cache_entry = tmp_dir
        .join("cache/shared-stdlib")
        .join(&stdlib_key)
        .join("stdlib.a");
    let manifest = serde_json::json!({
        "schema": "stdlib-manifest-v2-archive", "artifact_kind": "archive",
        "cache_key": stdlib_key, "cache_variant": "test",
        "compiler_fingerprint": "test", "target_triple": null,
    })
    .to_string();
    let runtime_symbols = tmp_dir.join("runtime_callable_symbols.txt");
    std::fs::write(&runtime_symbols, "molt_init_sys\nmolt_main\n")
        .expect("write runtime callable symbol set");
    let request = |index: usize| {
        serde_json::json!({
            "version": BACKEND_DAEMON_PROTOCOL_VERSION,
            "config_digest": "stdlib-producer",
            "env": {
                "MOLT_ENTRY_MODULE": "demo",
                "MOLT_STDLIB_OBJ": tmp_dir.join(format!("stdlib-{index}.a")).to_string_lossy(),
                "MOLT_STDLIB_CACHE_KEY": stdlib_key,
                "MOLT_STDLIB_CACHE_ENTRY": cache_entry.to_string_lossy(),
                "MOLT_STDLIB_CACHE_MANIFEST": manifest,
                "MOLT_STDLIB_MODULE_SYMBOLS": "[\"sys\"]",
                "MOLT_RUNTIME_CALLABLE_SYMBOLS": runtime_symbols.to_string_lossy(),
                "MOLT_RUNTIME_CALLABLE_SYMBOLS_SHA256": "cd85e5b7627ffa5a6821fde44e31407d23a4d3da80a6e07a5b675af8d0925784",
            },
            "jobs": [{
                "id": format!("job{index}"),
                "is_wasm": false,
                "native_output_kind": "archive",
                "output": tmp_dir.join(format!("app-{index}.a")).to_string_lossy(),
                // Distinct application keys: each build compiles its own
                // application and prepares the shared stdlib entry.
                "cache_key": format!("module-{index}"),
                "function_cache_key": format!("function-{index}"),
                "ir_path": write_daemon_ir_fixture(&tmp_dir.join(format!("input-{index}.msgpack")), &serde_json::json!({
                    "functions": [
                        {"return_abi": "void", "name": "molt_main", "params": [], "ops": [{"kind": "call", "s_value": "demo__module"}]},
                        {"return_abi": "void", "name": "demo__module", "params": [], "ops": [{"kind": "call_internal", "s_value": "molt_init_sys"}, {"kind": "ret_void"}]},
                        {"return_abi": "void", "name": "molt_isolate_bootstrap", "params": [], "ops": [{"kind": "ret_void"}]},
                        {"return_abi": "void", "name": "molt_isolate_import", "params": ["p0"], "ops": [{"kind": "ret_void"}]},
                        {"return_abi": "void", "name": "molt_init_sys", "params": [], "ops": [{"kind": "ret_void"}]}
                    ],
                    "profile": null
                }))
            }]
        })
    };
    let cache = Arc::new(DaemonCache::new(None));
    let start = Arc::new(std::sync::Barrier::new(BUILDS));
    let builds: Vec<_> = (0..BUILDS)
        .map(|index| {
            let parsed = DaemonRequest::from_json_bytes(
                &serde_json::to_vec(&request(index)).expect("serialize request"),
            )
            .expect("parse request");
            let (cache, start) = (Arc::clone(&cache), Arc::clone(&start));
            std::thread::spawn(move || {
                let _scope = backend_env::enter(parsed.environment);
                let job = parsed.jobs.expect("jobs").pop().expect("job");
                start.wait();
                compile_single_job(job, &cache)
            })
        })
        .collect();
    for build in builds {
        let response = build.join().expect("build thread");
        assert!(response.ok, "build failed: {:?}", response.message);
    }
    let materialized = crate::backend_process::STDLIB_MATERIALIZED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&stdlib_key)
        .copied();
    assert_eq!(
        materialized,
        Some(1),
        "concurrent builds compiled one shared stdlib entry {materialized:?} times"
    );
    let first = std::fs::read(tmp_dir.join("stdlib-0.a")).expect("read stdlib archive");
    for index in 1..BUILDS {
        assert_eq!(
            std::fs::read(tmp_dir.join(format!("stdlib-{index}.a"))).expect("read stdlib archive"),
            first,
            "build {index} linked different stdlib bytes"
        );
    }
    let _ = std::fs::remove_dir_all(&tmp_dir);
}
