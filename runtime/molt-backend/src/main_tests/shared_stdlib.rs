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
    let stdlib_path = tmp_dir.join("stdlib.a");
    let mut object = object::write::Object::new(
        object::BinaryFormat::Elf,
        object::Architecture::X86_64,
        object::Endianness::Little,
    );
    object.add_section(Vec::new(), b".text".to_vec(), object::SectionKind::Text);
    let bytes = object.write().expect("write object fixture");
    crate::backend_process::write_native_archive_bytes(&stdlib_path, &bytes)
        .expect("write stdlib archive");

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

    // Even current sidecars cannot relabel an old merged object as an archive.
    std::fs::write(&stdlib_path, &bytes).expect("replace archive with old-style object");
    write_shared_stdlib_cache_sidecars(
        &stdlib_path,
        7,
        Some("abc123"),
        Some("{\"cache_key\":\"abc123\"}"),
        "partition-a",
    )
    .expect("write matching object sidecars");
    assert!(!shared_stdlib_cache_matches(
        &stdlib_path,
        Some("abc123"),
        Some("{\"cache_key\":\"abc123\"}"),
        Some("partition-a")
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
    let stdlib_path = tmp_dir.join("stdlib.a");
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
        execution_context: Default::default(),
    };
    let func_b = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Void,
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
        codegen_partition: false,
        execution_context: Default::default(),
    };
    let mut changed = func_b.clone();
    changed.ops[0].s_value = Some("3.13".to_string());

    let mut ordered_functions = vec![func_a.clone(), func_b.clone()];
    let mut reordered_functions = vec![func_b, func_a.clone()];
    let mut changed_functions = vec![func_a, changed];
    let mut context_changed_functions = ordered_functions.clone();
    context_changed_functions[0].execution_context =
        molt_backend::ir::ExecutionContextPolicy::Local;
    let ordered_context =
        molt_backend::SimpleBackend::prepare_module_context(&mut ordered_functions);
    let reordered_context =
        molt_backend::SimpleBackend::prepare_module_context(&mut reordered_functions);
    let changed_context =
        molt_backend::SimpleBackend::prepare_module_context(&mut changed_functions);
    let context_changed_context =
        molt_backend::SimpleBackend::prepare_module_context(&mut context_changed_functions);
    let ordered = shared_stdlib_partition_manifest(&ordered_functions, &ordered_context)
        .expect("partition manifest");
    let reordered = shared_stdlib_partition_manifest(&reordered_functions, &reordered_context)
        .expect("partition manifest");
    let body_changed = shared_stdlib_partition_manifest(&changed_functions, &changed_context)
        .expect("partition manifest");
    let execution_context_changed =
        shared_stdlib_partition_manifest(&context_changed_functions, &context_changed_context)
            .expect("execution-context-mutated partition manifest");
    let mut altered_context_json =
        serde_json::to_value(&ordered_context).expect("serialize module context");
    altered_context_json["function_linkage_abis"]["molt_init_sys"]["return_type"] =
        serde_json::json!("F64");
    let altered_context: molt_backend::NativeBackendModuleContext =
        serde_json::from_value(altered_context_json).expect("deserialize altered linkage context");
    let linkage_changed = shared_stdlib_partition_manifest(&ordered_functions, &altered_context)
        .expect("linkage-mutated partition manifest");

    assert_eq!(ordered, reordered);
    assert_ne!(ordered, body_changed);
    assert_ne!(
        ordered, execution_context_changed,
        "cache admission manifest must bind the complete FunctionIR contract"
    );
    assert_ne!(
        ordered, linkage_changed,
        "cache admission manifest must bind exact linkage carriers even when FunctionIR is unchanged"
    );
    assert!(ordered.contains("\"molt_init_sys\""));
    assert!(ordered.contains("\"sys__version\""));
    assert!(ordered.contains("\"schema\":\"stdlib-partition-v2-exact-linkage-abi\""));
}

