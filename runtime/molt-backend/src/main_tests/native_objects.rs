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
            codegen_partition: false,
            execution_context: Default::default(),
        },
        FunctionIR {
            name: "b".to_string(),
            params: vec![],
            ops: vec![Default::default(); 90],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: Default::default(),
        },
        FunctionIR {
            name: "c".to_string(),
            params: vec![],
            ops: vec![Default::default(); 10],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
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
            codegen_partition: false,
            execution_context: Default::default(),
        })
        .collect();

    let batches = partition_functions_for_batches(funcs, 2, 1000);
    let sizes: Vec<usize> = batches.into_iter().map(|batch| batch.len()).collect();

    assert_eq!(sizes, vec![2, 2, 1]);
}

#[test]
fn native_application_archive_publication_failure_cleans_batches() {
    let _env = TestEnvGuard::clear(&["MOLT_BACKEND_BATCH_SIZE", "MOLT_BACKEND_BATCH_OP_BUDGET"]);
    unsafe { std::env::set_var("MOLT_BACKEND_BATCH_SIZE", "1") };
    let temp_root = std::env::temp_dir();
    let prefix = format!("molt_batch_{}_", std::process::id());
    let batch_dirs = || {
        std::fs::read_dir(&temp_root)
            .expect("read temp root")
            .map(|entry| entry.expect("read directory entry").file_name())
            .filter(|name| name.to_string_lossy().starts_with(&prefix))
            .collect::<std::collections::BTreeSet<_>>()
    };
    let before = batch_dirs();
    let directory = native_artifact_test_directory("publication-failure");
    let output = directory.join("blocked.a");
    std::fs::create_dir(&output).expect("create blocking directory");
    let error = compile_native_application_artifact_to_path(
        native_artifact_test_ir(),
        &output,
        native_artifact_test_options(NativeArtifactKind::Archive),
    )
    .expect_err("archive publication into a directory must fail");
    assert!(
        !error.to_string().is_empty(),
        "publication failure must retain diagnostics"
    );
    assert!(
        output.is_dir(),
        "failure must preserve the previous destination"
    );
    assert_eq!(
        batch_dirs(),
        before,
        "failure must not strand batch directories"
    );
    std::fs::remove_dir_all(directory).expect("clean test directory");
}

#[test]
fn native_archive_honors_batch_budget_while_object_is_one_relocatable() {
    let _env = TestEnvGuard::clear(&["MOLT_BACKEND_BATCH_SIZE", "MOLT_BACKEND_BATCH_OP_BUDGET"]);
    unsafe {
        std::env::set_var("MOLT_BACKEND_BATCH_SIZE", "64");
        std::env::set_var("MOLT_BACKEND_BATCH_OP_BUDGET", "1");
    }
    let directory = native_artifact_test_directory("output-kinds");
    for kind in [NativeArtifactKind::Object, NativeArtifactKind::Archive] {
        let output = directory.join(if kind == NativeArtifactKind::Object {
            "output.o"
        } else {
            "output.a"
        });
        let result = compile_native_application_artifact_to_path(
            native_artifact_test_ir(),
            &output,
            native_artifact_test_options(kind),
        )
        .expect("compile typed native artifact");
        let bytes = std::fs::read(&output).expect("read native artifact");
        if kind == NativeArtifactKind::Object {
            use object::Object;
            assert_eq!(
                result.batch_count, 1,
                "explicit object output cannot cross object modules"
            );
            let object = object::File::parse(bytes.as_slice()).expect("actual relocatable object");
            assert_eq!(object.kind(), object::ObjectKind::Relocatable);
        } else {
            assert_eq!(result.batch_count, 2, "archive output obeys the op budget");
            let archive = object::read::archive::ArchiveFile::parse(bytes.as_slice())
                .expect("actual archive");
            assert_eq!(archive.members().count(), 2);
        }
    }
    std::fs::remove_dir_all(directory).expect("clean test directory");
}

#[test]
fn native_object_refuses_split_graph_before_emission() {
    let mut options = native_artifact_test_options(NativeArtifactKind::Object);
    options.stdlib_split_enabled = true;
    let error = compile_native_application_artifact_to_path(
        native_artifact_test_ir(),
        std::path::Path::new("must-not-be-emitted.o"),
        options,
    )
    .expect_err("object mode cannot externalize the shared stdlib");
    assert!(
        error
            .to_string()
            .contains("cannot omit the shared stdlib graph")
    );
}

