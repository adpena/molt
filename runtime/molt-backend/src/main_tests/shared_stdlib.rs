use super::*;

#[test]
fn user_owned_symbol_partition_uses_explicit_stdlib_modules() {
    let stdlib_modules = std::collections::BTreeSet::from(["sys".to_string(), "json".to_string()]);

    assert!(is_user_owned_symbol(
        "molt_main",
        "app",
        Some(&stdlib_modules)
    ));
    assert!(is_user_owned_symbol(
        "molt_host_init",
        "app",
        Some(&stdlib_modules)
    ));
    assert!(is_user_owned_symbol(
        "app__module",
        "app",
        Some(&stdlib_modules)
    ));
    assert!(is_user_owned_symbol(
        "molt_init_app",
        "app",
        Some(&stdlib_modules)
    ));
    assert!(is_user_owned_symbol(
        "molt_init___main__",
        "app",
        Some(&stdlib_modules)
    ));
    assert!(is_user_owned_symbol(
        "molt_isolate_import",
        "app",
        Some(&stdlib_modules)
    ));
    assert!(is_user_owned_symbol(
        "molt_isolate_bootstrap",
        "app",
        Some(&stdlib_modules)
    ));
    assert!(is_user_owned_symbol(
        "molt_init_main_molt",
        "app",
        Some(&stdlib_modules)
    ));
    assert!(is_user_owned_symbol(
        "main_molt__helper",
        "app",
        Some(&stdlib_modules)
    ));

    assert!(!is_user_owned_symbol(
        "molt_init_sys",
        "app",
        Some(&stdlib_modules)
    ));
    assert!(!is_user_owned_symbol(
        "molt_init_json",
        "app",
        Some(&stdlib_modules)
    ));
}

