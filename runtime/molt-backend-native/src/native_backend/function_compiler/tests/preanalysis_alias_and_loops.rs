use super::*;

#[test]
fn native_transparency_uses_generated_no_incref_move_authority() {
    for kind in [
        "copy",
        "copy_var",
        "load_var",
        "store_var",
        "identity_alias",
        "binding_alias",
        "box",
        "unbox",
        "cast",
        "widen",
        "guard_tag",
        "guard_type",
    ] {
        let op = OpIR {
            kind: kind.into(),
            args: Some(vec!["source".into()]),
            out: Some("result".into()),
            ..OpIR::default()
        };
        assert_eq!(
            super::super::preanalyze_alias_source(&op).is_some(),
            crate::tir::op_kinds_generated::copy_kind_is_explicit_no_heap_move_table(kind),
            "{kind}",
        );
    }
    for kind in ["box", "unbox", "binding_alias"] {
        let mut input = super::cleanup_roots::token_test_ir();
        input.ops.push(OpIR {
            kind: kind.into(),
            args: Some(vec!["owner".into()]),
            out: Some("independent".into()),
            ..OpIR::default()
        });
        let analysis = preanalyze_for_test(&input);
        assert_eq!(
            analysis.alias_roots.get("independent").map(String::as_str),
            Some("independent"),
            "{kind} must retain its independent cleanup obligation"
        );
    }
}