fn native_artifact_test_options(
    kind: NativeArtifactKind,
) -> NativeApplicationArtifactOptions<'static> {
    NativeApplicationArtifactOptions {
        native_output_kind: kind,
        target_triple: None,
        stdlib_split_enabled: false,
        app_callable_manifest: None,
        log_prefix: "MOLT_BACKEND(test)",
        module_registry: None,
        module_context: None,
    }
}

fn native_artifact_test_directory(label: &str) -> std::path::PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "molt-native-artifact-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&directory).expect("create test directory");
    directory
}

fn native_artifact_test_ir() -> SimpleIR {
    let function = |name: &str, ops| FunctionIR {
        name: name.to_string(),
        params: vec![],
        ops,
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        execution_context: Default::default(),
    };
    SimpleIR {
        functions: vec![
            function(
                "molt_main",
                vec![
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
            ),
            function(
                "helper",
                vec![OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                }],
            ),
        ],
        profile: None,
    }
}

#[test]
fn resolved_batch_size_and_op_budget_limits_default_and_zero_disable_caps() {
    let _env_guard =
        TestEnvGuard::clear(&["MOLT_BACKEND_BATCH_SIZE", "MOLT_BACKEND_BATCH_OP_BUDGET"]);
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
            codegen_partition: false,
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
            codegen_partition: false,
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
fn native_batch_ir_carries_referenced_external_execution_context_contracts() {
    let inherited = FunctionIR {
        name: "demo__molt_module_chunk_1".to_string(),
        params: Vec::new(),
        ops: vec![OpIR {
            kind: "ret_void".to_string(),
            ..OpIR::default()
        }],
        param_types: Some(Vec::new()),
        source_file: Some("demo.py".to_string()),
        is_extern: false,
        codegen_partition: false,
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
        codegen_partition: false,
        execution_context: molt_backend::ir::ExecutionContextPolicy::Local,
    };
    let declarations = external_function_declarations(&[local.clone(), inherited.clone()]);

    let mut string_only_reference = vec![FunctionIR {
        name: "string_collision".to_string(),
        params: Vec::new(),
        ops: vec![OpIR {
            kind: "const_str".to_string(),
            s_value: Some(inherited.name.clone()),
            out: Some("text".to_string()),
            ..OpIR::default()
        }],
        param_types: None,
        source_file: Some("demo.py".to_string()),
        is_extern: false,
        codegen_partition: false,
        execution_context: Default::default(),
    }];
    append_referenced_external_declarations(&mut string_only_reference, &declarations);
    assert_eq!(
        string_only_reference.len(),
        1,
        "plain string payloads must not pull external declarations into a batch"
    );

    let mut dynamic_method_collision = vec![FunctionIR {
        name: "method_collision".to_string(),
        ops: vec![OpIR {
            kind: "call_method".to_string(),
            s_value: Some(inherited.name.clone()),
            ..OpIR::default()
        }],
        ..FunctionIR::default()
    }];
    append_referenced_external_declarations(&mut dynamic_method_collision, &declarations);
    assert_eq!(
        dynamic_method_collision.len(),
        1,
        "dynamic method names must not be mistaken for static external symbols"
    );

    let mut local_external_reference = vec![FunctionIR {
        name: "warm_user".to_string(),
        params: Vec::new(),
        ops: vec![OpIR {
            kind: "call_internal".to_string(),
            s_value: Some(local.name.clone()),
            ..OpIR::default()
        }],
        param_types: None,
        source_file: Some("warm.py".to_string()),
        is_extern: false,
        codegen_partition: false,
        execution_context: Default::default(),
    }];
    append_referenced_external_declarations(&mut local_external_reference, &declarations);
    assert_eq!(local_external_reference.len(), 2);
    let local_declaration = &local_external_reference[1];
    assert_eq!(local_declaration.name, local.name);
    assert!(local_declaration.is_extern);
    assert_eq!(
        local_declaration.execution_context,
        molt_backend::ir::ExecutionContextPolicy::Local
    );

    let mut batch_functions = vec![local];

    append_referenced_external_declarations(&mut batch_functions, &declarations);

    assert_eq!(batch_functions.len(), 2);
    let declaration = &batch_functions[1];
    assert_eq!(declaration.name, inherited.name);
    assert_eq!(declaration.params, inherited.params);
    assert_eq!(declaration.param_types, inherited.param_types);
    assert_eq!(declaration.source_file, inherited.source_file);
    assert!(declaration.is_extern);
    assert_eq!(declaration.ops.len(), 1);
    assert_eq!(declaration.ops[0].kind, "ret_void");
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
