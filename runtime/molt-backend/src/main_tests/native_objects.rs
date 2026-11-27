use super::*;

#[test]
fn application_body_edit_reuses_independent_objects_and_matches_fresh_archive() {
    use crate::NativeObjectCache;
    let _env = TestEnvGuard::clear(&["MOLT_BACKEND_BATCH_SIZE", "MOLT_BACKEND_BATCH_OP_BUDGET"]);
    let directory = native_artifact_test_directory("object-edit-reuse");
    let _scope = calling_program_scope(&directory, serde_json::json!({}));
    let options = |root: &std::path::Path, program| {
        let manifest = serde_json::json!({
            "schema":"stdlib-manifest-v2-archive", "artifact_kind":"archive",
            "cache_key":program, "cache_variant":"profile=test", "compiler_fingerprint":"test",
            "target_triple":null
        })
        .to_string();
        let mut options = native_artifact_test_options(NativeArtifactKind::Archive);
        options.object_cache = NativeObjectCache::for_archive(
            &root.join("stdlib.a"),
            Some(program),
            Some(&manifest),
            None,
        )
        .unwrap();
        options
    };
    let mut source = native_artifact_test_ir();
    source.functions[0].return_abi = molt_ir::FunctionReturnAbi::Void;
    source.functions[0].source_file = Some("a_entry.py".into());
    source.functions[1].source_file = Some("b_helper.py".into());
    source.functions[1].ops = vec![
        OpIR {
            value: Some(41),
            out: Some("answer".into()),
            ..OpIR::new(molt_ir::OpKind::Const)
        },
        OpIR {
            args: Some(vec!["answer".into()]),
            ..OpIR::new(molt_ir::OpKind::Ret)
        },
    ];
    source.functions[0].ops[0].out = Some("result".into());
    let run = |ir: SimpleIR, name: &str, root: &std::path::Path, program| {
        let output = directory.join(name);
        let result =
            compile_native_application_artifact_to_path(ir, &output, options(root, program))
                .unwrap();
        (result, std::fs::read(output).unwrap())
    };
    let (cold, cold_bytes) = run(source.clone(), "cold.a", &directory, "program");
    assert_eq!(
        cold.batch_count, 2,
        "source units apply even below the old single-batch limit"
    );
    let (warm, warm_bytes) = run(source.clone(), "warm.a", &directory, "changed-stdlib");
    assert_eq!(cold_bytes, warm_bytes);
    assert_eq!(
        cold.workers, warm.workers,
        "a hit retains the actual original worker emission"
    );
    source.functions[1].ops[0].value = Some(42);
    let (edited, edited_bytes) = run(source.clone(), "edited.a", &directory, "changed-stdlib");
    let old = cold.workers.as_ref().unwrap();
    let new = edited.workers.as_ref().unwrap();
    assert_eq!(
        old[0], new[0],
        "unchanged entry object retains its emission"
    );
    assert_ne!(
        old[1].input, new[1].input,
        "edited source must compile a new input"
    );
    let (_, fresh_bytes) = run(source, "fresh.a", &directory.join("fresh"), "program");
    assert_eq!(
        edited_bytes, fresh_bytes,
        "cached edit must equal fresh compilation"
    );
    let (metadata, metadata_bytes) = run(
        SimpleIR {
            functions: Vec::new(),
            profile: None,
        },
        "metadata.a",
        &directory,
        "program",
    );
    assert_eq!(
        metadata.batch_count, 1,
        "metadata-only archives retain one owner object"
    );
    let archive = object::read::archive::ArchiveFile::parse(metadata_bytes.as_slice()).unwrap();
    assert_eq!(archive.members().count(), 1);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn native_application_archive_publication_failure_cleans_batches() {
    let _env = TestEnvGuard::clear(&["MOLT_BACKEND_BATCH_SIZE", "MOLT_BACKEND_BATCH_OP_BUDGET"]);
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
    let _scope = calling_program_scope(
        &directory,
        serde_json::json!({"MOLT_BACKEND_BATCH_SIZE": "1"}),
    );
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
    let directory = native_artifact_test_directory("output-kinds");
    let _scope = calling_program_scope(
        &directory,
        serde_json::json!({"MOLT_BACKEND_BATCH_SIZE": "64", "MOLT_BACKEND_BATCH_OP_BUDGET": "1"}),
    );
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
            .contains("native object output requires the complete graph")
    );
}

#[test]
fn native_application_refuses_unbound_program_slots_before_batching() {
    let _env = TestEnvGuard::clear(&["MOLT_BACKEND_BATCH_SIZE"]);
    unsafe { std::env::set_var("MOLT_BACKEND_BATCH_SIZE", "1") };
    let directory = native_artifact_test_directory("unbound-program-slots");
    for kind in [molt_ir::OpKind::ModuleSlot, molt_ir::OpKind::TraceEnterSlot] {
        let mut ir = native_artifact_test_ir();
        ir.functions[1].ops.insert(
            0,
            OpIR {
                s_value: Some("helper".into()),
                out: (kind == molt_ir::OpKind::ModuleSlot).then(|| "slot".into()),
                ..OpIR::new(kind)
            },
        );
        let output = directory.join(format!("{kind}.a"));
        let error = compile_native_application_artifact_to_path(
            ir,
            &output,
            native_artifact_test_options(NativeArtifactKind::Archive),
        )
        .expect_err("a later batch must not bypass full-program admission");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains(kind.as_str()), "{error}");
        assert!(!output.exists());
    }
    std::fs::remove_dir_all(directory).unwrap();
}