#[test]
fn native_definition_scan_preserves_multi_results_and_rejects_metadata() {
    let mut input = super::cleanup_roots::token_test_ir();
    input.ops.extend([
        OpIR {
            kind: "unpack_sequence".into(),
            args: Some(vec!["borrowed".into(), "first".into(), "second".into()]),
            value: Some(2),
            ..OpIR::default()
        },
        OpIR {
            kind: "checked_add".into(),
            var: Some("sum".into()),
            out: Some("overflow".into()),
            args: Some(vec!["first".into(), "second".into()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "dec_ref".into(),
            out: Some("metadata_only".into()),
            args: Some(vec!["owner".into()]),
            ..OpIR::default()
        },
    ]);
    let analysis = preanalyze_function_ir(&input, &ScalarRepresentationPlan::default());
    for name in ["first", "second", "sum", "overflow"] {
        assert!(
            analysis.var_names.iter().any(|defined| defined == name),
            "{name}"
        );
        assert!(analysis.alias_roots.contains_key(name), "{name}");
    }
    assert!(
        !analysis
            .var_names
            .iter()
            .any(|name| name == "metadata_only")
    );
}

#[test]
fn result_carrying_store_preserves_both_binding_and_alias_definition() {
    let mut input = super::cleanup_roots::token_test_ir();
    input.ops.push(OpIR {
        kind: "store_var".into(),
        var: Some("local".into()),
        out: Some("result".into()),
        args: Some(vec!["owner".into()]),
        ..OpIR::default()
    });
    let analysis = preanalyze_for_test(&input);
    assert_eq!(
        analysis.alias_roots.get("local").map(String::as_str),
        Some("local")
    );
    assert_eq!(
        analysis.alias_roots.get("result").map(String::as_str),
        Some("owner")
    );
}

#[test]
fn native_binding_consumers_share_destination_and_optional_result_roles() {
    for (var, out, result) in [
        (Some("_bb4_arg0"), None, None),
        (None, Some("_bb4_arg0"), None),
        (Some("_bb4_arg0"), Some("_bb4_arg0"), None),
        (Some("_bb4_arg0"), Some("none"), None),
        (Some("_bb4_arg0"), Some("snapshot"), Some("snapshot")),
    ] {
        let mut input = super::cleanup_roots::token_test_ir();
        let store = OpIR {
            kind: "store_var".into(),
            var: var.map(str::to_string),
            out: out.map(str::to_string),
            args: Some(vec!["owner".into()]),
            ..OpIR::default()
        };
        input.ops.push(store.clone());
        let analysis = preanalyze_for_test(&input);
        assert_eq!(
            analysis.alias_roots.get("_bb4_arg0").map(String::as_str),
            Some("_bb4_arg0")
        );
        assert_eq!(
            analysis.alias_roots.contains_key("snapshot"),
            result.is_some()
        );
        assert!(!analysis.alias_roots.contains_key("none"));
        let slots = collect_slot_backed_join_names(&[store], &BTreeSet::new(), false);
        assert_eq!(slots, BTreeSet::from(["_bb4_arg0".to_string()]));
    }
}

#[test]
fn native_join_planning_uses_semantic_copy_sources_not_metadata() {
    for kind in ["copy_var", "load_var"] {
        for (var, args, expected) in [
            ("_bb1_arg0", Some(vec!["source".to_string()]), None),
            (
                "metadata",
                Some(vec!["_bb2_arg0".to_string()]),
                Some("_bb2_arg0"),
            ),
            ("_bb1_arg0", None, Some("_bb1_arg0")),
            ("_bb1_arg0", Some(vec![]), Some("_bb1_arg0")),
        ] {
            let read = OpIR {
                kind: kind.into(),
                var: Some(var.into()),
                args,
                out: Some("snapshot".into()),
                ..OpIR::default()
            };
            let ops = vec![
                OpIR {
                    kind: "try_start".into(),
                    value: Some(10),
                    ..OpIR::default()
                },
                read.clone(),
                OpIR {
                    kind: "exception_pop".into(),
                    ..OpIR::default()
                },
            ];
            let slots = collect_slot_backed_join_names(&ops, &BTreeSet::from([10]), false);
            assert_eq!(
                slots,
                expected.into_iter().map(str::to_string).collect(),
                "{read:?}"
            );
            let source =
                super::super::preanalyze_alias_source(&read).expect("copy has one semantic source");
            assert_eq!(source, expected.unwrap_or("source"), "{read:?}");
        }
    }
}

#[test]
fn parameter_entry_definition_makes_single_rebind_a_mutable_epoch() {
    let mut input = super::cleanup_roots::token_test_ir();
    input.return_abi = molt_ir::FunctionReturnAbi::Value;
    input.ops = vec![
        OpIR {
            kind: "copy".into(),
            out: Some("snapshot".into()),
            args: Some(vec!["borrowed".into()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "alloc".into(),
            out: Some("borrowed".into()),
            ..OpIR::default()
        },
        OpIR {
            kind: "ret".into(),
            args: Some(vec!["snapshot".into()]),
            ..OpIR::default()
        },
    ];
    let analysis = preanalyze_for_test(&input);
    assert_eq!(
        analysis.alias_roots.get("snapshot").map(String::as_str),
        Some("snapshot")
    );
    assert_eq!(
        analysis.alias_roots.get("borrowed").map(String::as_str),
        Some("borrowed")
    );
}

#[test]
fn preanalysis_separates_retained_storage_bindings_from_ssa_aliases() {
    let func = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
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
                args: Some(vec!["joined".to_string()]),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        execution_context: Default::default(),
    };

    let analysis = preanalyze_for_test(&func);

    assert_eq!(
        analysis.alias_roots.get("_bb4_arg0").map(String::as_str),
        Some("_bb4_arg0")
    );
    assert_eq!(
        analysis.alias_roots.get("joined").map(String::as_str),
        Some("joined")
    );
    assert_eq!(analysis.last_use.get("src"), Some(&1));
    assert_eq!(analysis.last_use.get("_bb4_arg0"), Some(&2));
}

#[test]
fn preanalysis_uses_args_based_copy_var_value_source() {
    let func = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
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
                args: Some(vec!["alias".to_string()]),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        execution_context: Default::default(),
    };

    let analysis = preanalyze_for_test(&func);

    assert_eq!(
        analysis.alias_roots.get("alias").map(String::as_str),
        Some("value"),
        "args[0] is the copied value authority; var is local-name metadata"
    );
    assert_eq!(analysis.last_use.get("value"), Some(&1));
    assert_eq!(analysis.last_use.get("metadata_slot"), None);
}

#[test]
fn preanalysis_marks_unused_outputs_live_through_their_definition_site() {
    let func = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Void,
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
        codegen_partition: false,
        execution_context: Default::default(),
    };

    let analysis = preanalyze_for_test(&func);

    assert_eq!(analysis.last_use.get("tmp_loaded"), Some(&0));
    assert_eq!(analysis.last_use.get("tmp_missing"), Some(&2));
}

#[test]
fn preanalysis_only_marks_store_slots_as_loop_body_reassignments() {
    let func = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Void,
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
        codegen_partition: false,
        execution_context: Default::default(),
    };

    let analysis = preanalyze_for_test(&func);

    assert_eq!(
        analysis.loop_body_init_vars.get(&0),
        Some(&vec!["slot".to_string()]),
        "slot-backed loop vars without any pre-loop store need an explicit first-iteration sentinel",
    );
}

#[test]
fn preanalysis_does_not_reinitialize_loop_slots_with_preloop_store() {
    let func = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Void,
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
        codegen_partition: false,
        execution_context: Default::default(),
    };

    let analysis = preanalyze_for_test(&func);

    assert!(
        analysis
            .loop_body_init_vars
            .get(&2)
            .is_none_or(|names| !names.iter().any(|name| name == "slot")),
        "pre-loop stores must not be clobbered by synthetic None initialization",
    );
}
