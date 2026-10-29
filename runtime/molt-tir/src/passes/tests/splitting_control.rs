use super::super::megafunction_split::{split_region_boundaries, verify_split_frame_ops};
use super::*;

use std::collections::BTreeMap;

fn label_op(kind: molt_ir::OpKind, target: i64) -> OpIR {
    OpIR {
        kind,
        value: Some(target),
        ..OpIR::new(kind)
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

fn shared_suffix(kind: molt_ir::OpKind) -> FunctionIR {
    let mut transfer = label_op(kind, 100);
    if kind == molt_ir::OpKind::BrIf {
        transfer.args = Some(vec!["condition".into()]);
    }
    let mut ops = vec![label_op(molt_ir::OpKind::Line, 1)];
    if kind == molt_ir::OpKind::TryEnd {
        ops.push(label_op(molt_ir::OpKind::TryStart, 100));
    }
    ops.push(transfer);
    if kind == molt_ir::OpKind::TryStart {
        ops.push(label_op(molt_ir::OpKind::TryEnd, 100));
    }
    for index in 0..12 {
        ops.push(label_op(molt_ir::OpKind::Line, index + 2));
        ops.push(make_const_int(&format!("value_{index}"), index));
    }
    ops.extend([
        label_op(molt_ir::OpKind::Label, 100),
        make_op(molt_ir::OpKind::RetVoid),
    ]);
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
        .filter(|op| op.kind.is_verifier_label_definition())
        .filter_map(|op| op.value)
        .collect();
    for op in &func.ops {
        if op.kind.is_verifier_label_reference() {
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
        molt_ir::OpKind::Jump,
        molt_ir::OpKind::Goto,
        molt_ir::OpKind::BrIf,
        molt_ir::OpKind::TryStart,
        molt_ir::OpKind::TryEnd,
        molt_ir::OpKind::CheckException,
        molt_ir::OpKind::AsyncWorkPoll,
    ] {
        assert!(kind.is_verifier_label_reference());
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
                .any(|op| op.kind == molt_ir::OpKind::Label && op.value == Some(100)),
            "{kind} must bring its target into the first chunk"
        );
    }
}

#[test]
fn missing_generated_label_targets_refuse_without_mutating_source() {
    for kind in [
        molt_ir::OpKind::Jump,
        molt_ir::OpKind::Goto,
        molt_ir::OpKind::BrIf,
        molt_ir::OpKind::TryStart,
        molt_ir::OpKind::TryEnd,
        molt_ir::OpKind::CheckException,
        molt_ir::OpKind::AsyncWorkPoll,
    ] {
        let mut source = shared_suffix(kind);
        source
            .ops
            .iter_mut()
            .find(|op| op.kind == molt_ir::OpKind::Label)
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
        (molt_ir::OpKind::If, molt_ir::OpKind::EndIf),
        (molt_ir::OpKind::LoopStart, molt_ir::OpKind::LoopEnd),
        (molt_ir::OpKind::TryStart, molt_ir::OpKind::TryEnd),
    ] {
        let mut source = shared_suffix(molt_ir::OpKind::CheckException);
        source.ops.pop();
        source.ops.pop();
        let mut opener = label_op(start, 200);
        if start == molt_ir::OpKind::If {
            opener.args = Some(vec!["condition".into()]);
        }
        source.ops.extend([
            opener,
            label_op(molt_ir::OpKind::Label, 100),
            make_const_int("inside", 1),
            label_op(end, 200),
            label_op(molt_ir::OpKind::Label, 200),
            make_op(molt_ir::OpKind::RetVoid),
        ]);
        let boundaries = split_region_boundaries(&source.ops).expect("balanced source");
        let target = source
            .ops
            .iter()
            .position(|op| op.kind == molt_ir::OpKind::Label && op.value == Some(100))
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
        vec![make_op(molt_ir::OpKind::EndIf)],
        vec![make_op(molt_ir::OpKind::If)],
        vec![
            make_op(molt_ir::OpKind::If),
            make_op(molt_ir::OpKind::LoopEnd),
        ],
        vec![
            make_op(molt_ir::OpKind::If),
            make_op(molt_ir::OpKind::Else),
            make_op(molt_ir::OpKind::Else),
            make_op(molt_ir::OpKind::EndIf),
        ],
        vec![make_op(molt_ir::OpKind::TryEnd)],
        vec![make_op(molt_ir::OpKind::TryStart)],
        vec![make_op(molt_ir::OpKind::LoopBreak)],
    ] {
        let mut source = shared_suffix(molt_ir::OpKind::CheckException);
        source.ops.pop();
        source.ops.extend(tail);
        source.ops.push(make_op(molt_ir::OpKind::RetVoid));
        assert!(split_region_boundaries(&source.ops).is_none());
        assert!(split(source).is_err());
    }
}

#[test]
fn canonical_loop_index_initialization_does_not_open_a_second_region() {
    let ops = vec![
        make_op(molt_ir::OpKind::LoopStart),
        make_op(molt_ir::OpKind::LoopIndexStart),
        make_op(molt_ir::OpKind::LoopEnd),
        label_op(molt_ir::OpKind::Line, 1),
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
        make_arith(
            molt_ir::OpKind::Index,
            &[frame, "__molt_split_frame_index"],
            "loaded",
        ),
        make_const_int("ordinary_slot_name", 1),
        OpIR {
            kind: molt_ir::OpKind::StoreIndex,
            args: Some(vec![
                frame.into(),
                "ordinary_slot_name".into(),
                "stored".into(),
            ]),
            ..OpIR::new(molt_ir::OpKind::StoreIndex)
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
        make_op(molt_ir::OpKind::RetVoid),
    ]);
    verify_split_frame_ops(&user_names, frame, &layout).unwrap();
}
