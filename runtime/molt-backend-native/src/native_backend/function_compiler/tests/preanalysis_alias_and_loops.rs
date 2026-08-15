use super::*;

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
                args: Some(vec!["joined".to_string()]),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
        execution_context: Default::default(),
    };

    let analysis = preanalyze_for_test(&func);

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
                args: Some(vec!["alias".to_string()]),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
        execution_context: Default::default(),
    };

    let analysis = preanalyze_for_test(&func);

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
        execution_context: Default::default(),
    };

    let analysis = preanalyze_for_test(&func);

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
        execution_context: Default::default(),
    };

    let analysis = preanalyze_for_test(&func);

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
        execution_context: Default::default(),
    };

    let analysis = preanalyze_for_test(&func);

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
