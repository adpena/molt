use super::*;

#[test]
fn partition_functions_for_batches_respects_op_budget() {
    let funcs = vec![
        FunctionIR {
            name: "a".to_string(),
            params: vec![],
            ops: vec![Default::default(); 90],
            param_types: None,
            source_file: None,
            is_extern: false,
            execution_context: Default::default(),
        },
        FunctionIR {
            name: "b".to_string(),
            params: vec![],
            ops: vec![Default::default(); 90],
            param_types: None,
            source_file: None,
            is_extern: false,
            execution_context: Default::default(),
        },
        FunctionIR {
            name: "c".to_string(),
            params: vec![],
            ops: vec![Default::default(); 10],
            param_types: None,
            source_file: None,
            is_extern: false,
            execution_context: Default::default(),
        },
    ];

    let batches = partition_functions_for_batches(funcs, 64, 100);
    let names: Vec<Vec<String>> = batches
        .into_iter()
        .map(|batch| batch.into_iter().map(|f| f.name).collect())
        .collect();

    assert_eq!(
        names,
        vec![
            vec!["a".to_string()],
            vec!["b".to_string(), "c".to_string()],
        ]
    );
}

#[test]
fn partition_functions_for_batches_respects_count_budget() {
    let funcs = (0..5)
        .map(|idx| FunctionIR {
            name: format!("f{idx}"),
            params: vec![],
            ops: vec![Default::default(); 1],
            param_types: None,
            source_file: None,
            is_extern: false,
            execution_context: Default::default(),
        })
        .collect();

    let batches = partition_functions_for_batches(funcs, 2, 1000);
    let sizes: Vec<usize> = batches.into_iter().map(|batch| batch.len()).collect();

    assert_eq!(sizes, vec![2, 2, 1]);
}

#[test]
fn relocatable_linker_binary_prefers_override_then_env() {
    let _env_guard = ENV_TEST_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let prior_molt_linker = std::env::var("MOLT_LINKER").ok();
    let prior_ld = std::env::var("LD").ok();
    let prior_cc = std::env::var("CC").ok();

    unsafe {
        std::env::set_var("MOLT_LINKER", "molt-ld");
        std::env::set_var("LD", "system-ld");
        std::env::set_var("CC", "clang");
    }
    assert_eq!(relocatable_linker_binary(Some("explicit")), "explicit");
    assert_eq!(relocatable_linker_binary(None), "molt-ld");

    unsafe {
        std::env::remove_var("MOLT_LINKER");
    }
    assert_eq!(relocatable_linker_binary(None), "system-ld");

    unsafe {
        std::env::remove_var("LD");
    }
    assert_eq!(relocatable_linker_binary(None), "clang");

    match prior_molt_linker {
        Some(value) => unsafe { std::env::set_var("MOLT_LINKER", value) },
        None => unsafe { std::env::remove_var("MOLT_LINKER") },
    }
    match prior_ld {
        Some(value) => unsafe { std::env::set_var("LD", value) },
        None => unsafe { std::env::remove_var("LD") },
    }
    match prior_cc {
        Some(value) => unsafe { std::env::set_var("CC", value) },
        None => unsafe { std::env::remove_var("CC") },
    }
}

