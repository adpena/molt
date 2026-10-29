use super::*;
use molt_ir::OpKind;

#[test]
fn native_transparency_uses_generated_no_incref_move_authority() {
    for kind in [
        OpKind::Copy,
        OpKind::CopyVar,
        OpKind::LoadVar,
        OpKind::StoreVar,
        OpKind::IdentityAlias,
        OpKind::BindingAlias,
        OpKind::Box,
        OpKind::Unbox,
        OpKind::Cast,
        OpKind::Widen,
        OpKind::GuardTag,
        OpKind::GuardType,
    ] {
        let op = OpIR {
            args: Some(if matches!(kind, OpKind::GuardTag | OpKind::GuardType) {
                vec!["source".into(), "expected_tag".into()]
            } else {
                vec!["source".into()]
            }),
            out: Some("result".into()),
            ..OpIR::new(kind)
        };
        assert_eq!(
            super::super::preanalyze_alias_source(&op).is_some(),
            crate::tir::op_kinds_generated::copy_kind_is_explicit_no_heap_move_table(kind),
            "{kind}",
        );
    }
    for kind in [OpKind::Box, OpKind::Unbox, OpKind::BindingAlias] {
        let mut input = super::cleanup_roots::token_test_ir();
        input.ops.push(OpIR {
            args: Some(vec!["owner".into()]),
            out: Some("independent".into()),
            ..OpIR::new(kind)
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
            args: Some(vec!["borrowed".into(), "first".into(), "second".into()]),
            value: Some(2),
            ..OpIR::new(OpKind::UnpackSequence)
        },
        OpIR {
            var: Some("sum".into()),
            out: Some("overflow".into()),
            args: Some(vec!["first".into(), "second".into()]),
            ..OpIR::new(OpKind::CheckedAdd)
        },
        OpIR {
            out: Some("metadata_only".into()),
            args: Some(vec!["owner".into()]),
            ..OpIR::new(OpKind::DecRef)
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
        var: Some("local".into()),
        out: Some("result".into()),
        args: Some(vec!["owner".into()]),
        ..OpIR::new(OpKind::StoreVar)
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
            var: var.map(str::to_string),
            out: out.map(str::to_string),
            args: Some(vec!["owner".into()]),
            ..OpIR::new(OpKind::StoreVar)
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
    for kind in [OpKind::CopyVar, OpKind::LoadVar] {
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
                var: Some(var.into()),
                args,
                out: Some("snapshot".into()),
                ..OpIR::new(kind)
            };
            let ops = vec![
                OpIR {
                    value: Some(10),
                    ..OpIR::new(OpKind::TryStart)
                },
                read.clone(),
                OpIR {
                    ..OpIR::new(OpKind::ExceptionPop)
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
            out: Some("snapshot".into()),
            args: Some(vec!["borrowed".into()]),
            ..OpIR::new(OpKind::Copy)
        },
        OpIR {
            out: Some("borrowed".into()),
            ..OpIR::new(OpKind::Alloc)
        },
        OpIR {
            args: Some(vec!["snapshot".into()]),
            ..OpIR::new(OpKind::Ret)
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
                out: Some("src".to_string()),
                s_value: Some("hi".to_string()),
                ..OpIR::new(OpKind::ConstStr)
            },
            OpIR {
                var: Some("_bb4_arg0".to_string()),
                args: Some(vec!["src".to_string()]),
                ..OpIR::new(OpKind::StoreVar)
            },
            OpIR {
                var: Some("_bb4_arg0".to_string()),
                out: Some("joined".to_string()),
                ..OpIR::new(OpKind::LoadVar)
            },
            OpIR {
                args: Some(vec!["joined".to_string()]),
                ..OpIR::new(OpKind::Ret)
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
                var: Some("metadata_slot".to_string()),
                args: Some(vec!["value".to_string()]),
                out: Some("alias".to_string()),
                ..OpIR::new(OpKind::CopyVar)
            },
            OpIR {
                args: Some(vec!["alias".to_string()]),
                ..OpIR::new(OpKind::Ret)
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
                var: Some("item".to_string()),
                out: Some("tmp_loaded".to_string()),
                ..OpIR::new(OpKind::LoadVar)
            },
            OpIR {
                out: Some("tmp_missing".to_string()),
                ..OpIR::new(OpKind::Missing)
            },
            OpIR {
                var: Some("item".to_string()),
                args: Some(vec!["tmp_missing".to_string()]),
                ..OpIR::new(OpKind::StoreVar)
            },
            OpIR {
                ..OpIR::new(OpKind::RetVoid)
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
                ..OpIR::new(OpKind::LoopStart)
            },
            OpIR {
                out: Some("tmp".to_string()),
                s_value: Some("hi".to_string()),
                ..OpIR::new(OpKind::ConstStr)
            },
            OpIR {
                var: Some("slot".to_string()),
                args: Some(vec!["tmp".to_string()]),
                ..OpIR::new(OpKind::StoreVar)
            },
            OpIR {
                out: Some("v116".to_string()),
                value: Some(0),
                ..OpIR::new(OpKind::Const)
            },
            OpIR {
                var: Some("_v7".to_string()),
                args: Some(vec!["v116".to_string()]),
                ..OpIR::new(OpKind::StoreVar)
            },
            OpIR {
                ..OpIR::new(OpKind::LoopEnd)
            },
            OpIR {
                ..OpIR::new(OpKind::RetVoid)
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
                out: Some("v0".to_string()),
                value: Some(1),
                ..OpIR::new(OpKind::ConstBool)
            },
            OpIR {
                var: Some("slot".to_string()),
                args: Some(vec!["v0".to_string()]),
                ..OpIR::new(OpKind::StoreVar)
            },
            OpIR {
                ..OpIR::new(OpKind::LoopStart)
            },
            OpIR {
                out: Some("v1".to_string()),
                value: Some(0),
                ..OpIR::new(OpKind::ConstBool)
            },
            OpIR {
                var: Some("slot".to_string()),
                args: Some(vec!["v1".to_string()]),
                ..OpIR::new(OpKind::StoreVar)
            },
            OpIR {
                ..OpIR::new(OpKind::LoopContinue)
            },
            OpIR {
                ..OpIR::new(OpKind::LoopEnd)
            },
            OpIR {
                ..OpIR::new(OpKind::RetVoid)
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

    assert!(
        analysis
            .loop_body_init_vars
            .get(&2)
            .is_none_or(|names| !names.iter().any(|name| name == "slot")),
        "pre-loop stores must not be clobbered by synthetic None initialization",
    );
}

#[test]
fn native_runtime_guards_check_expected_tags_and_define_used_results() {
    for kind in [OpKind::GuardTag, OpKind::GuardType] {
        let mut input = super::cleanup_roots::token_test_ir();
        input.name = "molt_main".into();
        input.return_abi = molt_ir::FunctionReturnAbi::Value;
        input.params.clear();
        input.ops = vec![
            OpIR {
                out: Some("source".into()),
                value: Some(7),
                ..OpIR::new(OpKind::Const)
            },
            OpIR {
                out: Some("tag".into()),
                value: Some(5),
                ..OpIR::new(OpKind::Const)
            },
            OpIR {
                args: Some(vec!["source".into(), "tag".into()]),
                out: Some("checked".into()),
                s_value: Some("int".into()),
                ..OpIR::new(kind)
            },
            OpIR {
                args: Some(vec!["checked".into()]),
                ..OpIR::new(OpKind::Ret)
            },
        ];
        let object = SimpleBackend::new().compile(SimpleIR {
            functions: vec![input],
            profile: None,
        });
        assert!(
            native_object_symbols(&object.bytes)
                .undefined
                .contains("molt_guard_type"),
            "{kind}: two raw integers and a misleading hint must not erase the check"
        );
        assert!(
            native_object_symbols(&object.bytes)
                .undefined
                .contains("molt_profile_enabled"),
            "{kind}: admitted tag checks must be gated by the runtime profile flag"
        );
    }
}