#[test]
fn shared_stdlib_partition_rejects_unclosed_copy_reference() {
    let userdict_copy = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Void,
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
        codegen_partition: false,
        execution_context: Default::default(),
    };
    let copy_init = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Void,
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
        codegen_partition: false,
        execution_context: Default::default(),
    };
    let copy_chunk = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: "copy__molt_module_chunk_1".to_string(),
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
    };
    let copy_copy = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
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
        codegen_partition: false,
        execution_context: Default::default(),
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
    let stdlib_path = blocking.join("stdlib.a");

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

fn stdlib_code_slot_fixture(code_id: i64) -> Vec<OpIR> {
    vec![
        OpIR {
            kind: "const_str".into(),
            s_value: Some("<module>".into()),
            out: Some("code_name".into()),
            ..OpIR::default()
        },
        OpIR {
            kind: "const".into(),
            value: Some(0),
            out: Some("zero".into()),
            ..OpIR::default()
        },
        OpIR {
            kind: "const_none".into(),
            out: Some("linetable".into()),
            ..OpIR::default()
        },
        OpIR {
            kind: "tuple_new".into(),
            args: Some(vec![]),
            out: Some("names".into()),
            ..OpIR::default()
        },
        OpIR {
            kind: "dict_new".into(),
            args: Some(vec![]),
            out: Some("globals".into()),
            ..OpIR::default()
        },
        OpIR {
            kind: "code_new".into(),
            args: Some(
                [
                    "code_name",
                    "code_name",
                    "zero",
                    "linetable",
                    "names",
                    "names",
                    "zero",
                    "zero",
                    "zero",
                ]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            ),
            out: Some("code".into()),
            ..OpIR::default()
        },
        OpIR {
            kind: "code_slot_set".into(),
            args: Some(vec!["code".into(), "globals".into()]),
            value: Some(code_id),
            ..OpIR::default()
        },
    ]
}

