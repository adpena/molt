use super::*;

fn field_op(kind: &str, args: &[&str], out: Option<&str>, value: Option<i64>) -> OpIR {
    OpIR {
        kind: kind.into(),
        args: Some(args.iter().map(|arg| (*arg).into()).collect()),
        out: out.map(str::to_owned),
        value,
        ..OpIR::default()
    }
}

fn field_fixture(allocation_kind: &str, payload: Option<i64>, body: Vec<OpIR>) -> FunctionIR {
    let mut ops = vec![
        field_op("const", &[], Some("zero"), Some(0)),
        field_op("const", &[], Some("one"), Some(1)),
        field_op(allocation_kind, &["cls"], Some("obj"), payload),
    ];
    ops.extend(body);
    ops.push(field_op("ret_void", &[], None, None));
    FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "field_store_authority".into(),
        params: ["cls", "heap", "callback", "dynamic"]
            .map(str::to_owned)
            .to_vec(),
        ops,
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        execution_context: Default::default(),
    }
}

fn field_store(kind: &str, object: &str, value: &str, offset: i64) -> OpIR {
    field_op(kind, &[object, value], None, Some(offset))
}

#[test]
fn pristine_field_modes_share_owned_allocation_and_alias_contracts() {
    for cell_alias in [false, true] {
        let mut body = vec![field_store("store", "obj", "zero", 0)];
        if cell_alias {
            let mut save = field_op("store_var", &["obj"], None, None);
            save.var = Some("cell".into());
            let mut load = field_op("load_var", &[], Some("alias"), None);
            load.var = Some("cell".into());
            body.extend([save, load]);
        } else {
            body.push(field_op("copy", &["obj"], Some("alias"), None));
        }
        body.push(field_store("store", "alias", "one", 0));
        let func = field_fixture("object_new_bound", Some(24), body);
        let replacement = func.ops.len() - 2;
        let analysis = preanalyze_for_test(&func);
        assert_eq!(
            analysis.field_store_modes.get(&3),
            Some(&FieldStoreMode::FreshInit)
        );
        assert_eq!(
            analysis.field_store_modes.get(&replacement),
            Some(&FieldStoreMode::DirectNonHeap),
            "cell={cell_alias}"
        );
        assert!(
            analysis.needs_field_store_profile,
            "FreshInit still owns field-store profiling"
        );
    }
}

#[test]
fn raw_boxed_neutral_stores_do_not_request_runtime_profiling() {
    let mut func = field_fixture(
        "alloc",
        Some(16),
        vec![
            field_store("store", "obj", "zero", 0),
            field_store("store", "obj", "one", 0),
        ],
    );
    func.ops[2].args = Some(vec![]);
    let analysis = preanalyze_for_test(&func);
    assert_eq!(analysis.field_store_modes.len(), 2);
    for index in [3, 4] {
        assert_eq!(
            analysis.field_store_modes.get(&index),
            Some(&FieldStoreMode::DirectNonHeap)
        );
    }
    assert!(!analysis.needs_field_store_profile);
}

#[test]
fn unsized_and_out_of_extent_fields_do_not_gain_direct_modes() {
    for payload in [None, Some(7), Some(9), Some(24)] {
        for offset in [-8, 0, 1, 16, 24, i64::MAX - 7] {
            if payload == Some(24) && offset == 0 {
                continue;
            }
            let func = field_fixture(
                "object_new_bound",
                payload,
                vec![
                    field_store("store", "obj", "zero", offset),
                    field_store("store", "obj", "one", offset),
                ],
            );
            let analysis = preanalyze_for_test(&func);
            assert!(
                analysis.field_store_modes.is_empty(),
                "{payload:?} {offset}"
            );
            assert!(analysis.needs_field_store_profile);
        }
    }
}

#[test]
fn first_heap_write_uses_init_but_displaced_heap_release_stays_generic() {
    let func = field_fixture(
        "object_new_bound",
        Some(24),
        vec![
            field_store("store", "obj", "heap", 0),
            field_store("store", "obj", "dynamic", 0),
            field_store("store", "obj", "one", 0),
        ],
    );
    let analysis = preanalyze_for_test(&func);
    assert_eq!(
        analysis.field_store_modes.get(&3),
        Some(&FieldStoreMode::FreshInit)
    );
    assert_eq!(
        typed_slot_store_helper_name(analysis.field_store_modes.get(&3).copied()),
        "molt_object_field_init_ptr"
    );
    for index in [4, 5] {
        assert!(!analysis.field_store_modes.contains_key(&index));
        assert_eq!(
            typed_slot_store_helper_name(analysis.field_store_modes.get(&index).copied()),
            "molt_object_field_set_ptr"
        );
    }
}

