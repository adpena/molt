use super::*;

#[test]
fn sum_reduction_detects_canonical_pattern() {
    // Simulates the IR for:
    //   total = 0
    //   for x in list_of_ints:
    //       total += x
    let ops = vec![
        list_int_new("my_list"),
        // 0: loop_start
        OpIR {
            kind: "loop_start".to_string(),
            ..OpIR::default()
        },
        // 1: loop_index_start  (idx)
        OpIR {
            kind: "loop_index_start".to_string(),
            out: Some("idx".to_string()),
            args: Some(vec!["start_val".to_string()]),
            ..OpIR::default()
        },
        // 2: index  list[idx]  -> elem
        OpIR {
            kind: "index".to_string(),
            args: Some(vec!["my_list".to_string(), "idx".to_string()]),
            out: Some("elem".to_string()),
            bce_safe: Some(true),
            ..OpIR::default()
        },
        // 3: add  [total, elem]  -> sum_result
        OpIR {
            kind: "add".to_string(),
            args: Some(vec!["total".to_string(), "elem".to_string()]),
            out: Some("sum_result".to_string()),
            ..OpIR::default()
        },
        // 4: store_var  total = sum_result
        OpIR {
            kind: "store_var".to_string(),
            var: Some("total".to_string()),
            args: Some(vec!["sum_result".to_string()]),
            ..OpIR::default()
        },
        // 5: loop_index_next
        OpIR {
            kind: "loop_index_next".to_string(),
            args: Some(vec!["next_idx".to_string()]),
            out: Some("idx_next".to_string()),
            ..OpIR::default()
        },
        // 6: loop_continue
        OpIR {
            kind: "loop_continue".to_string(),
            ..OpIR::default()
        },
        // 7: loop_end
        OpIR {
            kind: "loop_end".to_string(),
            ..OpIR::default()
        },
    ];

    let plan = representation_plan_for_ops(&ops);
    let result = scan_loop_int_sum_reduction(&ops, 2, "idx", &plan);
    assert!(result.is_some(), "canonical sum reduction must be detected");
    let candidate = result.unwrap();
    assert_eq!(candidate.list_name, "my_list");
    assert_eq!(candidate.acc_store_slot, "total");
    assert_eq!(candidate.add_out_name, "sum_result");
    assert_eq!(candidate.acc_operand_name, "total");
    assert_eq!(candidate.loop_end_idx, 8);
}

