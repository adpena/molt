use super::*;

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
        execution_context: Default::default(),
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
        execution_context: Default::default(),
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
        execution_context: Default::default(),
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
        execution_context: Default::default(),
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
        execution_context: Default::default(),
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
        execution_context: Default::default(),
    };

    let analysis = preanalyze_for_test(&func, &BTreeMap::new());

    assert!(
        !analysis.field_store_modes.contains_key(&4),
        "once the object escapes, first-write init semantics are no longer locally provable"
    );
}