#[test]
fn shared_stdlib_cache_requires_matching_key() {
    let tmp_dir = std::env::temp_dir().join(format!(
        "molt-stdlib-cache-key-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
    let stdlib_path = tmp_dir.join("stdlib.o");
    std::fs::write(&stdlib_path, b"placeholder").expect("write stdlib object");

    write_shared_stdlib_cache_sidecars(
        &stdlib_path,
        7,
        Some("abc123"),
        Some("{\"cache_key\":\"abc123\"}"),
        "partition-a",
    )
    .expect("write sidecars");
    assert!(shared_stdlib_cache_matches(
        &stdlib_path,
        Some("abc123"),
        Some("{\"cache_key\":\"abc123\"}"),
        Some("partition-a"),
    ));
    assert!(shared_stdlib_cache_matches(
        &stdlib_path,
        Some("abc123"),
        Some("{\"cache_key\":\"abc123\"}"),
        None,
    ));
    assert!(!shared_stdlib_cache_matches(
        &stdlib_path,
        Some("def456"),
        Some("{\"cache_key\":\"abc123\"}"),
        Some("partition-a"),
    ));
    assert!(!shared_stdlib_cache_matches(
        &stdlib_path,
        Some("abc123"),
        Some("{\"cache_key\":\"def456\"}"),
        Some("partition-a"),
    ));
    assert!(!shared_stdlib_cache_matches(
        &stdlib_path,
        Some("abc123"),
        Some("{\"cache_key\":\"abc123\"}"),
        Some("partition-b"),
    ));
    assert!(!shared_stdlib_cache_matches(
        &stdlib_path,
        Some("abc123"),
        None,
        Some("partition-a"),
    ));
    assert!(!shared_stdlib_cache_matches(&stdlib_path, None, None, None));

    std::fs::remove_file(stdlib_cache_partition_manifest_sidecar_path(&stdlib_path))
        .expect("remove partition manifest");
    assert!(!shared_stdlib_cache_matches(
        &stdlib_path,
        Some("abc123"),
        Some("{\"cache_key\":\"abc123\"}"),
        None,
    ));
    assert!(!shared_stdlib_cache_matches(
        &stdlib_path,
        Some("abc123"),
        Some("{\"cache_key\":\"abc123\"}"),
        Some("partition-a"),
    ));

    write_shared_stdlib_cache_sidecars(
        &stdlib_path,
        7,
        Some("abc123"),
        Some("{\"cache_key\":\"abc123\"}"),
        "partition-a",
    )
    .expect("rewrite sidecars");
    std::fs::write(&stdlib_path, b"changed-object").expect("mutate object");
    assert!(!shared_stdlib_cache_matches(
        &stdlib_path,
        Some("abc123"),
        Some("{\"cache_key\":\"abc123\"}"),
        Some("partition-a"),
    ));

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn shared_stdlib_publish_lock_serializes_concurrent_threads() {
    let tmp_dir = std::env::temp_dir().join(format!(
        "molt-stdlib-publish-lock-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
    let stdlib_path = tmp_dir.join("stdlib.o");
    let first_inside = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let violation = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (first_entered_tx, first_entered_rx) = std::sync::mpsc::channel();
    let (second_entered_tx, second_entered_rx) = std::sync::mpsc::channel();

    let first_path = stdlib_path.clone();
    let first_inside_for_first = Arc::clone(&first_inside);
    let first_thread = std::thread::spawn(move || {
        with_shared_stdlib_cache_publish_lock(&first_path, || {
            first_inside_for_first.store(true, std::sync::atomic::Ordering::SeqCst);
            first_entered_tx.send(()).expect("signal first entered");
            std::thread::sleep(std::time::Duration::from_millis(150));
            first_inside_for_first.store(false, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        })
        .expect("first lock body");
    });

    first_entered_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("first thread entered lock");

    let second_path = stdlib_path.clone();
    let first_inside_for_second = Arc::clone(&first_inside);
    let violation_for_second = Arc::clone(&violation);
    let second_thread = std::thread::spawn(move || {
        with_shared_stdlib_cache_publish_lock(&second_path, || {
            if first_inside_for_second.load(std::sync::atomic::Ordering::SeqCst) {
                violation_for_second.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            second_entered_tx.send(()).expect("signal second entered");
            Ok(())
        })
        .expect("second lock body");
    });

    assert!(
        second_entered_rx
            .recv_timeout(std::time::Duration::from_millis(40))
            .is_err(),
        "second publisher entered while the first publisher held the lock"
    );
    first_thread.join().expect("join first publisher");
    second_entered_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("second publisher eventually entered");
    second_thread.join().expect("join second publisher");
    assert!(
        !violation.load(std::sync::atomic::Ordering::SeqCst),
        "shared stdlib publish lock allowed overlapping writers"
    );

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn shared_stdlib_partition_manifest_tracks_names_and_bodies() {
    let func_a = FunctionIR {
        name: "molt_init_sys".to_string(),
        params: vec![],
        ops: vec![OpIR {
            kind: "return_none".to_string(),
            ..OpIR::default()
        }],
        param_types: None,
        source_file: None,
        is_extern: false,
    };
    let func_b = FunctionIR {
        name: "sys__version".to_string(),
        params: vec![],
        ops: vec![OpIR {
            kind: "const_str".to_string(),
            s_value: Some("3.12".to_string()),
            ..OpIR::default()
        }],
        param_types: None,
        source_file: None,
        is_extern: false,
    };
    let mut changed = func_b.clone();
    changed.ops[0].s_value = Some("3.13".to_string());

    let ordered = shared_stdlib_partition_manifest(&[func_a.clone(), func_b.clone()])
        .expect("partition manifest");
    let reordered =
        shared_stdlib_partition_manifest(&[func_b, func_a.clone()]).expect("partition manifest");
    let body_changed =
        shared_stdlib_partition_manifest(&[func_a, changed]).expect("partition manifest");

    assert_eq!(ordered, reordered);
    assert_ne!(ordered, body_changed);
    assert!(ordered.contains("\"molt_init_sys\""));
    assert!(ordered.contains("\"sys__version\""));
    assert!(ordered.contains("\"schema\":\"stdlib-partition-v1\""));
}

#[test]
fn shared_stdlib_partition_rejects_unclosed_copy_reference() {
    let userdict_copy = FunctionIR {
        name: "collections__UserDict_copy".to_string(),
        params: vec!["self".to_string()],
        ops: vec![OpIR {
            kind: "call".to_string(),
            s_value: Some("copy__copy".to_string()),
            args: Some(vec!["self".to_string()]),
            out: Some("v0".to_string()),
            ..OpIR::default()
        }],
        param_types: None,
        source_file: None,
        is_extern: false,
    };
    let copy_init = FunctionIR {
        name: "molt_init_copy".to_string(),
        params: vec![],
        ops: vec![OpIR {
            kind: "call".to_string(),
            s_value: Some("copy__molt_module_chunk_1".to_string()),
            out: Some("v0".to_string()),
            ..OpIR::default()
        }],
        param_types: None,
        source_file: None,
        is_extern: false,
    };
    let copy_chunk = FunctionIR {
        name: "copy__molt_module_chunk_1".to_string(),
        params: vec![],
        ops: vec![OpIR {
            kind: "ret_void".to_string(),
            ..OpIR::default()
        }],
        param_types: None,
        source_file: None,
        is_extern: false,
    };
    let copy_copy = FunctionIR {
        name: "copy__copy".to_string(),
        params: vec!["obj".to_string()],
        ops: vec![OpIR {
            kind: "ret".to_string(),
            args: Some(vec!["obj".to_string()]),
            ..OpIR::default()
        }],
        param_types: None,
        source_file: None,
        is_extern: false,
    };
    let valid_partition = vec![
        userdict_copy.clone(),
        copy_init,
        copy_chunk,
        copy_copy.clone(),
    ];
    let valid_function_names: std::collections::BTreeSet<String> = valid_partition
        .iter()
        .map(|func| func.name.clone())
        .collect();
    validate_shared_stdlib_partition(&valid_partition, &valid_function_names)
        .expect("closed partition");

    let invalid_partition = vec![userdict_copy];
    let invalid_function_names: std::collections::BTreeSet<String> =
        ["collections__UserDict_copy", "copy__copy"]
            .into_iter()
            .map(str::to_string)
            .collect();
    let issue = shared_stdlib_partition_closure_issue(&invalid_partition, &invalid_function_names)
        .expect("missing copy reference");
    assert!(issue.contains("collections__UserDict_copy -> copy__copy"));
    assert!(validate_shared_stdlib_partition(&invalid_partition, &invalid_function_names).is_err());
}

#[test]
fn shared_stdlib_cache_sidecar_write_failures_propagate() {
    let tmp_dir = std::env::temp_dir().join(format!(
        "molt-stdlib-cache-key-error-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
    let blocking = tmp_dir.join("not-a-dir");
    std::fs::write(&blocking, b"x").expect("write blocking file");
    let stdlib_path = blocking.join("stdlib.o");

    let err = write_shared_stdlib_cache_sidecars(
        &stdlib_path,
        7,
        Some("abc123"),
        Some("{\"cache_key\":\"abc123\"}"),
        "partition-a",
    )
    .expect_err("sidecar writes should fail when parent is not a directory");
    assert!(!err.to_string().is_empty());

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn dead_function_elimination_prunes_stdlib_before_partition() {
    let mut ir = SimpleIR {
        functions: vec![
            FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "call_internal".to_string(),
                    s_value: Some("molt_init_sys".to_string()),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
            FunctionIR {
                name: "molt_init_app".to_string(),
                params: vec![],
                ops: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
            FunctionIR {
                name: "app__module".to_string(),
                params: vec![],
                ops: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
            FunctionIR {
                name: "molt_init_sys".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "code_slot_set".to_string(),
                    value: Some(73),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
            FunctionIR {
                name: "molt_init_json".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "code_slot_set".to_string(),
                    value: Some(843),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
        ],
        profile: None,
    };

    molt_backend::inject_runtime_exit(&mut ir);
    molt_backend::eliminate_dead_functions(&mut ir);
    molt_backend::eliminate_dead_imports(&mut ir);
    molt_backend::eliminate_dead_ops(&mut ir);
    let retained: std::collections::BTreeSet<_> =
        ir.functions.iter().map(|func| func.name.as_str()).collect();

    assert!(retained.contains("molt_main"));
    assert!(retained.contains("molt_init_sys"));
    assert!(!retained.contains("molt_init_json"));
}

#[test]
fn prune_and_partition_native_stdlib_keeps_only_reachable_stdlib() {
    let mut ir = SimpleIR {
        functions: vec![
            FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "call_internal".to_string(),
                    s_value: Some("molt_init_sys".to_string()),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
            FunctionIR {
                name: "molt_init_app".to_string(),
                params: vec![],
                ops: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
            FunctionIR {
                name: "app__module".to_string(),
                params: vec![],
                ops: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
            FunctionIR {
                name: "molt_init_sys".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "code_slot_set".to_string(),
                    value: Some(73),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
            FunctionIR {
                name: "molt_init_json".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "code_slot_set".to_string(),
                    value: Some(843),
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
        &mut ir,
        "app",
        Some(&stdlib_modules),
        &std::collections::BTreeSet::new(),
    );
    let user_names: Vec<_> = user_remaining
        .iter()
        .map(|func| func.name.as_str())
        .collect();
    let stdlib_names: Vec<_> = stdlib_funcs.iter().map(|func| func.name.as_str()).collect();

    assert_eq!(user_names, vec!["molt_main"]);
    assert_eq!(stdlib_names, vec!["molt_init_sys"]);
}

#[test]
fn prune_and_partition_native_stdlib_keeps_non_entry_user_module_in_user_partition() {
    let mut ir = SimpleIR {
        functions: vec![
            FunctionIR {
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
            },
            FunctionIR {
                name: "demo__module".to_string(),
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
        ],
        profile: None,
    };

    let stdlib_modules = std::collections::BTreeSet::new();
    let (user_remaining, stdlib_funcs) = prune_and_partition_native_stdlib(
        &mut ir,
        "__main__",
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
        vec!["molt_main", "demo__module", "molt_isolate_import"]
    );
    assert!(stdlib_names.is_empty());
}

#[test]
fn compile_stdlib_cache_object_emits_parseable_empty_object() {
    let tmp_dir = std::env::temp_dir().join(format!(
        "molt-empty-stdlib-cache-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
    let stdlib = tmp_dir.join("empty-stdlib.o");

    compile_stdlib_cache_object(&stdlib, Vec::new(), None, None, "MOLT_BACKEND(test)")
        .expect("empty stdlib cache must emit an object");

    let bytes = std::fs::read(&stdlib).expect("read emitted empty stdlib object");
    assert!(
        !bytes.is_empty(),
        "empty stdlib cache path must publish a real object file"
    );
    object::File::parse(&*bytes).expect("empty stdlib cache must be a parseable object");

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn daemon_empty_stdlib_partition_emits_cache_artifact_and_sidecars() {
    let _env_guard = ENV_TEST_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let tmp_dir = std::env::temp_dir().join(format!(
        "molt-daemon-empty-stdlib-cache-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
    let output = tmp_dir.join("out.o");
    let stdlib = tmp_dir.join("stdlib.o");
    let runtime_symbols = tmp_dir.join("runtime_callable_symbols.txt");
    std::fs::write(&runtime_symbols, "molt_main\n").expect("write runtime symbols");

    let env_keys = [
        "MOLT_ENTRY_MODULE",
        "MOLT_STDLIB_OBJ",
        "MOLT_STDLIB_CACHE_KEY",
        "MOLT_STDLIB_CACHE_MANIFEST",
        "MOLT_STDLIB_MODULE_SYMBOLS",
        "MOLT_RUNTIME_CALLABLE_SYMBOLS",
    ];
    let prior_env: Vec<(&str, Option<String>)> = env_keys
        .iter()
        .copied()
        .map(|key| (key, std::env::var(key).ok()))
        .collect();
    unsafe {
        std::env::set_var("MOLT_ENTRY_MODULE", "demo");
        std::env::set_var("MOLT_STDLIB_OBJ", &stdlib);
        std::env::set_var("MOLT_STDLIB_CACHE_KEY", "daemon-empty-key");
        std::env::set_var("MOLT_STDLIB_CACHE_MANIFEST", "daemon-empty-manifest");
        std::env::set_var("MOLT_STDLIB_MODULE_SYMBOLS", "[\"sys\"]");
        std::env::set_var("MOLT_RUNTIME_CALLABLE_SYMBOLS", &runtime_symbols);
    }

    let job = DaemonJobRequest {
        id: "job0".to_string(),
        is_wasm: false,
        target_triple: None,
        wasm_link: false,
        wasm_data_base: None,
        wasm_table_base: None,
        wasm_split_runtime_app_table_base: None,
        output: output.to_string_lossy().into_owned(),
        cache_key: "".to_string(),
        function_cache_key: None,
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
                        ops: vec![OpIR {
                            kind: "ret_void".to_string(),
                            ..OpIR::default()
                        }],
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
                ],
                profile: None,
            },
        }),
        ir_path: None,
    };

    let mut cache = DaemonCache::new(None);
    let result = compile_single_job(job, &mut cache);

    for (key, value) in prior_env {
        match value {
            Some(value) => unsafe { std::env::set_var(key, value) },
            None => unsafe { std::env::remove_var(key) },
        }
    }

    assert!(result.ok, "daemon compile failed: {:?}", result.message);
    assert!(output.exists(), "application output object missing");
    let stdlib_bytes = std::fs::read(&stdlib).expect("read daemon empty stdlib object");
    assert!(
        !stdlib_bytes.is_empty(),
        "daemon empty stdlib cache must publish a real object"
    );
    object::File::parse(&*stdlib_bytes)
        .expect("daemon empty stdlib cache must be a parseable object");
    assert_eq!(
        std::fs::read_to_string(stdlib_cache_count_sidecar_path(&stdlib))
            .expect("read stdlib count sidecar"),
        "0"
    );
    assert_eq!(
        read_stdlib_cache_key(&stdlib).as_deref(),
        Some("daemon-empty-key")
    );
    assert_eq!(
        read_stdlib_cache_manifest(&stdlib).as_deref(),
        Some("daemon-empty-manifest")
    );
    let partition_manifest =
        std::fs::read_to_string(stdlib_cache_partition_manifest_sidecar_path(&stdlib))
            .expect("read stdlib partition manifest");
    assert!(partition_manifest.contains("\"functions\":[]"));

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn daemon_native_without_stdlib_obj_keeps_full_ir() {
    let _env_guard = ENV_TEST_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut ir = SimpleIR {
        functions: vec![
            FunctionIR {
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
            },
            FunctionIR {
                name: "demo__module".to_string(),
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
        ],
        profile: None,
    };

    let stdlib_obj_path = std::env::var("MOLT_STDLIB_OBJ").ok();
    let entry_module = std::env::var("MOLT_ENTRY_MODULE").ok();
    unsafe {
        std::env::remove_var("MOLT_STDLIB_OBJ");
        std::env::remove_var("MOLT_ENTRY_MODULE");
    }

    // Mirror the daemon native path: without a stdlib cache target,
    // it must compile the full IR, not the drained remainder.
    let maybe_stdlib = std::env::var("MOLT_STDLIB_OBJ").ok();
    if maybe_stdlib.is_none() {
        molt_backend::inject_runtime_exit(&mut ir);
        molt_backend::eliminate_dead_functions(&mut ir);
        molt_backend::eliminate_dead_imports(&mut ir);
        molt_backend::eliminate_dead_ops(&mut ir);
    }

    let names: Vec<_> = ir.functions.iter().map(|func| func.name.as_str()).collect();

    match stdlib_obj_path {
        Some(value) => unsafe { std::env::set_var("MOLT_STDLIB_OBJ", value) },
        None => unsafe { std::env::remove_var("MOLT_STDLIB_OBJ") },
    }
    match entry_module {
        Some(value) => unsafe { std::env::set_var("MOLT_ENTRY_MODULE", value) },
        None => unsafe { std::env::remove_var("MOLT_ENTRY_MODULE") },
    }

    assert_eq!(
        names,
        vec![
            "molt_main",
            "demo__module",
            "molt_isolate_bootstrap",
            "molt_isolate_import"
        ]
    );
}