#[test]
fn sum_reduction_detects_reversed_add_operands() {
    // add [elem, total] instead of [total, elem]
    let ops = vec![
        list_int_new("lst"),
        OpIR {
            kind: "loop_index_start".to_string(),
            out: Some("i".to_string()),
            args: Some(vec!["zero".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "index".to_string(),
            args: Some(vec!["lst".to_string(), "i".to_string()]),
            out: Some("e".to_string()),
            bce_safe: Some(true),
            ..OpIR::default()
        },
        OpIR {
            kind: "inplace_add".to_string(),
            args: Some(vec!["e".to_string(), "acc".to_string()]),
            out: Some("new_acc".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_var".to_string(),
            var: Some("acc".to_string()),
            args: Some(vec!["new_acc".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "loop_end".to_string(),
            ..OpIR::default()
        },
    ];

    let plan = representation_plan_for_ops(&ops);
    let result = scan_loop_int_sum_reduction(&ops, 1, "i", &plan);
    assert!(
        result.is_some(),
        "reversed operand sum reduction must be detected"
    );
    let c = result.unwrap();
    assert_eq!(c.acc_operand_name, "acc");
    assert_eq!(c.list_name, "lst");
}

#[test]
fn sum_reduction_rejects_non_bce_safe() {
    let ops = vec![
        list_int_new("lst"),
        OpIR {
            kind: "loop_index_start".to_string(),
            out: Some("i".to_string()),
            args: Some(vec!["zero".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "index".to_string(),
            args: Some(vec!["lst".to_string(), "i".to_string()]),
            out: Some("e".to_string()),
            bce_safe: None, // NOT bce_safe
            ..OpIR::default()
        },
        OpIR {
            kind: "add".to_string(),
            args: Some(vec!["acc".to_string(), "e".to_string()]),
            out: Some("new_acc".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_var".to_string(),
            var: Some("acc".to_string()),
            args: Some(vec!["new_acc".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "loop_end".to_string(),
            ..OpIR::default()
        },
    ];
    let plan = representation_plan_for_ops(&ops);

    assert!(
        scan_loop_int_sum_reduction(&ops, 1, "i", &plan).is_none(),
        "non-bce_safe index must disqualify sum reduction"
    );
}

#[test]
fn sum_reduction_rejects_call_in_body() {
    let ops = vec![
        list_int_new("lst"),
        OpIR {
            kind: "loop_index_start".to_string(),
            out: Some("i".to_string()),
            args: Some(vec!["zero".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "index".to_string(),
            args: Some(vec!["lst".to_string(), "i".to_string()]),
            out: Some("e".to_string()),
            bce_safe: Some(true),
            ..OpIR::default()
        },
        // Side-effecting call in loop body â€” disqualifies
        OpIR {
            kind: "call".to_string(),
            args: Some(vec!["e".to_string()]),
            out: Some("result".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "add".to_string(),
            args: Some(vec!["acc".to_string(), "result".to_string()]),
            out: Some("new_acc".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_var".to_string(),
            var: Some("acc".to_string()),
            args: Some(vec!["new_acc".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "loop_end".to_string(),
            ..OpIR::default()
        },
    ];
    let plan = representation_plan_for_ops(&ops);

    assert!(
        scan_loop_int_sum_reduction(&ops, 1, "i", &plan).is_none(),
        "call in loop body must disqualify sum reduction"
    );
}

#[test]
fn sum_reduction_rejects_nested_loop() {
    let ops = vec![
        list_int_new("lst"),
        OpIR {
            kind: "loop_index_start".to_string(),
            out: Some("i".to_string()),
            args: Some(vec!["zero".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "index".to_string(),
            args: Some(vec!["lst".to_string(), "i".to_string()]),
            out: Some("e".to_string()),
            bce_safe: Some(true),
            ..OpIR::default()
        },
        // Nested loop
        OpIR {
            kind: "loop_start".to_string(),
            ..OpIR::default()
        },
        OpIR {
            kind: "loop_end".to_string(),
            ..OpIR::default()
        },
        OpIR {
            kind: "add".to_string(),
            args: Some(vec!["acc".to_string(), "e".to_string()]),
            out: Some("new_acc".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_var".to_string(),
            var: Some("acc".to_string()),
            args: Some(vec!["new_acc".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "loop_end".to_string(),
            ..OpIR::default()
        },
    ];
    let plan = representation_plan_for_ops(&ops);

    assert!(
        scan_loop_int_sum_reduction(&ops, 1, "i", &plan).is_none(),
        "nested loop must disqualify sum reduction"
    );
}

#[test]
fn sum_reduction_rejects_wrong_index_var() {
    // Index uses a different variable than the loop induction variable
    let ops = vec![
        list_int_new("lst"),
        OpIR {
            kind: "loop_index_start".to_string(),
            out: Some("i".to_string()),
            args: Some(vec!["zero".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "index".to_string(),
            args: Some(vec!["lst".to_string(), "other_var".to_string()]),
            out: Some("e".to_string()),
            bce_safe: Some(true),
            ..OpIR::default()
        },
        OpIR {
            kind: "add".to_string(),
            args: Some(vec!["acc".to_string(), "e".to_string()]),
            out: Some("new_acc".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_var".to_string(),
            var: Some("acc".to_string()),
            args: Some(vec!["new_acc".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "loop_end".to_string(),
            ..OpIR::default()
        },
    ];
    let plan = representation_plan_for_ops(&ops);

    assert!(
        scan_loop_int_sum_reduction(&ops, 1, "i", &plan).is_none(),
        "index with non-induction variable must disqualify"
    );
}

#[test]
fn sum_reduction_rejects_non_list_int() {
    let ops = vec![
        OpIR {
            kind: "loop_index_start".to_string(),
            out: Some("i".to_string()),
            args: Some(vec!["zero".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "index".to_string(),
            args: Some(vec!["lst".to_string(), "i".to_string()]),
            out: Some("e".to_string()),
            container_type: Some("list".to_string()), // generic list, not list_int
            bce_safe: Some(true),
            ..OpIR::default()
        },
        OpIR {
            kind: "add".to_string(),
            args: Some(vec!["acc".to_string(), "e".to_string()]),
            out: Some("new_acc".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_var".to_string(),
            var: Some("acc".to_string()),
            args: Some(vec!["new_acc".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "loop_end".to_string(),
            ..OpIR::default()
        },
    ];
    let plan = representation_plan_for_ops(&ops);

    assert!(
        scan_loop_int_sum_reduction(&ops, 0, "i", &plan).is_none(),
        "non-list_int container must disqualify"
    );
}

#[test]
fn sum_reduction_rejects_multiple_stores() {
    let ops = vec![
        list_int_new("lst"),
        OpIR {
            kind: "loop_index_start".to_string(),
            out: Some("i".to_string()),
            args: Some(vec!["zero".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "index".to_string(),
            args: Some(vec!["lst".to_string(), "i".to_string()]),
            out: Some("e".to_string()),
            bce_safe: Some(true),
            ..OpIR::default()
        },
        OpIR {
            kind: "add".to_string(),
            args: Some(vec!["acc".to_string(), "e".to_string()]),
            out: Some("new_acc".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_var".to_string(),
            var: Some("acc".to_string()),
            args: Some(vec!["new_acc".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_var".to_string(),
            var: Some("other".to_string()),
            args: Some(vec!["e".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "loop_end".to_string(),
            ..OpIR::default()
        },
    ];
    let plan = representation_plan_for_ops(&ops);

    assert!(
        scan_loop_int_sum_reduction(&ops, 1, "i", &plan).is_none(),
        "multiple store_var ops must disqualify"
    );
}

#[test]
fn sum_reduction_rejects_add_elem_mismatch() {
    // add operands don't include the index element
    let ops = vec![
        list_int_new("lst"),
        OpIR {
            kind: "loop_index_start".to_string(),
            out: Some("i".to_string()),
            args: Some(vec!["zero".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "index".to_string(),
            args: Some(vec!["lst".to_string(), "i".to_string()]),
            out: Some("e".to_string()),
            bce_safe: Some(true),
            ..OpIR::default()
        },
        OpIR {
            kind: "add".to_string(),
            args: Some(vec!["acc".to_string(), "other_val".to_string()]),
            out: Some("new_acc".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_var".to_string(),
            var: Some("acc".to_string()),
            args: Some(vec!["new_acc".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "loop_end".to_string(),
            ..OpIR::default()
        },
    ];
    let plan = representation_plan_for_ops(&ops);

    assert!(
        scan_loop_int_sum_reduction(&ops, 1, "i", &plan).is_none(),
        "add operand mismatch must disqualify"
    );
}

// â”€â”€ scalar_slot_exclusion_unsafe tests â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

#[test]
fn scan_loop_hoistable_lists_treats_store_index_as_mutation() {
    let flat_ops = vec![
        list_int_new("lst"),
        OpIR {
            kind: "loop_start".to_string(),
            ..OpIR::default()
        },
        OpIR {
            kind: "index".to_string(),
            args: Some(vec!["lst".to_string(), "idx".to_string()]),
            out: Some("cur".to_string()),
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
            kind: "loop_end".to_string(),
            ..OpIR::default()
        },
    ];
    let flat_plan = representation_plan_for_ops(&flat_ops);
    let flat_pre_loop_defined = collect_pre_loop_defined_names(&flat_ops, 1);
    let (flat_hoist, generic_hoist) =
        scan_loop_hoistable_lists(&flat_ops, 1, &flat_pre_loop_defined, &flat_plan);
    assert!(
        !flat_hoist.contains("lst"),
        "store_index must invalidate flat-list hoisting"
    );
    assert!(
        !generic_hoist.contains("lst"),
        "store_index must not leak through the generic hoist set"
    );

    let generic_ops = vec![
        OpIR {
            kind: "loop_start".to_string(),
            ..OpIR::default()
        },
        OpIR {
            kind: "index".to_string(),
            args: Some(vec!["lst".to_string(), "idx".to_string()]),
            out: Some("cur".to_string()),
            container_type: Some("list".to_string()),
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
            kind: "loop_end".to_string(),
            ..OpIR::default()
        },
    ];
    let generic_plan = representation_plan_for_typed_ops(
        &["lst", "idx", "val"],
        Some(vec!["list", "int", "int"]),
        &generic_ops,
    );
    let generic_pre_loop_defined = BTreeSet::from(["lst".to_string()]);
    let (flat_hoist, generic_hoist) =
        scan_loop_hoistable_lists(&generic_ops, 0, &generic_pre_loop_defined, &generic_plan);
    assert!(
        !flat_hoist.contains("lst"),
        "generic store_index must not enter the flat hoist set"
    );
    assert!(
        !generic_hoist.contains("lst"),
        "store_index must invalidate generic-list hoisting"
    );
}