fn native_artifact_test_options(
    kind: NativeArtifactKind,
) -> NativeApplicationArtifactOptions<'static> {
    NativeApplicationArtifactOptions {
        object_cache: None,
        code_slots: Default::default(),
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
        module: None,
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: name.to_string(),
        params: vec![],
        ops,
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        frame_locals_exposed: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    };
    SimpleIR {
        functions: vec![
            function(
                "molt_main",
                vec![
                    OpIR {
                        s_value: Some("helper".to_string()),
                        ..OpIR::new(molt_ir::OpKind::Call)
                    },
                    OpIR::new(molt_ir::OpKind::RetVoid),
                ],
            ),
            function("helper", vec![OpIR::new(molt_ir::OpKind::RetVoid)]),
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
    let batch_funcs = vec![
        FunctionIR {
            module: None,
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![OpIR::new(molt_ir::OpKind::RetVoid)],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            frame_locals_exposed: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        },
        FunctionIR {
            module: None,
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "demo__module".to_string(),
            params: vec![],
            ops: vec![OpIR::new(molt_ir::OpKind::RetVoid)],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            frame_locals_exposed: false,
            parameter_custody: Vec::new(),
            execution_context: Default::default(),
        },
    ];

    let mut batch_funcs = batch_funcs;
    batch_funcs.push(FunctionIR {
        name: "referenced".into(),
        is_extern: true,
        ..Default::default()
    });
    let external_names = batch_external_function_names(&batch_funcs);

    assert_eq!(
        external_names,
        std::collections::BTreeSet::from(["referenced".to_string(),])
    );
    assert!(!external_names.contains("molt_main"));
    assert!(!external_names.contains("demo__module"));
}

#[test]
fn native_batch_ir_carries_referenced_external_execution_context_contracts() {
    let inherited = FunctionIR {
        module: None,
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: "demo__molt_module_chunk_1".to_string(),
        params: Vec::new(),
        ops: vec![OpIR::new(molt_ir::OpKind::RetVoid)],
        param_types: Some(Vec::new()),
        source_file: Some("demo.py".to_string()),
        is_extern: false,
        codegen_partition: false,
        frame_locals_exposed: false,
        parameter_custody: Vec::new(),
        execution_context: molt_backend::ir::ExecutionContextPolicy::Inherited,
    };
    let local = FunctionIR {
        module: None,
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: "molt_init_demo".to_string(),
        params: vec![],
        ops: vec![
            OpIR {
                s_value: Some("molt_init_demo".into()),
                ..OpIR::new(molt_ir::OpKind::TraceEnterSlot)
            },
            OpIR {
                s_value: Some(inherited.name.clone()),
                passes_execution_context: true,
                ..OpIR::new(molt_ir::OpKind::CallInternal)
            },
            OpIR::new(molt_ir::OpKind::TraceExit),
            OpIR::new(molt_ir::OpKind::RetVoid),
        ],
        param_types: None,
        source_file: Some("demo.py".to_string()),
        is_extern: false,
        codegen_partition: false,
        frame_locals_exposed: false,
        parameter_custody: Vec::new(),
        execution_context: molt_backend::ir::ExecutionContextPolicy::Local,
    };
    let string_only = FunctionIR {
        module: None,
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: "string_collision".to_string(),
        params: Vec::new(),
        ops: vec![OpIR {
            s_value: Some(inherited.name.clone()),
            out: Some("text".to_string()),
            ..OpIR::new(molt_ir::OpKind::ConstStr)
        }],
        param_types: None,
        source_file: Some("demo.py".to_string()),
        is_extern: false,
        codegen_partition: false,
        frame_locals_exposed: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    };
    let method_collision = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: "method_collision".to_string(),
        ops: vec![OpIR {
            s_value: Some(inherited.name.clone()),
            ..OpIR::new(molt_ir::OpKind::CallMethod)
        }],
        ..FunctionIR::default()
    };
    let warm_user = FunctionIR {
        module: None,
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: "warm_user".to_string(),
        params: Vec::new(),
        ops: vec![OpIR {
            s_value: Some(local.name.clone()),
            ..OpIR::new(molt_ir::OpKind::CallInternal)
        }],
        param_types: None,
        source_file: Some("warm.py".to_string()),
        is_extern: false,
        codegen_partition: false,
        frame_locals_exposed: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    };
    // One decoded source document holds every body (HF-847 slice 1); each
    // batch is projected from it.
    let mut source = vec![
        local.clone(),
        inherited.clone(),
        string_only,
        method_collision,
        warm_user,
    ];
    molt_ir::function_table::resolve_function_references(&mut source).unwrap();
    let declarations = external_function_declarations(&source);
    let local = source[0].clone();
    let inherited = source[1].clone();

    let mut string_only_reference = vec![source[2].clone()];
    append_referenced_external_declarations(&mut string_only_reference, &declarations);
    assert_eq!(
        string_only_reference.len(),
        1,
        "plain string payloads must not pull external declarations into a batch"
    );

    let mut dynamic_method_collision = vec![source[3].clone()];
    append_referenced_external_declarations(&mut dynamic_method_collision, &declarations);
    assert_eq!(
        dynamic_method_collision.len(),
        1,
        "dynamic method names must not be mistaken for static external symbols"
    );

    let mut local_external_reference = vec![source[4].clone()];
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
    assert!(declaration.ops.is_empty());
    assert_eq!(declaration.return_abi, molt_ir::FunctionReturnAbi::Void);
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