#[test]
fn merge_relocatable_objects_copies_single_input() {
    let tmp_dir = std::env::temp_dir().join(format!(
        "molt-merge-reloc-single-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
    let input = tmp_dir.join("input.o");
    let output = tmp_dir.join("output.o");
    std::fs::write(&input, b"object-bytes").expect("write input object");

    merge_relocatable_objects(
        &output,
        std::slice::from_ref(&input),
        Some("linker-that-must-not-run"),
    )
    .expect("copy single input object");

    assert_eq!(
        std::fs::read(&output).expect("read merged output"),
        b"object-bytes"
    );

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn merge_relocatable_objects_reports_linker_failure() {
    let tmp_dir = std::env::temp_dir().join(format!(
        "molt-merge-reloc-fail-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
    let input_a = tmp_dir.join("a.o");
    let input_b = tmp_dir.join("b.o");
    let output = tmp_dir.join("output.o");
    std::fs::write(&input_a, b"a").expect("write first input object");
    std::fs::write(&input_b, b"b").expect("write second input object");
    let failing_linker = write_failing_relocatable_linker(&tmp_dir);
    let failing_linker_arg = failing_linker.to_string_lossy();

    let err = merge_relocatable_objects(
        &output,
        &[input_a.clone(), input_b.clone()],
        Some(failing_linker_arg.as_ref()),
    )
    .expect_err("merge should fail with failing linker");
    let message = err.to_string();
    assert!(message.contains("relocatable link failed"), "{message}");
    assert!(!output.exists());

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn native_application_object_batches_cleanup_temp_dir_after_merge_failure() {
    let _env_guard = ENV_TEST_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let prior_batch_size = std::env::var("MOLT_BACKEND_BATCH_SIZE").ok();
    let prior_linker = std::env::var("MOLT_LINKER").ok();
    let temp_root = std::env::temp_dir();
    let batch_prefix = format!("molt_batch_{}_", std::process::id());
    let before: std::collections::BTreeSet<_> = std::fs::read_dir(&temp_root)
        .expect("read temp root before")
        .flatten()
        .map(|entry| entry.file_name())
        .filter(|name| name.to_string_lossy().starts_with(&batch_prefix))
        .collect();
    let tmp_dir = temp_root.join(format!(
        "molt-native-app-merge-fail-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
    let failing_linker = write_failing_relocatable_linker(&tmp_dir);
    unsafe {
        std::env::set_var("MOLT_BACKEND_BATCH_SIZE", "1");
        std::env::set_var("MOLT_LINKER", failing_linker.as_os_str());
    }

    let output = tmp_dir.join("output.o");
    let ir = SimpleIR {
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
                execution_context: Default::default(),
            },
        ],
        profile: None,
    };

    let err = compile_native_application_object_to_path(
        ir,
        &output,
        NativeApplicationObjectOptions {
            target_triple: None,
            stdlib_split_enabled: false,
            app_callable_manifest: None,
            log_prefix: "MOLT_BACKEND(test)",
            module_registry: None,
        },
    )
    .expect_err("forced linker failure should propagate");
    let message = err.to_string();
    assert!(message.contains("relocatable link failed"), "{message}");
    assert!(!output.exists(), "failed merge must not publish output");

    let after: std::collections::BTreeSet<_> = std::fs::read_dir(&temp_root)
        .expect("read temp root after")
        .flatten()
        .map(|entry| entry.file_name())
        .filter(|name| name.to_string_lossy().starts_with(&batch_prefix))
        .collect();
    assert_eq!(
        after, before,
        "batch temp dirs must be cleaned after failure"
    );

    match prior_batch_size {
        Some(value) => unsafe { std::env::set_var("MOLT_BACKEND_BATCH_SIZE", value) },
        None => unsafe { std::env::remove_var("MOLT_BACKEND_BATCH_SIZE") },
    }
    match prior_linker {
        Some(value) => unsafe { std::env::set_var("MOLT_LINKER", value) },
        None => unsafe { std::env::remove_var("MOLT_LINKER") },
    }
    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn native_application_object_uses_op_budget_even_when_count_fits() {
    let _env_guard = ENV_TEST_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let prior_batch_size = std::env::var("MOLT_BACKEND_BATCH_SIZE").ok();
    let prior_op_budget = std::env::var("MOLT_BACKEND_BATCH_OP_BUDGET").ok();
    let prior_linker = std::env::var("MOLT_LINKER").ok();
    let tmp_dir = std::env::temp_dir().join(format!(
        "molt-native-app-op-budget-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
    let failing_linker = write_failing_relocatable_linker(&tmp_dir);
    unsafe {
        std::env::set_var("MOLT_BACKEND_BATCH_SIZE", "64");
        std::env::set_var("MOLT_BACKEND_BATCH_OP_BUDGET", "1");
        std::env::set_var("MOLT_LINKER", failing_linker.as_os_str());
    }

    let output = tmp_dir.join("output.o");
    let ir = SimpleIR {
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
                execution_context: Default::default(),
            },
        ],
        profile: None,
    };

    let err = compile_native_application_object_to_path(
        ir,
        &output,
        NativeApplicationObjectOptions {
            target_triple: None,
            stdlib_split_enabled: false,
            app_callable_manifest: None,
            log_prefix: "MOLT_BACKEND(test)",
            module_registry: None,
        },
    )
    .expect_err("op budget must force relocatable batching");
    let message = err.to_string();
    assert!(message.contains("relocatable link failed"), "{message}");
    assert!(!output.exists(), "failed merge must not publish output");

    match prior_batch_size {
        Some(value) => unsafe { std::env::set_var("MOLT_BACKEND_BATCH_SIZE", value) },
        None => unsafe { std::env::remove_var("MOLT_BACKEND_BATCH_SIZE") },
    }
    match prior_op_budget {
        Some(value) => unsafe { std::env::set_var("MOLT_BACKEND_BATCH_OP_BUDGET", value) },
        None => unsafe { std::env::remove_var("MOLT_BACKEND_BATCH_OP_BUDGET") },
    }
    match prior_linker {
        Some(value) => unsafe { std::env::set_var("MOLT_LINKER", value) },
        None => unsafe { std::env::remove_var("MOLT_LINKER") },
    }
    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn resolved_batch_size_and_op_budget_limits_default_and_zero_disable_caps() {
    let _env_guard = ENV_TEST_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let prior_size = std::env::var("MOLT_BACKEND_BATCH_SIZE").ok();
    let prior_ops = std::env::var("MOLT_BACKEND_BATCH_OP_BUDGET").ok();

    unsafe {
        std::env::remove_var("MOLT_BACKEND_BATCH_SIZE");
        std::env::remove_var("MOLT_BACKEND_BATCH_OP_BUDGET");
    }
    assert_eq!(
        resolved_batch_size_limit(DEFAULT_BACKEND_BATCH_SIZE),
        DEFAULT_BACKEND_BATCH_SIZE
    );
    assert_eq!(
        resolved_batch_size_limit(DEFAULT_STDLIB_BATCH_SIZE),
        DEFAULT_STDLIB_BATCH_SIZE
    );
    assert_eq!(
        resolved_batch_op_budget_limit(DEFAULT_BACKEND_BATCH_OP_BUDGET),
        DEFAULT_BACKEND_BATCH_OP_BUDGET
    );

    unsafe {
        std::env::set_var("MOLT_BACKEND_BATCH_SIZE", "0");
        std::env::set_var("MOLT_BACKEND_BATCH_OP_BUDGET", "0");
    }
    assert_eq!(
        resolved_batch_size_limit(DEFAULT_BACKEND_BATCH_SIZE),
        usize::MAX
    );
    assert_eq!(
        resolved_batch_size_limit(DEFAULT_STDLIB_BATCH_SIZE),
        usize::MAX
    );
    assert_eq!(
        resolved_batch_op_budget_limit(DEFAULT_BACKEND_BATCH_OP_BUDGET),
        usize::MAX
    );

    match prior_size {
        Some(value) => unsafe { std::env::set_var("MOLT_BACKEND_BATCH_SIZE", value) },
        None => unsafe { std::env::remove_var("MOLT_BACKEND_BATCH_SIZE") },
    }
    match prior_ops {
        Some(value) => unsafe { std::env::set_var("MOLT_BACKEND_BATCH_OP_BUDGET", value) },
        None => unsafe { std::env::remove_var("MOLT_BACKEND_BATCH_OP_BUDGET") },
    }
}

#[test]
fn batch_external_function_names_excludes_current_batch_symbols() {
    let all_names = std::collections::BTreeSet::from([
        "molt_main".to_string(),
        "demo__module".to_string(),
        "molt_isolate_bootstrap".to_string(),
        "molt_isolate_import".to_string(),
    ]);
    let batch_funcs = vec![
        FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![OpIR {
                kind: "ret_void".to_string(),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
            execution_context: Default::default(),
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
            execution_context: Default::default(),
        },
    ];

    let external_names = batch_external_function_names(&all_names, &batch_funcs);

    assert_eq!(
        external_names,
        std::collections::BTreeSet::from([
            "molt_isolate_bootstrap".to_string(),
            "molt_isolate_import".to_string(),
        ])
    );
    assert!(!external_names.contains("molt_main"));
    assert!(!external_names.contains("demo__module"));
}

#[test]
fn native_batch_ir_carries_referenced_inherited_execution_context_contracts() {
    let inherited = FunctionIR {
        name: "demo__molt_module_chunk_1".to_string(),
        params: vec!["module".to_string()],
        ops: vec![OpIR {
            kind: "ret_void".to_string(),
            ..OpIR::default()
        }],
        param_types: Some(vec!["i64".to_string()]),
        source_file: Some("demo.py".to_string()),
        is_extern: false,
        execution_context: molt_backend::ir::ExecutionContextPolicy::Inherited,
    };
    let local = FunctionIR {
        name: "molt_init_demo".to_string(),
        params: vec![],
        ops: vec![
            OpIR {
                kind: "trace_enter_slot".to_string(),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "call_internal".to_string(),
                s_value: Some(inherited.name.clone()),
                passes_execution_context: true,
                ..OpIR::default()
            },
            OpIR {
                kind: "trace_exit".to_string(),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret_void".to_string(),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: Some("demo.py".to_string()),
        is_extern: false,
        execution_context: molt_backend::ir::ExecutionContextPolicy::Local,
    };
    let declarations = inherited_function_declarations(&[local.clone(), inherited.clone()]);
    let mut batch_functions = vec![local];

    append_referenced_inherited_declarations(&mut batch_functions, &declarations);

    assert_eq!(batch_functions.len(), 2);
    let declaration = &batch_functions[1];
    assert_eq!(declaration.name, inherited.name);
    assert_eq!(declaration.params, inherited.params);
    assert_eq!(declaration.param_types, inherited.param_types);
    assert_eq!(declaration.source_file, inherited.source_file);
    assert!(declaration.is_extern);
    assert!(declaration.ops.is_empty());
    assert_eq!(
        declaration.execution_context,
        molt_backend::ir::ExecutionContextPolicy::Inherited
    );

    let batch_ir = SimpleIR {
        functions: batch_functions,
        profile: None,
    };
    let encoded = serde_json::to_vec(&batch_ir).expect("serialize self-contained batch IR");
    let decoded: SimpleIR =
        serde_json::from_slice(&encoded).expect("deserialize and validate batch IR");
    assert_eq!(decoded.functions.len(), 2);
}
