use super::super::megafunction_split::{split_region_boundaries, verify_split_frame_ops};
use super::*;
use crate::tir::op_kinds_generated::{
    simpleir_kind_is_verifier_label_definition, simpleir_kind_is_verifier_label_reference,
};
use std::collections::BTreeMap;

fn label_op(kind: &str, target: i64) -> OpIR {
    OpIR {
        kind: kind.into(),
        value: Some(target),
        ..OpIR::default()
    }
}

fn function(ops: Vec<OpIR>) -> FunctionIR {
    FunctionIR {
        name: "split_control".into(),
        params: vec!["condition".into()],
        return_abi: molt_ir::FunctionReturnAbi::Void,
        execution_context: ExecutionContextPolicy::Inherited,
        ops,
        ..FunctionIR::default()
    }
}

fn split(func: FunctionIR) -> Result<(FunctionIR, Vec<FunctionIR>), Box<FunctionIR>> {
    let mut occupied = BTreeSet::from([func.name.clone()]);
    split_large_function(func, 8, &mut occupied)
}

fn shared_suffix(kind: &str) -> FunctionIR {
    let mut transfer = label_op(kind, 100);
    if kind == "br_if" {
        transfer.args = Some(vec!["condition".into()]);
    }
    let mut ops = vec![label_op("line", 1)];
    if kind == "try_end" {
        ops.push(label_op("try_start", 100));
    }
    ops.push(transfer);
    if kind == "try_start" {
        ops.push(label_op("try_end", 100));
    }
    for index in 0..12 {
        ops.push(label_op("line", index + 2));
        ops.push(make_const_int(&format!("value_{index}"), index));
    }
    ops.extend([label_op("label", 100), make_op("ret_void")]);
    function(ops)
}

fn assert_closed_control(func: &FunctionIR) {
    assert!(
        split_region_boundaries(&func.ops).is_some(),
        "{} has unbalanced regions",
        func.name
    );
    let labels: BTreeSet<_> = func
        .ops
        .iter()
        .filter(|op| simpleir_kind_is_verifier_label_definition(&op.kind))
        .filter_map(|op| op.value)
        .collect();
    for op in &func.ops {
        if simpleir_kind_is_verifier_label_reference(&op.kind) {
            assert!(
                op.value.is_some_and(|target| labels.contains(&target)),
                "{} contains external {} target {:?}",
                func.name,
                op.kind,
                op.value
            );
        }
    }
}

#[test]
fn every_generated_label_reference_clones_a_closed_shared_suffix() {
    for kind in [
        "jump",
        "goto",
        "br_if",
        "try_start",
        "try_end",
        "check_exception",
        "async_work_poll",
    ] {
        assert!(simpleir_kind_is_verifier_label_reference(kind));
        let (stub, chunks) =
            split(shared_suffix(kind)).unwrap_or_else(|_| panic!("{kind} should split"));
        assert!(chunks.len() >= 2, "{kind}");
        assert_closed_control(&stub);
        for chunk in &chunks {
            assert_closed_control(chunk);
        }
        assert!(
            chunks[0]
                .ops
                .iter()
                .any(|op| op.kind == kind && op.value == Some(100)),
            "{kind}"
        );
        assert!(
            chunks[0]
                .ops
                .iter()
                .any(|op| op.kind == "label" && op.value == Some(100)),
            "{kind} must bring its target into the first chunk"
        );
    }
}

#[test]
fn missing_generated_label_targets_refuse_without_mutating_source() {
    for kind in [
        "jump",
        "goto",
        "br_if",
        "try_start",
        "try_end",
        "check_exception",
        "async_work_poll",
    ] {
        let mut source = shared_suffix(kind);
        source
            .ops
            .iter_mut()
            .find(|op| op.kind == "label")
            .unwrap()
            .value = Some(200);
        let expected = serde_json::to_value(&source).unwrap();
        let original = split(source).expect_err("missing target must refuse split");
        assert_eq!(
            serde_json::to_value(&*original).unwrap(),
            expected,
            "{kind}"
        );
    }
}

