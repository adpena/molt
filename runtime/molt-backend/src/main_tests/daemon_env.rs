use super::*;

#[test]
#[cfg(feature = "native-backend")]
fn daemon_request_with_env_preserves_user_entry_object() {
    let _env_guard = ENV_TEST_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let tmp_dir = std::env::temp_dir().join(format!(
        "molt-daemon-request-env-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
    let output = tmp_dir.join("out.o");
    let stdlib = tmp_dir.join("stdlib.o");
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
    let request = serde_json::json!({
        "version": BACKEND_DAEMON_PROTOCOL_VERSION,
        "config_digest": "daemon-test",
        "env": {
            "MOLT_ENTRY_MODULE": "demo",
            "MOLT_STDLIB_OBJ": stdlib.to_string_lossy(),
            "MOLT_STDLIB_CACHE_KEY": "daemon-stdlib-key",
            "MOLT_STDLIB_MODULE_SYMBOLS": "[\"sys\"]",
            "MOLT_RUNTIME_CALLABLE_SYMBOLS": runtime_symbols.to_string_lossy(),
        },
        "jobs": [{
            "id": "job0",
            "is_wasm": false,
            "output": output.to_string_lossy(),
            "cache_key": "",
            "function_cache_key": "",
            "ir": {
                "functions": [
                    {"name": "molt_main", "params": [], "ops": [{"kind": "call", "s_value": "demo__module", "value": 0}]},
                    {"name": "demo__module", "params": [], "ops": [{"kind": "call_internal", "s_value": "molt_init_sys"}, {"kind": "ret_void"}]},
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
    assert_eq!(
        std::env::var("MOLT_ENTRY_MODULE").ok().as_deref(),
        Some("demo")
    );
    assert_eq!(
        std::env::var("MOLT_STDLIB_OBJ").ok().as_deref(),
        Some(stdlib.to_string_lossy().as_ref())
    );
    assert_eq!(
        std::env::var("MOLT_STDLIB_MODULE_SYMBOLS").ok().as_deref(),
        Some("[\"sys\"]")
    );
    let job = request.jobs.expect("jobs").into_iter().next().expect("job");
    let mut partition_ir = SimpleIR {
        functions: vec![
            FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "call".to_string(),
                    s_value: Some("demo__module".to_string()),
                    value: Some(0),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
            FunctionIR {
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
            },
            FunctionIR {
                name: "molt_isolate_bootstrap".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
            FunctionIR {
                name: "molt_isolate_import".to_string(),
                params: vec!["p0".to_string()],
                ops: vec![OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
            FunctionIR {
                name: "molt_init_sys".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
        ],
        profile: None,
    };
    let stdlib_modules = std::collections::BTreeSet::from(["sys".to_string()]);
    let (user_remaining, stdlib_funcs) = prune_and_partition_native_stdlib(
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
    let mut cache = DaemonCache::new(None);
    let result = compile_single_job(job, &mut cache);

    assert!(result.ok, "daemon compile failed: {:?}", result.message);
    assert!(output.exists(), "output object missing");
    assert!(
        output.metadata().expect("output metadata").len() > 240,
        "daemon path emitted empty object"
    );

    // The daemon env-passthrough mutated the process environment; clear the
    // resolver symbol-set var so it does not leak into sibling tests that
    // share `ENV_TEST_MUTEX`.
    unsafe { std::env::remove_var("MOLT_RUNTIME_CALLABLE_SYMBOLS") };
    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn daemon_request_env_clears_omitted_stdlib_module_symbols() {
    let _env_guard = ENV_TEST_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    unsafe {
        std::env::set_var("MOLT_STDLIB_MODULE_SYMBOLS", "[\"stale\"]");
        std::env::set_var("MOLT_ENTRY_MODULE", "stale_entry");
    }

    let request = serde_json::json!({
        "version": BACKEND_DAEMON_PROTOCOL_VERSION,
        "config_digest": "daemon-clear-test",
        "env": {
            "MOLT_ENTRY_MODULE": "demo",
        },
        "jobs": [],
    });

    let parsed = DaemonRequest::from_json_bytes(
        serde_json::to_string(&request)
            .expect("serialize request")
            .as_bytes(),
    )
    .expect("parse daemon request");

    assert_eq!(parsed.version, Some(BACKEND_DAEMON_PROTOCOL_VERSION));
    assert_eq!(
        std::env::var("MOLT_ENTRY_MODULE").ok().as_deref(),
        Some("demo")
    );
    assert!(std::env::var("MOLT_STDLIB_MODULE_SYMBOLS").is_err());
}

#[test]
fn daemon_request_env_clears_omitted_resource_and_trace_keys_between_requests() {
    let _env_guard = ENV_TEST_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let keys = [
        "MOLT_BACKEND_MEMORY_AVAILABLE_GB",
        "MOLT_CLI_MEMORY_AVAILABLE_GB",
        "MOLT_CLI_MEM_AVAILABLE_GB",
        "MOLT_MEMORY_AVAILABLE_GB",
        "MOLT_MEM_AVAILABLE_GB",
        "MOLT_BACKEND_MAX_RSS_GB",
        "MOLT_BACKEND_MEMORY_RESERVE_GB",
        "MOLT_CLI_MEMORY_RESERVE_GB",
        "MOLT_CLI_MEM_RESERVE_GB",
        "MOLT_MEMORY_RESERVE_GB",
        "MOLT_MEM_RESERVE_GB",
        "RAYON_NUM_THREADS",
        "MOLT_TIR_TRACE_FUNC",
    ];
    let prior_env: Vec<_> = keys
        .iter()
        .map(|key| (*key, std::env::var(key).ok()))
        .collect();
    unsafe {
        for key in keys {
            std::env::set_var(key, "stale");
        }
    }

    let first = serde_json::json!({
        "version": BACKEND_DAEMON_PROTOCOL_VERSION,
        "config_digest": "daemon-resource-env-set",
        "env": {
            "MOLT_BACKEND_MEMORY_AVAILABLE_GB": "9",
            "MOLT_BACKEND_MEMORY_RESERVE_GB": "1",
            "RAYON_NUM_THREADS": "3",
            "MOLT_TIR_TRACE_FUNC": "target_func",
        },
        "jobs": [],
    });
    DaemonRequest::from_json_bytes(
        serde_json::to_string(&first)
            .expect("serialize first request")
            .as_bytes(),
    )
    .expect("parse first daemon request");
    assert_eq!(
        std::env::var("MOLT_BACKEND_MEMORY_AVAILABLE_GB")
            .ok()
            .as_deref(),
        Some("9")
    );
    assert_eq!(
        std::env::var("MOLT_BACKEND_MEMORY_RESERVE_GB")
            .ok()
            .as_deref(),
        Some("1")
    );
    assert_eq!(
        std::env::var("RAYON_NUM_THREADS").ok().as_deref(),
        Some("3")
    );
    assert_eq!(
        std::env::var("MOLT_TIR_TRACE_FUNC").ok().as_deref(),
        Some("target_func")
    );

    let second = serde_json::json!({
        "version": BACKEND_DAEMON_PROTOCOL_VERSION,
        "config_digest": "daemon-resource-env-clear",
        "env": {},
        "jobs": [],
    });
    DaemonRequest::from_json_bytes(
        serde_json::to_string(&second)
            .expect("serialize second request")
            .as_bytes(),
    )
    .expect("parse second daemon request");

    for key in keys {
        assert!(
            std::env::var(key).is_err(),
            "{key} leaked across daemon requests"
        );
    }
    for (key, value) in prior_env {
        match value {
            Some(value) => unsafe { std::env::set_var(key, value) },
            None => unsafe { std::env::remove_var(key) },
        }
    }
}

#[test]
fn daemon_request_env_rejects_malformed_stdlib_module_symbols() {
    let _env_guard = ENV_TEST_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    unsafe {
        std::env::set_var("MOLT_STDLIB_MODULE_SYMBOLS", "[\"stale\"]");
    }
    let request = serde_json::json!({
        "version": BACKEND_DAEMON_PROTOCOL_VERSION,
        "config_digest": "daemon-bad-stdlib-symbols-test",
        "env": {
            "MOLT_STDLIB_MODULE_SYMBOLS": "not-json",
        },
        "jobs": [],
    });

    let err = DaemonRequest::from_json_bytes(
        serde_json::to_string(&request)
            .expect("serialize request")
            .as_bytes(),
    )
    .expect_err("malformed stdlib symbol authority must fail closed");
    assert!(
        err.contains("MOLT_STDLIB_MODULE_SYMBOLS must be a JSON array of strings"),
        "unexpected error message: {err}"
    );
    assert!(std::env::var("MOLT_STDLIB_MODULE_SYMBOLS").is_err());
}
