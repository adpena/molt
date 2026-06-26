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
    };

    let plan = ScalarRepresentationPlan::for_function_ir(&func);

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
    };

    let plan = ScalarRepresentationPlan::for_function_ir(&func);

    assert!(plan.integer_family_names().contains("_v7"));
    assert!(!plan.name_has_scalar_kind("_v7", ScalarKind::Int));
    assert!(!plan.name_has_scalar_kind("_v7", ScalarKind::Float));
}

#[test]
fn preanalysis_fuses_control_flow_state_and_cleanup_metadata() {
    let func = FunctionIR {
        name: "molt_main".to_string(),
        params: vec!["arg".to_string()],
        ops: vec![
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
                kind: "state_yield".to_string(),
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
                var: Some("out".to_string()),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
    };

    let analysis = preanalyze_for_test(&func, &BTreeMap::new());

    assert!(analysis.has_ret);
    assert!(analysis.stateful);
    assert_eq!(analysis.if_to_end_if.get(&1), Some(&4));
    assert_eq!(analysis.if_to_else.get(&1), Some(&3));
    assert_eq!(analysis.else_to_end_if.get(&3), Some(&4));
    assert_eq!(analysis.state_ids, vec![7, 42]);
    assert_eq!(analysis.label_ids, vec![42]);
    assert!(analysis.state_label_ids.contains(&42));
    assert!(!analysis.state_label_ids.contains(&7));
    assert!(analysis.shared_resume_label_ids.contains(&42));
    assert!(!analysis.shared_resume_label_ids.contains(&7));
    assert!(analysis.resume_states.contains(&7));
    assert!(analysis.resume_states.contains(&42));
    assert_eq!(analysis.function_exception_label_id, Some(42));
    assert!(analysis.var_names.contains(&"msg_ptr".to_string()));
    assert!(analysis.var_names.contains(&"msg_len".to_string()));
    // After alias analysis, "msg" and "out" share the same alias root
    // (copy propagation makes "out" an alias of "msg"), so both last_use
    // values are extended to the maximum of the group (op 9, the ret op).
    assert_eq!(analysis.last_use.get("msg"), Some(&9));
    assert_eq!(analysis.last_use.get("out"), Some(&9));
}

#[test]
fn preanalysis_distinguishes_ret_from_ret_void() {
    let value_ret = FunctionIR {
        name: "value_ret".to_string(),
        params: vec![],
        ops: vec![OpIR {
            kind: "ret".to_string(),
            var: Some("out".to_string()),
            ..OpIR::default()
        }],
        param_types: None,
        source_file: None,
        is_extern: false,
    };
    let void_ret = FunctionIR {
        name: "void_ret".to_string(),
        params: vec![],
        ops: vec![OpIR {
            kind: "ret_void".to_string(),
            ..OpIR::default()
        }],
        param_types: None,
        source_file: None,
        is_extern: false,
    };

    assert!(
        preanalyze_for_test(&value_ret, &BTreeMap::new()).has_ret,
        "`ret` should mark the function as value-returning"
    );
    assert!(
        !preanalyze_for_test(&void_ret, &BTreeMap::new()).has_ret,
        "`ret_void` must not mark the function as value-returning"
    );
}

#[test]
fn preanalysis_marks_every_persisted_coroutine_state_resumable() {
    let func = FunctionIR {
        name: "stateful_ready_continuations".to_string(),
        params: vec!["self".to_string()],
        ops: vec![
            OpIR {
                kind: "state_label".to_string(),
                value: Some(216),
                ..OpIR::default()
            },
            OpIR {
                kind: "state_transition".to_string(),
                args: Some(vec![
                    "future".to_string(),
                    "await_slot".to_string(),
                    "pending_state".to_string(),
                ]),
                value: Some(217),
                ..OpIR::default()
            },
            OpIR {
                kind: "chan_send_yield".to_string(),
                args: Some(vec![
                    "chan".to_string(),
                    "value".to_string(),
                    "pending_state".to_string(),
                ]),
                value: Some(301),
                ..OpIR::default()
            },
            OpIR {
                kind: "chan_recv_yield".to_string(),
                args: Some(vec!["chan".to_string(), "pending_state".to_string()]),
                value: Some(302),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
    };

    let analysis = preanalyze_for_test(&func, &BTreeMap::new());

    assert!(
        analysis.resume_states.contains(&216),
        "textual state labels remain dispatchable resume states",
    );
    assert!(
        analysis.resume_states.contains(&217),
        "state_transition ready continuations are stored in object state and must dispatch",
    );
    assert!(
        analysis.resume_states.contains(&301),
        "channel send ready continuations are stored in object state and must dispatch",
    );
    assert!(
        analysis.resume_states.contains(&302),
        "channel receive ready continuations are stored in object state and must dispatch",
    );
}

#[test]
fn preanalysis_keeps_regular_labels_distinct_from_resume_state_collisions() {
    let func = FunctionIR {
        name: "resume_label_collision".to_string(),
        params: vec!["self".to_string()],
        ops: vec![
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
                kind: "state_transition".to_string(),
                args: Some(vec![
                    "future".to_string(),
                    "await_slot".to_string(),
                    "pending_state".to_string(),
                ]),
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
    };

    let analysis = preanalyze_for_test(&func, &BTreeMap::new());

    assert_eq!(analysis.label_ids, vec![12, 13]);
    assert!(analysis.resume_states.contains(&12));
    assert!(analysis.resume_states.contains(&13));
    assert!(analysis.state_label_ids.contains(&12));
    assert!(analysis.shared_resume_label_ids.contains(&12));
    assert!(
        !analysis.state_label_ids.contains(&13),
        "a plain label with the same numeric id as a ready continuation must not share its resume block",
    );
    assert!(
        !analysis.shared_resume_label_ids.contains(&13),
        "a plain label collision is not a persisted pending label and must stay separate",
    );
}

#[test]
fn preanalysis_marks_pending_plain_labels_as_shared_resume_entries() {
    let func = FunctionIR {
        name: "pending_plain_label".to_string(),
        params: vec!["self".to_string()],
        ops: vec![
            OpIR {
                kind: "const".to_string(),
                out: Some("pending_state".to_string()),
                value: Some(12),
                ..OpIR::default()
            },
            OpIR {
                kind: "state_transition".to_string(),
                args: Some(vec![
                    "future".to_string(),
                    "await_slot".to_string(),
                    "pending_state".to_string(),
                ]),
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
    };

    let analysis = preanalyze_for_test(&func, &BTreeMap::new());

    assert_eq!(analysis.label_ids, vec![12]);
    assert!(analysis.resume_states.contains(&12));
    assert!(analysis.resume_states.contains(&13));
    assert!(!analysis.state_label_ids.contains(&12));
    assert!(analysis.shared_resume_label_ids.contains(&12));
    assert!(
        !analysis.shared_resume_label_ids.contains(&13),
        "ready-continuation states use dedicated resume blocks unless a textual label is actually persisted",
    );
}

#[test]
fn preanalysis_treats_immediate_fresh_object_field_stores_as_direct() {
    let func = FunctionIR {
        name: "stack_field_store".to_string(),
        params: vec![],
        ops: vec![
            OpIR {
                kind: "const".to_string(),
                out: Some("cls".to_string()),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "object_new_bound_stack".to_string(),
                out: Some("obj".to_string()),
                args: Some(vec!["cls".to_string()]),
                value: Some(24),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".to_string(),
                out: Some("zero".to_string()),
                value: Some(0),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_init".to_string(),
                args: Some(vec!["obj".to_string(), "zero".to_string()]),
                value: Some(0),
                ..OpIR::default()
            },
            OpIR {
                kind: "copy".to_string(),
                out: Some("alias".to_string()),
                args: Some(vec!["obj".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".to_string(),
                out: Some("one".to_string()),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "store".to_string(),
                args: Some(vec!["alias".to_string(), "one".to_string()]),
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
    };

    let analysis = preanalyze_for_test(&func, &BTreeMap::new());

    assert!(
        !analysis.has_store,
        "immediate stores into fresh stack object slots should lower as direct field writes"
    );
    assert_eq!(
        analysis.field_store_modes.get(&3),
        Some(&FieldStoreMode::FreshInit),
        "the init write owns fresh-slot initialization semantics"
    );
    assert_eq!(
        analysis.field_store_modes.get(&6),
        Some(&FieldStoreMode::DirectNonHeap),
        "the later same-slot immediate write should be direct"
    );
}

#[test]
fn preanalysis_treats_immediate_heap_fixed_layout_field_stores_as_direct() {
    let func = FunctionIR {
        name: "heap_fixed_layout_field_store".to_string(),
        params: vec![],
        ops: vec![
            OpIR {
                kind: "const".to_string(),
                out: Some("cls".to_string()),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "object_new_bound".to_string(),
                out: Some("obj".to_string()),
                args: Some(vec!["cls".to_string()]),
                value: Some(24),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".to_string(),
                out: Some("zero".to_string()),
                value: Some(0),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_init".to_string(),
                args: Some(vec!["obj".to_string(), "zero".to_string()]),
                value: Some(0),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some("p".to_string()),
                args: Some(vec!["obj".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "load_var".to_string(),
                var: Some("p".to_string()),
                out: Some("alias".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".to_string(),
                out: Some("one".to_string()),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "store".to_string(),
                args: Some(vec!["alias".to_string(), "one".to_string()]),
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
    };

    let analysis = preanalyze_for_test(&func, &BTreeMap::new());

    assert!(
        !analysis.has_store,
        "non-heap stores into fresh fixed-layout heap object slots should lower as direct field writes"
    );
    assert_eq!(
        analysis.field_store_modes.get(&3),
        Some(&FieldStoreMode::FreshInit),
        "sized object_new_bound roots should initialize fixed payload slots"
    );
    assert_eq!(
        analysis.field_store_modes.get(&7),
        Some(&FieldStoreMode::DirectNonHeap),
        "sized object_new_bound roots should share the stack-object direct-store contract"
    );
}

#[test]
fn preanalysis_rejects_unsized_heap_object_direct_field_stores() {
    let func = FunctionIR {
        name: "unsized_heap_field_store".to_string(),
        params: vec![],
        ops: vec![
            OpIR {
                kind: "const".to_string(),
                out: Some("cls".to_string()),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "object_new_bound".to_string(),
                out: Some("obj".to_string()),
                args: Some(vec!["cls".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".to_string(),
                out: Some("zero".to_string()),
                value: Some(0),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_init".to_string(),
                args: Some(vec!["obj".to_string(), "zero".to_string()]),
                value: Some(0),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some("p".to_string()),
                args: Some(vec!["obj".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "load_var".to_string(),
                var: Some("p".to_string()),
                out: Some("alias".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".to_string(),
                out: Some("one".to_string()),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "store".to_string(),
                args: Some(vec!["alias".to_string(), "one".to_string()]),
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
    };

    let analysis = preanalyze_for_test(&func, &BTreeMap::new());

    assert!(
        analysis.has_store,
        "heap object stores without a fixed payload-size proof must keep runtime field helpers"
    );
    assert!(analysis.field_store_modes.is_empty());
}

#[test]
fn preanalysis_classifies_fresh_heap_field_first_store_as_init() {
    let func = FunctionIR {
        name: "fresh_heap_first_store".to_string(),
        params: vec![],
        ops: vec![
            OpIR {
                kind: "const".to_string(),
                out: Some("cls".to_string()),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "object_new_bound".to_string(),
                out: Some("obj".to_string()),
                args: Some(vec!["cls".to_string()]),
                value: Some(24),
                ..OpIR::default()
            },
            OpIR {
                kind: "dict_new".to_string(),
                out: Some("regs".to_string()),
                args: Some(vec![]),
                ..OpIR::default()
            },
            OpIR {
                kind: "store".to_string(),
                args: Some(vec!["obj".to_string(), "regs".to_string()]),
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
    };

    let analysis = preanalyze_for_test(&func, &BTreeMap::new());

    assert_eq!(
        analysis.field_store_modes.get(&3),
        Some(&FieldStoreMode::FreshInit),
        "first heap-valued write to a fresh fixed-layout slot must not use overwrite semantics"
    );
}

#[test]
fn preanalysis_keeps_heap_field_second_store_as_overwrite() {
    let func = FunctionIR {
        name: "fresh_heap_second_store".to_string(),
        params: vec![],
        ops: vec![
            OpIR {
                kind: "const".to_string(),
                out: Some("cls".to_string()),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "object_new_bound".to_string(),
                out: Some("obj".to_string()),
                args: Some(vec!["cls".to_string()]),
                value: Some(24),
                ..OpIR::default()
            },
            OpIR {
                kind: "dict_new".to_string(),
                out: Some("first".to_string()),
                args: Some(vec![]),
                ..OpIR::default()
            },
            OpIR {
                kind: "store".to_string(),
                args: Some(vec!["obj".to_string(), "first".to_string()]),
                value: Some(0),
                ..OpIR::default()
            },
            OpIR {
                kind: "dict_new".to_string(),
                out: Some("second".to_string()),
                args: Some(vec![]),
                ..OpIR::default()
            },
            OpIR {
                kind: "store".to_string(),
                args: Some(vec!["obj".to_string(), "second".to_string()]),
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
    };

    let analysis = preanalyze_for_test(&func, &BTreeMap::new());

    assert_eq!(
        analysis.field_store_modes.get(&3),
        Some(&FieldStoreMode::FreshInit)
    );
    assert!(
        !analysis.field_store_modes.contains_key(&5),
        "second heap write to the same slot must stay generic overwrite so the old dict is released"
    );
}

#[test]
fn preanalysis_rejects_fresh_init_after_escape() {
    let func = FunctionIR {
        name: "fresh_store_after_escape".to_string(),
        params: vec![],
        ops: vec![
            OpIR {
                kind: "const".to_string(),
                out: Some("cls".to_string()),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "object_new_bound".to_string(),
                out: Some("obj".to_string()),
                args: Some(vec!["cls".to_string()]),
                value: Some(24),
                ..OpIR::default()
            },
            OpIR {
                kind: "call".to_string(),
                args: Some(vec!["obj".to_string()]),
                out: Some("escaped".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "dict_new".to_string(),
                out: Some("regs".to_string()),
                args: Some(vec![]),
                ..OpIR::default()
            },
            OpIR {
                kind: "store".to_string(),
                args: Some(vec!["obj".to_string(), "regs".to_string()]),
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
    };

    let analysis = preanalyze_for_test(&func, &BTreeMap::new());

    assert!(
        !analysis.field_store_modes.contains_key(&4),
        "once the object escapes, first-write init semantics are no longer locally provable"
    );
}

#[test]
fn preanalysis_treats_store_var_join_slot_as_alias_definition() {
    let func = FunctionIR {
        name: "join_alias".to_string(),
        params: vec![],
        ops: vec![
            OpIR {
                kind: "const_str".to_string(),
                out: Some("src".to_string()),
                s_value: Some("hi".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some("_bb4_arg0".to_string()),
                args: Some(vec!["src".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "load_var".to_string(),
                var: Some("_bb4_arg0".to_string()),
                out: Some("joined".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".to_string(),
                var: Some("joined".to_string()),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
    };

    let analysis = preanalyze_for_test(&func, &BTreeMap::new());

    assert_eq!(
        analysis.alias_roots.get("_bb4_arg0").map(String::as_str),
        Some("src")
    );
    assert_eq!(
        analysis.alias_roots.get("joined").map(String::as_str),
        Some("src")
    );
    assert_eq!(analysis.last_use.get("src"), Some(&3));
    assert_eq!(analysis.last_use.get("_bb4_arg0"), Some(&3));
}

#[test]
fn preanalysis_uses_args_based_copy_var_value_source() {
    let func = FunctionIR {
        name: "args_copy_alias".to_string(),
        params: vec!["value".to_string(), "metadata_slot".to_string()],
        ops: vec![
            OpIR {
                kind: "copy_var".to_string(),
                var: Some("metadata_slot".to_string()),
                args: Some(vec!["value".to_string()]),
                out: Some("alias".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".to_string(),
                var: Some("alias".to_string()),
                args: Some(vec!["alias".to_string()]),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
    };

    let analysis = preanalyze_for_test(&func, &BTreeMap::new());

    assert_eq!(
        analysis.alias_roots.get("alias").map(String::as_str),
        Some("value"),
        "args[0] is the copied value authority; var is local-name metadata"
    );
    assert_eq!(analysis.last_use.get("value"), Some(&1));
    assert_eq!(analysis.last_use.get("metadata_slot"), Some(&0));
}

#[test]
fn preanalysis_marks_unused_outputs_live_through_their_definition_site() {
    let func = FunctionIR {
        name: "unused_delete_temp".to_string(),
        params: vec![],
        ops: vec![
            OpIR {
                kind: "load_var".to_string(),
                var: Some("item".to_string()),
                out: Some("tmp_loaded".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "missing".to_string(),
                out: Some("tmp_missing".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some("item".to_string()),
                args: Some(vec!["tmp_missing".to_string()]),
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
    };

    let analysis = preanalyze_for_test(&func, &BTreeMap::new());

    assert_eq!(analysis.last_use.get("tmp_loaded"), Some(&0));
    assert_eq!(analysis.last_use.get("tmp_missing"), Some(&2));
}

#[test]
fn preanalysis_only_marks_store_slots_as_loop_body_reassignments() {
    let func = FunctionIR {
        name: "loop_store_slot_only".to_string(),
        params: vec![],
        ops: vec![
            OpIR {
                kind: "loop_start".to_string(),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_str".to_string(),
                out: Some("tmp".to_string()),
                s_value: Some("hi".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some("slot".to_string()),
                args: Some(vec!["tmp".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".to_string(),
                out: Some("v116".to_string()),
                value: Some(0),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some("_v7".to_string()),
                args: Some(vec!["v116".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_end".to_string(),
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
    };

    let analysis = preanalyze_for_test(&func, &BTreeMap::new());

    assert_eq!(
        analysis.loop_body_out_vars.get(&0),
        Some(&vec!["slot".to_string()]),
        "loop-body slot tracking should ignore SSA temps and only keep slot-backed reassignments",
    );
    assert_eq!(
        analysis.loop_body_init_vars.get(&0),
        Some(&vec!["slot".to_string()]),
        "slot-backed loop vars without any pre-loop store need an explicit first-iteration sentinel",
    );
}

#[test]
fn preanalysis_does_not_reinitialize_loop_slots_with_preloop_store() {
    let func = FunctionIR {
        name: "loop_store_slot_preinit".to_string(),
        params: vec![],
        ops: vec![
            OpIR {
                kind: "const_bool".to_string(),
                out: Some("v0".to_string()),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some("slot".to_string()),
                args: Some(vec!["v0".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_start".to_string(),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_bool".to_string(),
                out: Some("v1".to_string()),
                value: Some(0),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some("slot".to_string()),
                args: Some(vec!["v1".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_continue".to_string(),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_end".to_string(),
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
    };

    let analysis = preanalyze_for_test(&func, &BTreeMap::new());

    assert_eq!(
        analysis.loop_body_out_vars.get(&2),
        Some(&vec!["slot".to_string()]),
        "loop cleanup still needs to track the slot as loop-carried",
    );
    assert!(
        analysis
            .loop_body_init_vars
            .get(&2)
            .is_none_or(|names| !names.iter().any(|name| name == "slot")),
        "pre-loop stores must not be clobbered by synthetic None initialization",
    );
}

#[test]
fn slot_backed_join_names_skip_load_only_phi_join_carriers() {
    let ops = vec![
        OpIR {
            kind: "phi".to_string(),
            out: Some("joined".to_string()),
            args: Some(vec!["lhs".to_string(), "rhs".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "label".to_string(),
            value: Some(18),
            ..OpIR::default()
        },
        OpIR {
            kind: "load_var".to_string(),
            var: Some("_bb4_arg0".to_string()),
            out: Some("joined".to_string()),
            ..OpIR::default()
        },
    ];

    let names = collect_slot_backed_join_names(&ops, &BTreeSet::new(), false);

    assert!(
        !names.contains("_bb4_arg0"),
        "load-only structured phi join carriers must stay on the SSA path",
    );
}

#[test]
fn slot_backed_join_names_keep_explicit_store_backed_join_carriers() {
    let ops = vec![
        OpIR {
            kind: "store_var".to_string(),
            var: Some("_bb4_arg0".to_string()),
            args: Some(vec!["src".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "label".to_string(),
            value: Some(18),
            ..OpIR::default()
        },
        OpIR {
            kind: "load_var".to_string(),
            var: Some("_bb4_arg0".to_string()),
            out: Some("joined".to_string()),
            ..OpIR::default()
        },
    ];

    let names = collect_slot_backed_join_names(&ops, &BTreeSet::new(), false);

    assert!(
        names.contains("_bb4_arg0"),
        "explicit store-backed join carriers must remain slot-backed",
    );
}

#[test]
fn exception_slot_backing_ignores_compiler_value_temps() {
    let ops = vec![
        OpIR {
            kind: "store_var".to_string(),
            var: Some("_bb4_arg0".to_string()),
            args: Some(vec!["seed".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_var".to_string(),
            var: Some("slot".to_string()),
            args: Some(vec!["seed".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_var".to_string(),
            var: Some("_v7".to_string()),
            args: Some(vec!["seed".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_var".to_string(),
            var: Some("v116".to_string()),
            args: Some(vec!["seed".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "try_start".to_string(),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_var".to_string(),
            var: Some("_v8".to_string()),
            args: Some(vec!["seed".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_var".to_string(),
            var: Some("handler_slot".to_string()),
            args: Some(vec!["seed".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "exception_pop".to_string(),
            ..OpIR::default()
        },
    ];
    let exception_labels = BTreeSet::from([7]);

    let names = collect_slot_backed_join_names(&ops, &exception_labels, false);

    assert!(names.contains("_bb4_arg0"));
    assert!(names.contains("slot"));
    assert!(names.contains("handler_slot"));
    for temp in ["_v7", "v116", "_v8"] {
        assert!(
            !names.contains(temp),
            "compiler value temp {temp} must not become exception slot-backed"
        );
    }
}

#[test]
fn slot_exclusion_marks_call_arg_as_unsafe() {
    let func = FunctionIR {
        name: "call_escape".to_string(),
        params: vec![],
        ops: vec![
            OpIR {
                kind: "const".to_string(),
                out: Some("x".to_string()),
                value: Some(42),
                ..OpIR::default()
            },
            OpIR {
                kind: "call".to_string(),
                args: Some(vec!["x".to_string()]),
                out: Some("result".to_string()),
                s_value: Some("some_fn".to_string()),
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
    };

    let analysis = preanalyze_for_test(&func, &BTreeMap::new());
    assert!(
        analysis.scalar_slot_exclusion_unsafe.contains("x"),
        "int variable passed to call must be marked unsafe for slot exclusion"
    );
}

#[test]
fn slot_exclusion_marks_returned_var_as_unsafe() {
    let func = FunctionIR {
        name: "ret_escape".to_string(),
        params: vec![],
        ops: vec![
            OpIR {
                kind: "const".to_string(),
                out: Some("x".to_string()),
                value: Some(7),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".to_string(),
                var: Some("x".to_string()),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
    };

    let analysis = preanalyze_for_test(&func, &BTreeMap::new());
    assert!(
        analysis.scalar_slot_exclusion_unsafe.contains("x"),
        "int variable in ret must be marked unsafe for slot exclusion"
    );
}

#[test]
fn slot_exclusion_marks_store_attr_value_as_unsafe() {
    let func = FunctionIR {
        name: "heap_escape".to_string(),
        params: vec![],
        ops: vec![
            OpIR {
                kind: "const".to_string(),
                out: Some("val".to_string()),
                value: Some(99),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_attr".to_string(),
                args: Some(vec!["obj".to_string(), "val".to_string()]),
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
    };

    let analysis = preanalyze_for_test(&func, &BTreeMap::new());
    assert!(
        analysis.scalar_slot_exclusion_unsafe.contains("val"),
        "int variable in store_attr must be marked unsafe for slot exclusion"
    );
}

#[test]
fn slot_exclusion_marks_refcount_ops_as_unsafe() {
    let func = FunctionIR {
        name: "refcount_escape".to_string(),
        params: vec![],
        ops: vec![
            OpIR {
                kind: "const".to_string(),
                out: Some("x".to_string()),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "inc_ref".to_string(),
                args: Some(vec!["x".to_string()]),
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
    };

    let analysis = preanalyze_for_test(&func, &BTreeMap::new());
    assert!(
        analysis.scalar_slot_exclusion_unsafe.contains("x"),
        "int variable with inc_ref must be marked unsafe for slot exclusion"
    );
}

#[test]
fn slot_exclusion_marks_refcount_var_field_as_unsafe() {
    // A dec_ref op that references a scalar via op.var must also
    // mark it unsafe -- the runtime will dec_ref the boxed value
    // and needs the slot-backed refcount-correct representation.
    let func = FunctionIR {
        name: "refcount_var_escape".to_string(),
        params: vec![],
        ops: vec![
            OpIR {
                kind: "const".to_string(),
                out: Some("x".to_string()),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "dec_ref".to_string(),
                var: Some("x".to_string()),
                args: Some(vec!["x".to_string()]),
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
    };

    let analysis = preanalyze_for_test(&func, &BTreeMap::new());
    assert!(
        analysis.scalar_slot_exclusion_unsafe.contains("x"),
        "int variable in dec_ref var field must be marked unsafe for slot exclusion"
    );
}

#[test]
fn slot_exclusion_marks_release_var_field_as_unsafe() {
    // release op referencing a scalar via op.var
    let func = FunctionIR {
        name: "release_var_escape".to_string(),
        params: vec![],
        ops: vec![
            OpIR {
                kind: "const".to_string(),
                out: Some("y".to_string()),
                value: Some(42),
                ..OpIR::default()
            },
            OpIR {
                kind: "release".to_string(),
                var: Some("y".to_string()),
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
    };

    let analysis = preanalyze_for_test(&func, &BTreeMap::new());
    assert!(
        analysis.scalar_slot_exclusion_unsafe.contains("y"),
        "int variable in release var field must be marked unsafe for slot exclusion"
    );
}

#[test]
fn slot_exclusion_safe_for_pure_arithmetic_loop() {
    // Pure arithmetic: x = const, loop { x += 1 } -- no escape
    let func = FunctionIR {
        name: "safe_arith".to_string(),
        params: vec![],
        ops: vec![
            OpIR {
                kind: "const".to_string(),
                out: Some("x".to_string()),
                value: Some(0),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some("_bb1_arg0".to_string()),
                args: Some(vec!["x".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_start".to_string(),
                ..OpIR::default()
            },
            OpIR {
                kind: "load_var".to_string(),
                var: Some("_bb1_arg0".to_string()),
                out: Some("cur".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".to_string(),
                out: Some("one".to_string()),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "inplace_add".to_string(),
                args: Some(vec!["cur".to_string(), "one".to_string()]),
                out: Some("next".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some("_bb1_arg0".to_string()),
                args: Some(vec!["next".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_continue".to_string(),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_end".to_string(),
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
    };

    let analysis = preanalyze_for_test(&func, &BTreeMap::new());
    assert!(
        !analysis.scalar_slot_exclusion_unsafe.contains("x"),
        "pure arithmetic loop var must NOT be marked unsafe"
    );
    assert!(
        !analysis.scalar_slot_exclusion_unsafe.contains("_bb1_arg0"),
        "join slot for pure arithmetic loop must NOT be marked unsafe"
    );
    assert!(
        !analysis.scalar_slot_exclusion_unsafe.contains("cur"),
        "loaded loop var must NOT be marked unsafe"
    );
}

#[test]
fn slot_exclusion_marks_store_index_on_generic_list() {
    // Storing int to a generic list requires boxing correctness
    let func = FunctionIR {
        name: "list_store_escape".to_string(),
        params: vec![],
        ops: vec![
            OpIR {
                kind: "const".to_string(),
                out: Some("idx".to_string()),
                value: Some(0),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".to_string(),
                out: Some("val".to_string()),
                value: Some(42),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_index".to_string(),
                args: Some(vec![
                    "lst".to_string(),
                    "idx".to_string(),
                    "val".to_string(),
                ]),
                container_type: Some("list".to_string()),
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
    };

    let analysis = preanalyze_for_test(&func, &BTreeMap::new());
    assert!(
        analysis.scalar_slot_exclusion_unsafe.contains("val"),
        "int value stored to generic list must be marked unsafe"
    );
}

#[test]
fn slot_exclusion_allows_store_index_on_list_int() {
    // Storing int to list_int is safe (flat i64 storage, no boxing)
    let func = FunctionIR {
        name: "list_int_store_safe".to_string(),
        params: vec![],
        ops: vec![
            list_int_new("lst"),
            OpIR {
                kind: "const".to_string(),
                out: Some("idx".to_string()),
                value: Some(0),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".to_string(),
                out: Some("val".to_string()),
                value: Some(42),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_index".to_string(),
                args: Some(vec![
                    "lst".to_string(),
                    "idx".to_string(),
                    "val".to_string(),
                ]),
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
    };

    let analysis = preanalyze_for_test(&func, &BTreeMap::new());
    assert!(
        !analysis.scalar_slot_exclusion_unsafe.contains("val"),
        "int value stored to list_int must NOT be marked unsafe"
    );
}
