use super::*;

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
        execution_context: Default::default(),
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
                args: Some(vec!["x".to_string()]),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
        execution_context: Default::default(),
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
        execution_context: Default::default(),
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
        execution_context: Default::default(),
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
        execution_context: Default::default(),
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
        execution_context: Default::default(),
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
        execution_context: Default::default(),
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
        execution_context: Default::default(),
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
        execution_context: Default::default(),
    };

    let analysis = preanalyze_for_test(&func, &BTreeMap::new());
    assert!(
        !analysis.scalar_slot_exclusion_unsafe.contains("val"),
        "int value stored to list_int must NOT be marked unsafe"
    );
}
