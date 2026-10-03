use super::*;

#[test]
#[should_panic(expected = "import signature mismatch for molt_test_import")]
fn import_func_ref_validates_signature_before_local_reuse() {
    let mut backend = SimpleBackend::new();
    let mut builder_ctx = FunctionBuilderContext::new();
    let mut builder = FunctionBuilder::new(&mut backend.ctx.func, &mut builder_ctx);
    let entry_block = builder.create_block();
    builder.switch_to_block(entry_block);
    builder.seal_block(entry_block);

    let mut import_refs = BTreeMap::new();
    import_func_ref(
        &mut backend.module,
        &mut backend.import_ids,
        &mut builder,
        &mut import_refs,
        "molt_test_import",
        &[types::I64],
        &[types::I64],
    );
    import_func_ref(
        &mut backend.module,
        &mut backend.import_ids,
        &mut builder,
        &mut import_refs,
        "molt_test_import",
        &[types::I64, types::I64],
        &[types::I64],
    );
}

#[test]
fn preanalysis_keeps_mixed_join_store_targets_boxed() {
    let func = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: "mixed_join".to_string(),
        params: vec!["callable".to_string(), "args".to_string()],
        ops: vec![
            OpIR {
                kind: "call_indirect".to_string(),
                args: Some(vec!["callable".to_string(), "args".to_string()]),
                out: Some("dynamic".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some("_bb_arg0".to_string()),
                args: Some(vec!["dynamic".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_bool".to_string(),
                out: Some("fallback".to_string()),
                value: Some(0),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some("_bb_arg0".to_string()),
                args: Some(vec!["fallback".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "load_var".to_string(),
                var: Some("_bb_arg0".to_string()),
                out: Some("joined".to_string()),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    };

    let plan = native_representation_plan_for_test(&func);

    for name in ["_bb_arg0", "joined"] {
        assert!(
            plan.name_scalar_kind(name).is_none(),
            "mixed dynamic/scalar join target {name} must stay boxed",
        );
    }
}

#[test]
fn preanalysis_keeps_unbounded_integer_family_out_of_float_lane() {
    let func = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: "integer_family_chain".to_string(),
        params: vec!["x".to_string(), "seed".to_string()],
        ops: vec![
            OpIR {
                kind: "const".to_string(),
                value: Some(374761393),
                out: Some("_v0".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "mul".to_string(),
                args: Some(vec!["x".to_string(), "_v0".to_string()]),
                out: Some("_v1".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "bit_xor".to_string(),
                args: Some(vec!["seed".to_string(), "_v1".to_string()]),
                out: Some("_v2".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".to_string(),
                value: Some(13),
                out: Some("_v3".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "rshift".to_string(),
                args: Some(vec!["_v2".to_string(), "_v3".to_string()]),
                out: Some("_v4".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "bit_xor".to_string(),
                args: Some(vec!["_v2".to_string(), "_v4".to_string()]),
                out: Some("_v5".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".to_string(),
                value: Some(3266489917),
                out: Some("_v6".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "mul".to_string(),
                args: Some(vec!["_v5".to_string(), "_v6".to_string()]),
                out: Some("_v7".to_string()),
                ..OpIR::default()
            },
        ],
        param_types: Some(vec!["int".to_string(), "int".to_string()]),
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    };

    let plan = native_representation_plan_for_test(&func);

    assert!(plan.integer_family_names().contains("_v7"));
    assert!(plan.name_has_scalar_kind("_v7", ScalarKind::Int));
    assert!(!plan.is_raw_int_carrier_name("_v7"));
    assert!(!plan.name_has_scalar_kind("_v7", ScalarKind::Float));
}

#[test]
fn preanalysis_fuses_control_flow_state_and_cleanup_metadata() {
    let func = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "molt_main".to_string(),
        params: vec!["arg".to_string()],
        ops: vec![
            OpIR {
                kind: "state_switch".into(),
                state_targets: Some(vec![]),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_str".to_string(),
                out: Some("msg".to_string()),
                s_value: Some("hi".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "if".to_string(),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".to_string(),
                value: Some(42),
                ..OpIR::default()
            },
            OpIR {
                kind: "else".to_string(),
                ..OpIR::default()
            },
            OpIR {
                kind: "end_if".to_string(),
                ..OpIR::default()
            },
            OpIR {
                kind: "phi".to_string(),
                out: Some("joined".to_string()),
                args: Some(vec!["msg".to_string(), "msg".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "state_set".to_string(),
                value: Some(7),
                ..OpIR::default()
            },
            OpIR {
                kind: "state_label".to_string(),
                value: Some(42),
                ..OpIR::default()
            },
            OpIR {
                kind: "copy".to_string(),
                args: Some(vec!["msg".to_string()]),
                out: Some("out".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".to_string(),
                args: Some(vec!["out".to_string()]),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    };

    let analysis = preanalyze_for_test(&func);

    assert!(analysis.returns_value);
    assert!(analysis.stateful);
    assert_eq!(analysis.if_to_end_if.get(&2), Some(&5));
    assert_eq!(analysis.if_to_else.get(&2), Some(&4));
    assert_eq!(analysis.else_to_end_if.get(&4), Some(&5));
    assert_eq!(analysis.label_ids, vec![42]);
    assert!(analysis.state_label_ids.contains(&42));
    assert!(!analysis.state_label_ids.contains(&7));
    assert_eq!(
        analysis.resume_targets,
        Some(BTreeMap::new()),
        "a running state and a label cannot invent a resume case"
    );
    assert_eq!(analysis.function_exception_label_id, Some(42));
    assert!(analysis.var_names.contains(&"msg_ptr".to_string()));
    assert!(analysis.var_names.contains(&"msg_len".to_string()));
    // After alias analysis, "msg" and "out" share the same alias root
    // (copy propagation makes "out" an alias of "msg"), so both last_use
    // values are extended to the maximum of the group (op 10, the ret op).
    assert_eq!(analysis.last_use.get("msg"), Some(&10));
    assert_eq!(analysis.last_use.get("out"), Some(&10));
}

#[test]
fn preanalysis_uses_frozen_return_abi_instead_of_surviving_payloads() {
    let value_ret = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "value_ret".to_string(),
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
    };
    let void_ret = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: "void_ret".to_string(),
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
    };

    assert!(
        preanalyze_for_test(&value_ret).returns_value,
        "a surviving empty return must preserve the value ABI"
    );
    assert!(
        !preanalyze_for_test(&void_ret).returns_value,
        "the same payload shape must also support an authored void ABI"
    );
    let mut unreachable = value_ret;
    unreachable.ops = vec![OpIR {
        kind: "unreachable".into(),
        ..OpIR::default()
    }];
    assert!(preanalyze_for_test(&unreachable).returns_value);
}

#[test]
fn preanalysis_uses_only_declared_resume_cases() {
    let func = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "stateful_ready_continuations".to_string(),
        params: vec!["self".to_string()],
        ops: vec![
            OpIR {
                kind: "state_switch".into(),
                state_targets: Some(vec![(216, 216)]),
                ..OpIR::default()
            },
            OpIR {
                kind: "state_label".to_string(),
                value: Some(216),
                ..OpIR::default()
            },
            OpIR {
                kind: "state_set".to_string(),
                value: Some(217),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    };

    let analysis = preanalyze_for_test(&func);

    assert_eq!(analysis.resume_targets, Some(BTreeMap::from([(216, 216)])));
    assert!(
        !analysis.resume_targets.as_ref().unwrap().contains_key(&217),
        "a running state is not a resume case"
    );
}

#[test]
fn preanalysis_keeps_regular_labels_distinct_from_resume_state_collisions() {
    let func = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "resume_label_collision".to_string(),
        params: vec!["self".to_string()],
        ops: vec![
            OpIR {
                kind: "state_switch".into(),
                state_targets: Some(vec![(12, 12)]),
                ..OpIR::default()
            },
            OpIR {
                kind: "state_label".to_string(),
                value: Some(12),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".to_string(),
                out: Some("pending_state".to_string()),
                value: Some(12),
                ..OpIR::default()
            },
            OpIR {
                kind: "state_set".to_string(),
                value: Some(13),
                ..OpIR::default()
            },
            OpIR {
                kind: "label".to_string(),
                value: Some(13),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    };

    let analysis = preanalyze_for_test(&func);

    assert_eq!(analysis.label_ids, vec![12, 13]);
    assert_eq!(analysis.resume_targets, Some(BTreeMap::from([(12, 12)])));
    assert!(analysis.state_label_ids.contains(&12));
    assert!(!analysis.state_label_ids.contains(&13));
}

#[test]
fn preanalysis_maps_saved_states_to_plain_labels() {
    let func = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "pending_plain_label".to_string(),
        params: vec!["self".to_string()],
        ops: vec![
            OpIR {
                kind: "state_switch".into(),
                state_targets: Some(vec![(99, 12)]),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".to_string(),
                out: Some("pending_state".to_string()),
                value: Some(12),
                ..OpIR::default()
            },
            OpIR {
                kind: "state_set".to_string(),
                value: Some(13),
                ..OpIR::default()
            },
            OpIR {
                kind: "label".to_string(),
                value: Some(12),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    };

    let analysis = preanalyze_for_test(&func);

    assert_eq!(analysis.label_ids, vec![12]);
    assert_eq!(analysis.resume_targets, Some(BTreeMap::from([(99, 12)])));
    assert!(
        !analysis.state_label_ids.contains(&12),
        "a plain label can be the explicit resume target"
    );
}