#[test]
fn suffix_targets_inside_structured_regions_are_not_cloned() {
    for (start, end) in [
        ("if", "end_if"),
        ("loop_start", "loop_end"),
        ("try_start", "try_end"),
    ] {
        let mut source = shared_suffix("check_exception");
        source.ops.pop();
        source.ops.pop();
        let mut opener = label_op(start, 200);
        if start == "if" {
            opener.args = Some(vec!["condition".into()]);
        }
        source.ops.extend([
            opener,
            label_op("label", 100),
            make_const_int("inside", 1),
            label_op(end, 200),
            label_op("label", 200),
            make_op("ret_void"),
        ]);
        let boundaries = split_region_boundaries(&source.ops).expect("balanced source");
        let target = source
            .ops
            .iter()
            .position(|op| op.kind == "label" && op.value == Some(100))
            .unwrap();
        assert!(!boundaries[target]);
        assert!(
            split(source).is_err(),
            "{start} suffix starts inside an active region"
        );
    }
}

#[test]
fn malformed_region_suffixes_fail_before_partitioning() {
    for tail in [
        vec![make_op("end_if")],
        vec![make_op("if")],
        vec![make_op("if"), make_op("loop_end")],
        vec![
            make_op("if"),
            make_op("else"),
            make_op("else"),
            make_op("end_if"),
        ],
        vec![make_op("try_end")],
        vec![make_op("try_start")],
        vec![make_op("loop_break")],
    ] {
        let mut source = shared_suffix("check_exception");
        source.ops.pop();
        source.ops.extend(tail);
        source.ops.push(make_op("ret_void"));
        assert!(split_region_boundaries(&source.ops).is_none());
        assert!(split(source).is_err());
    }
}

#[test]
fn canonical_loop_index_initialization_does_not_open_a_second_region() {
    let ops = vec![
        make_op("loop_start"),
        make_op("loop_index_start"),
        make_op("loop_end"),
        label_op("line", 1),
    ];
    assert_eq!(
        split_region_boundaries(&ops).unwrap(),
        vec![true, false, false, true, true]
    );
}

#[test]
fn frame_checks_follow_allocated_identity_and_slot_operands_not_name_prefixes() {
    let layout = BTreeMap::from([("loaded".into(), 0), ("stored".into(), 1)]);
    let frame = "allocated_frame";
    let source = function(vec![
        make_const_int("__molt_split_frame_index", 0),
        make_arith("index", &[frame, "__molt_split_frame_index"], "loaded"),
        make_const_int("ordinary_slot_name", 1),
        OpIR {
            kind: "store_index".into(),
            args: Some(vec![
                frame.into(),
                "ordinary_slot_name".into(),
                "stored".into(),
            ]),
            ..OpIR::default()
        },
    ]);
    verify_split_frame_ops(&source, frame, &layout).unwrap();
    for index in [1, 3] {
        let mut bad = source.clone();
        bad.ops[index].args.as_mut().unwrap()[1] = "wrong_slot_operand".into();
        assert!(verify_split_frame_ops(&bad, frame, &layout).is_err());
        let mut bad = source.clone();
        bad.ops[index - 1].value = Some(99);
        assert!(verify_split_frame_ops(&bad, frame, &layout).is_err());
    }
    let mut wrong_value = source.clone();
    wrong_value.ops[3].args.as_mut().unwrap()[2] = "loaded".into();
    assert!(verify_split_frame_ops(&wrong_value, frame, &layout).is_err());
    let user_names = function(vec![
        make_const_int("__molt_split_frame_index", 5),
        make_const_int("__molt_split_frame_load_index", 6),
        make_const_int("__molt_split_frame_store_index", 7),
        make_op("ret_void"),
    ]);
    verify_split_generated_ops(&user_names).unwrap();
    verify_split_frame_ops(&user_names, frame, &layout).unwrap();
}