#[test]
fn wide_integer_carriers_do_not_mint_boxed_neutral_slot_facts() {
    let mut func = field_fixture(
        "object_new_bound",
        Some(24),
        vec![
            field_store("store", "obj", "zero", 0),
            field_store("store", "obj", "one", 0),
            field_store("store", "obj", "one", 0),
        ],
    );
    func.ops[0].value = Some(i64::MAX);
    let analysis = preanalyze_for_test(&func);
    assert_eq!(
        analysis.field_store_modes.get(&3),
        Some(&FieldStoreMode::FreshInit)
    );
    assert!(!analysis.field_store_modes.contains_key(&4));
    assert!(!analysis.field_store_modes.contains_key(&5));
}

#[test]
fn observers_captures_and_unrelated_callbacks_permanently_revoke_history() {
    for observer in [
        field_op("call", &["callback"], Some("result"), None),
        field_op("add", &["heap", "dynamic"], Some("result"), None),
        field_op("module_get_attr", &["dynamic"], Some("result"), None),
        field_op("get_attr", &["obj", "dynamic"], Some("result"), None),
        field_store("store", "heap", "obj", 0),
        field_store("store", "heap", "zero", 0),
    ] {
        let kind = observer.kind.clone();
        let func = field_fixture(
            "object_new_bound",
            Some(24),
            vec![
                field_store("store", "obj", "zero", 0),
                observer,
                field_store("store", "obj", "one", 0),
                field_store("store", "obj", "one", 8),
                field_store("store", "obj", "one", 8),
            ],
        );
        let analysis = preanalyze_for_test(&func);
        assert_eq!(
            analysis.field_store_modes.get(&3),
            Some(&FieldStoreMode::FreshInit)
        );
        for index in 5..=7 {
            assert!(
                !analysis.field_store_modes.contains_key(&index),
                "{kind} at {index}"
            );
            assert_eq!(
                typed_slot_store_helper_name(analysis.field_store_modes.get(&index).copied()),
                "molt_object_field_set_ptr",
                "{kind} at {index}"
            );
        }
    }
}

#[test]
fn releasing_one_field_revokes_other_field_history() {
    let func = field_fixture(
        "object_new_bound",
        Some(24),
        vec![
            field_store("store", "obj", "zero", 0),
            field_store("store", "obj", "heap", 8),
            field_store("store", "obj", "one", 8),
            field_store("store", "obj", "one", 0),
        ],
    );
    let analysis = preanalyze_for_test(&func);
    assert_eq!(
        analysis.field_store_modes.get(&3),
        Some(&FieldStoreMode::FreshInit)
    );
    assert_eq!(
        analysis.field_store_modes.get(&4),
        Some(&FieldStoreMode::FreshInit)
    );
    assert!(!analysis.field_store_modes.contains_key(&5));
    assert!(!analysis.field_store_modes.contains_key(&6));
}

#[test]
fn durable_origins_join_current_unoptimized_store_sites() {
    for transported in [false, true] {
        let mut func = field_fixture(
            "object_new_bound",
            Some(24),
            vec![
                field_store("store", "obj", "zero", 0),
                field_store("store", "obj", "one", 0),
            ],
        );
        // The optimizer's private clone may erase the local stores entirely.
        // Modes still belong to this exact current unoptimized SimpleIR.
        func.ops
            .insert(0, field_op("drop_inserted", &[], None, None));
        if transported {
            for (index, op) in func.ops.iter_mut().enumerate() {
                op.source_op_idx = Some(500 + index as i64 * 3);
            }
        }
        let analysis = preanalyze_for_test(&func);
        assert_eq!(
            analysis.field_store_modes.get(&4),
            Some(&FieldStoreMode::FreshInit)
        );
        assert_eq!(
            analysis.field_store_modes.get(&5),
            Some(&FieldStoreMode::DirectNonHeap)
        );
        assert_eq!(analysis.field_store_modes.len(), 2);
    }
}

#[test]
fn duplicate_source_origins_fail_closed_instead_of_first_or_last_wins() {
    let mut func = field_fixture(
        "object_new_bound",
        Some(24),
        vec![
            field_store("store", "obj", "zero", 0),
            field_store("store", "obj", "one", 0),
        ],
    );
    for collision in [0, 4] {
        func.ops.iter_mut().for_each(|op| op.source_op_idx = None);
        func.ops[3].source_op_idx = Some(70);
        func.ops[collision].source_op_idx = Some(70);
        let analysis = preanalyze_for_test(&func);
        assert!(!analysis.field_store_modes.contains_key(&3));
        if collision == 4 {
            assert!(analysis.field_store_modes.is_empty());
        }
    }
}