#[test]
fn dead_function_elimination_prunes_stdlib_before_partition() {
    let mut ir = SimpleIR {
        functions: vec![
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Void,
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
                codegen_partition: false,
                execution_context: Default::default(),
            },
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "molt_init_app".to_string(),
                params: vec![],
                ops: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                execution_context: Default::default(),
            },
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "app__module".to_string(),
                params: vec![],
                ops: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                execution_context: Default::default(),
            },
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Value,
                name: "molt_init_sys".to_string(),
                params: vec![],
                ops: stdlib_code_slot_fixture(73),
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                execution_context: Default::default(),
            },
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Value,
                name: "molt_init_json".to_string(),
                params: vec![],
                ops: stdlib_code_slot_fixture(843),
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                execution_context: Default::default(),
            },
        ],
        profile: None,
    };

    molt_backend::inject_runtime_exit(&mut ir);
    molt_backend::eliminate_dead_functions(&mut ir);
    molt_backend::eliminate_dead_imports(&mut ir);
    molt_backend::eliminate_dead_ops(
        &mut ir,
        &molt_backend::tir::target_info::TargetInfo::native_release_fast(),
    );
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
                return_abi: molt_ir::FunctionReturnAbi::Void,
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
                codegen_partition: false,
                execution_context: Default::default(),
            },
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "molt_init_app".to_string(),
                params: vec![],
                ops: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                execution_context: Default::default(),
            },
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "app__module".to_string(),
                params: vec![],
                ops: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                execution_context: Default::default(),
            },
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Value,
                name: "molt_init_sys".to_string(),
                params: vec![],
                ops: stdlib_code_slot_fixture(73),
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                execution_context: Default::default(),
            },
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Value,
                name: "molt_init_json".to_string(),
                params: vec![],
                ops: stdlib_code_slot_fixture(843),
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                execution_context: Default::default(),
            },
        ],
        profile: None,
    };

    let stdlib_modules = std::collections::BTreeSet::from(["sys".to_string()]);
    let (user_remaining, stdlib_funcs, _module_context) = prune_and_partition_native_stdlib(
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
                execution_context: Default::default(),
            },
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "demo__module".to_string(),
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
                execution_context: Default::default(),
            },
        ],
        profile: None,
    };

    let stdlib_modules = std::collections::BTreeSet::new();
    let (user_remaining, stdlib_funcs, _module_context) = prune_and_partition_native_stdlib(
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
fn compile_stdlib_cache_archive_emits_parseable_empty_member() {
    let tmp_dir = std::env::temp_dir().join(format!(
        "molt-empty-stdlib-cache-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
    let stdlib = tmp_dir.join("empty-stdlib.a");

    compile_stdlib_cache_archive(
        &stdlib,
        Vec::new(),
        None,
        None,
        "MOLT_BACKEND(test)",
        molt_backend::NativeBackendModuleContext::default(),
    )
    .expect("empty stdlib cache must emit an object");

    let bytes = std::fs::read(&stdlib).expect("read emitted empty stdlib object");
    assert!(
        !bytes.is_empty(),
        "empty stdlib cache path must publish a real object file"
    );
    assert_native_archive_members(&bytes, 1);

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn daemon_empty_stdlib_partition_emits_cache_artifact_and_sidecars() {
    let _env_guard = TestEnvGuard::clear(DAEMON_REQUEST_ENV_KEYS);
    let tmp_dir = std::env::temp_dir().join(format!(
        "molt-daemon-empty-stdlib-cache-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
    let output = tmp_dir.join("out.a");
    let stdlib = tmp_dir.join("stdlib.a");
    let runtime_symbols = tmp_dir.join("runtime_callable_symbols.txt");
    std::fs::write(&runtime_symbols, "molt_main\n").expect("write runtime symbols");

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
        native_output_kind: NativeArtifactKind::Archive,
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
                        return_abi: molt_ir::FunctionReturnAbi::Void,
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
                        codegen_partition: false,
                        execution_context: Default::default(),
                    },
                    FunctionIR {
                        return_abi: molt_ir::FunctionReturnAbi::Void,
                        name: "demo__module".to_string(),
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
                        execution_context: Default::default(),
                    },
                ],
                profile: None,
            },
        }),
        ir_path: None,
    };

    let mut cache = DaemonCache::new(None);
    let result = compile_single_job(job, &mut cache);

    assert!(result.ok, "daemon compile failed: {:?}", result.message);
    assert_native_archive_members(
        &std::fs::read(&output).expect("read application archive"),
        1,
    );
    let stdlib_bytes = std::fs::read(&stdlib).expect("read daemon empty stdlib archive");
    assert!(
        !stdlib_bytes.is_empty(),
        "daemon empty stdlib cache must publish a real archive"
    );
    assert_native_archive_members(&stdlib_bytes, 1);
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
    let _env_guard = TestEnvGuard::clear(&["MOLT_STDLIB_OBJ", "MOLT_ENTRY_MODULE"]);
    let mut ir = SimpleIR {
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
                execution_context: Default::default(),
            },
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "demo__module".to_string(),
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
                execution_context: Default::default(),
            },
        ],
        profile: None,
    };

    // Mirror the daemon native path: without a stdlib cache target,
    // it must compile the full IR, not the drained remainder.
    let maybe_stdlib = crate::backend_process::shared_stdlib_archive_path_from_env()
        .expect("shared stdlib request admission");
    if maybe_stdlib.is_none() {
        molt_backend::inject_runtime_exit(&mut ir);
        molt_backend::eliminate_dead_functions(&mut ir);
        molt_backend::eliminate_dead_imports(&mut ir);
        molt_backend::eliminate_dead_ops(
            &mut ir,
            &molt_backend::tir::target_info::TargetInfo::native_release_fast(),
        );
    }

    let names: Vec<_> = ir.functions.iter().map(|func| func.name.as_str()).collect();

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
