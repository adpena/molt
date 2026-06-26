use super::*;

#[test]
fn structured_if_skips_join_with_external_predecessor() {
    let mut func = TirFunction::new(
        "branch_with_shared_join".into(),
        vec![TirType::Bool, TirType::Bool],
        TirType::None,
    );

    let inner_if = func.fresh_block();
    let external_pred = func.fresh_block();
    let then_blk = func.fresh_block();
    let else_blk = func.fresh_block();
    let join_blk = func.fresh_block();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.terminator = Terminator::CondBranch {
        cond: ValueId(0),
        then_block: inner_if,
        then_args: vec![],
        else_block: external_pred,
        else_args: vec![],
    };

    func.blocks.insert(
        inner_if,
        TirBlock {
            id: inner_if,
            args: vec![],
            ops: vec![],
            terminator: Terminator::CondBranch {
                cond: ValueId(1),
                then_block: then_blk,
                then_args: vec![],
                else_block: else_blk,
                else_args: vec![],
            },
        },
    );
    func.blocks.insert(
        external_pred,
        TirBlock {
            id: external_pred,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: join_blk,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        then_blk,
        TirBlock {
            id: then_blk,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: join_blk,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        else_blk,
        TirBlock {
            id: else_blk,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: join_blk,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        join_blk,
        TirBlock {
            id: join_blk,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    let ops = lower_to_simple_ir(&func);
    assert!(
        validate_labels(&ops),
        "shared join labels must remain valid after lower_to_simple: {ops:?}"
    );
    assert!(
        !ops.iter()
            .any(|op| op.kind == "if" || op.kind == "else" || op.kind == "end_if"),
        "shared-join lowering must stay label-based instead of inlining to structured if/else: {ops:?}"
    );
    assert!(
        ops.iter().filter(|op| op.kind == "label").count() >= 4,
        "shared-join lowering must preserve explicit labels for the merge shape: {ops:?}"
    );
}

#[test]
fn structured_if_skips_arm_with_external_predecessor() {
    let mut func = TirFunction::new(
        "branch_with_shared_then_arm".into(),
        vec![TirType::Bool, TirType::Bool],
        TirType::None,
    );

    let inner_if = func.fresh_block();
    let external_pred = func.fresh_block();
    let then_blk = func.fresh_block();
    let else_blk = func.fresh_block();
    let join_blk = func.fresh_block();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.terminator = Terminator::CondBranch {
        cond: ValueId(0),
        then_block: inner_if,
        then_args: vec![],
        else_block: external_pred,
        else_args: vec![],
    };

    func.blocks.insert(
        inner_if,
        TirBlock {
            id: inner_if,
            args: vec![],
            ops: vec![],
            terminator: Terminator::CondBranch {
                cond: ValueId(1),
                then_block: then_blk,
                then_args: vec![],
                else_block: else_blk,
                else_args: vec![],
            },
        },
    );
    func.blocks.insert(
        external_pred,
        TirBlock {
            id: external_pred,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: then_blk,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        then_blk,
        TirBlock {
            id: then_blk,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: join_blk,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        else_blk,
        TirBlock {
            id: else_blk,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: join_blk,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        join_blk,
        TirBlock {
            id: join_blk,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    let ops = lower_to_simple_ir(&func);
    assert!(
        validate_labels(&ops),
        "shared-arm lowering must remain label-valid after lower_to_simple: {ops:?}"
    );
    assert!(
        !ops.iter()
            .any(|op| op.kind == "if" || op.kind == "else" || op.kind == "end_if"),
        "shared-arm lowering must stay label-based instead of inlining to structured if/else: {ops:?}"
    );
    assert!(
        ops.iter().filter(|op| op.kind == "label").count() >= 4,
        "shared-arm lowering must preserve explicit labels for the reused then-arm shape: {ops:?}"
    );
}

#[test]
fn structured_if_emits_join_arg_store_load_without_phi() {
    let mut func = TirFunction::new(
        "branch_with_join_arg".into(),
        vec![TirType::Bool],
        TirType::I64,
    );

    let then_blk = func.fresh_block();
    let else_blk = func.fresh_block();
    let join_blk = func.fresh_block();
    let then_val = func.fresh_value();
    let else_val = func.fresh_value();
    let join_arg = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.terminator = Terminator::CondBranch {
        cond: ValueId(0),
        then_block: then_blk,
        then_args: vec![],
        else_block: else_blk,
        else_args: vec![],
    };

    let mut then_attrs = AttrDict::new();
    then_attrs.insert("value".into(), AttrValue::Int(1));
    func.blocks.insert(
        then_blk,
        TirBlock {
            id: then_blk,
            args: vec![],
            ops: vec![TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![then_val],
                attrs: then_attrs,
                source_span: None,
            }],
            terminator: Terminator::Branch {
                target: join_blk,
                args: vec![then_val],
            },
        },
    );

    let mut else_attrs = AttrDict::new();
    else_attrs.insert("value".into(), AttrValue::Int(2));
    func.blocks.insert(
        else_blk,
        TirBlock {
            id: else_blk,
            args: vec![],
            ops: vec![TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![else_val],
                attrs: else_attrs,
                source_span: None,
            }],
            terminator: Terminator::Branch {
                target: join_blk,
                args: vec![else_val],
            },
        },
    );

    func.blocks.insert(
        join_blk,
        TirBlock {
            id: join_blk,
            args: vec![TirValue {
                id: join_arg,
                ty: TirType::DynBox,
            }],
            ops: vec![],
            terminator: Terminator::Return {
                values: vec![join_arg],
            },
        },
    );

    let ops = lower_to_simple_ir(&func);
    let kinds: Vec<&str> = ops.iter().map(|op| op.kind.as_str()).collect();

    assert!(kinds.contains(&"if"), "{ops:?}");
    assert!(kinds.contains(&"else"), "{ops:?}");
    assert!(kinds.contains(&"end_if"), "{ops:?}");
    assert!(
        !kinds.contains(&"phi"),
        "structured if join args must round-trip as store/load, not phi: {ops:?}"
    );
    assert!(
        ops.iter().filter(|op| op.kind == "store_var").count() >= 2,
        "structured if join args must emit branch-site stores: {ops:?}"
    );
    assert!(
        ops.iter().any(|op| op.kind == "load_var"),
        "structured if join args must reload the merged value after end_if: {ops:?}"
    );
}

#[test]
fn check_exception_materializes_handler_arg_stores() {
    let mut func = TirFunction::new("check_exception_handler_args".into(), vec![], TirType::I64);

    let value = func.fresh_value();
    let exit_block = func.fresh_block();
    let handler_block = func.fresh_block();
    let handler_arg = func.fresh_value();

    let mut const_attrs = AttrDict::new();
    const_attrs.insert("value".into(), AttrValue::Int(7));
    let mut handler_attrs = AttrDict::new();
    handler_attrs.insert("value".into(), AttrValue::Int(100));

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![value],
        attrs: const_attrs,
        source_span: None,
    });
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::CheckException,
        operands: vec![value],
        results: vec![],
        attrs: handler_attrs,
        source_span: None,
    });
    entry.terminator = Terminator::Branch {
        target: exit_block,
        args: vec![],
    };

    func.blocks.insert(
        exit_block,
        TirBlock {
            id: exit_block,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    func.blocks.insert(
        handler_block,
        TirBlock {
            id: handler_block,
            args: vec![TirValue {
                id: handler_arg,
                ty: TirType::DynBox,
            }],
            ops: vec![],
            terminator: Terminator::Return {
                values: vec![handler_arg],
            },
        },
    );

    func.has_exception_handling = true;
    func.label_id_map.insert(handler_block.0, 100);

    let ops = lower_to_simple_ir(&func);
    let handler_param = format!("_bb{}_arg0", handler_block.0);
    let handler_value = value_var(handler_arg);
    let entry_value = value_var(value);

    assert!(
        ops.iter().any(|op| {
            op.kind == "store_var"
                && op.var.as_deref() == Some(handler_param.as_str())
                && op
                    .args
                    .as_ref()
                    .is_some_and(|args| args == &vec![entry_value.clone()])
        }),
        "check_exception lowering must materialize handler arg stores before the handler label: {ops:?}"
    );
    assert!(
        ops.iter().any(|op| {
            op.kind == "load_var"
                && op.var.as_deref() == Some(handler_param.as_str())
                && op.out.as_deref() == Some(handler_value.as_str())
        }),
        "handler block must still reload its synthesized arg slot: {ops:?}"
    );
}

#[test]
fn try_start_materializes_handler_arg_stores() {
    let mut func = TirFunction::new("try_start_handler_args".into(), vec![], TirType::I64);

    let value = func.fresh_value();
    let exit_block = func.fresh_block();
    let handler_block = func.fresh_block();
    let handler_arg = func.fresh_value();

    let mut const_attrs = AttrDict::new();
    const_attrs.insert("value".into(), AttrValue::Int(7));
    let mut handler_attrs = AttrDict::new();
    handler_attrs.insert("value".into(), AttrValue::Int(100));

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![value],
        attrs: const_attrs,
        source_span: None,
    });
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::TryStart,
        operands: vec![value],
        results: vec![],
        attrs: handler_attrs,
        source_span: None,
    });
    entry.terminator = Terminator::Branch {
        target: exit_block,
        args: vec![],
    };

    func.blocks.insert(
        exit_block,
        TirBlock {
            id: exit_block,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    func.blocks.insert(
        handler_block,
        TirBlock {
            id: handler_block,
            args: vec![TirValue {
                id: handler_arg,
                ty: TirType::DynBox,
            }],
            ops: vec![],
            terminator: Terminator::Return {
                values: vec![handler_arg],
            },
        },
    );

    func.has_exception_handling = true;
    func.label_id_map.insert(handler_block.0, 100);

    let ops = lower_to_simple_ir(&func);
    let handler_param = format!("_bb{}_arg0", handler_block.0);
    let handler_value = value_var(handler_arg);
    let entry_value = value_var(value);

    assert!(
        ops.iter().any(|op| {
            op.kind == "store_var"
                && op.var.as_deref() == Some(handler_param.as_str())
                && op
                    .args
                    .as_ref()
                    .is_some_and(|args| args == &vec![entry_value.clone()])
        }),
        "try_start lowering must materialize handler arg stores before the handler label: {ops:?}"
    );
    assert!(
        ops.iter().any(|op| {
            op.kind == "load_var"
                && op.var.as_deref() == Some(handler_param.as_str())
                && op.out.as_deref() == Some(handler_value.as_str())
        }),
        "handler block must still reload its synthesized arg slot: {ops:?}"
    );
}

#[test]
fn structured_if_skips_one_return_one_continue_shape() {
    let mut func = TirFunction::new(
        "branch_with_fallthrough_join".into(),
        vec![TirType::Bool],
        TirType::None,
    );

    let then_blk = func.fresh_block();
    let else_blk = func.fresh_block();
    let join_blk = func.fresh_block();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.terminator = Terminator::CondBranch {
        cond: ValueId(0),
        then_block: then_blk,
        then_args: vec![],
        else_block: else_blk,
        else_args: vec![],
    };

    func.blocks.insert(
        then_blk,
        TirBlock {
            id: then_blk,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func.blocks.insert(
        else_blk,
        TirBlock {
            id: else_blk,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: join_blk,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        join_blk,
        TirBlock {
            id: join_blk,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    let ops = lower_to_simple_ir(&func);
    assert!(
        validate_labels(&ops),
        "mixed return/fallthrough shape must keep valid labels after lower_to_simple: {ops:?}"
    );
    assert!(
        !ops.iter()
            .any(|op| op.kind == "if" || op.kind == "else" || op.kind == "end_if"),
        "mixed return/fallthrough shape must stay label-based until region analysis proves it safe: {ops:?}"
    );
}

#[test]
fn structured_if_skips_successor_with_nested_scf() {
    let mut func = TirFunction::new(
        "branch_with_nested_scf_successor".into(),
        vec![TirType::Bool],
        TirType::None,
    );

    let then_blk = func.fresh_block();
    let else_blk = func.fresh_block();
    let join_blk = func.fresh_block();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.terminator = Terminator::CondBranch {
        cond: ValueId(0),
        then_block: then_blk,
        then_args: vec![],
        else_block: else_blk,
        else_args: vec![],
    };

    func.blocks.insert(
        then_blk,
        TirBlock {
            id: then_blk,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: join_blk,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        else_blk,
        TirBlock {
            id: else_blk,
            args: vec![],
            ops: vec![TirOp {
                dialect: Dialect::Scf,
                opcode: OpCode::ScfWhile,
                operands: vec![],
                results: vec![],
                attrs: HashMap::new(),
                source_span: None,
            }],
            terminator: Terminator::Branch {
                target: join_blk,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        join_blk,
        TirBlock {
            id: join_blk,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    let ops = lower_to_simple_ir(&func);
    assert!(
        validate_labels(&ops),
        "nested-scf successor lowering must keep valid labels after lower_to_simple: {ops:?}"
    );
    assert!(
        !ops.iter()
            .any(|op| op.kind == "if" || op.kind == "else" || op.kind == "end_if"),
        "successors containing nested SCF must stay label-based instead of inlining to structured if/else: {ops:?}"
    );
}

#[test]
fn structured_if_skips_successor_with_try_region_markers() {
    let mut func = TirFunction::new(
        "branch_with_try_region_successor".into(),
        vec![TirType::Bool],
        TirType::None,
    );

    let then_blk = func.fresh_block();
    let else_blk = func.fresh_block();
    let join_blk = func.fresh_block();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.terminator = Terminator::CondBranch {
        cond: ValueId(0),
        then_block: then_blk,
        then_args: vec![],
        else_block: else_blk,
        else_args: vec![],
    };

    let mut try_attrs = AttrDict::new();
    try_attrs.insert("value".into(), AttrValue::Int(100));
    func.blocks.insert(
        then_blk,
        TirBlock {
            id: then_blk,
            args: vec![],
            ops: vec![
                TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::TryStart,
                    operands: vec![],
                    results: vec![],
                    attrs: try_attrs.clone(),
                    source_span: None,
                },
                TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::TryEnd,
                    operands: vec![],
                    results: vec![],
                    attrs: try_attrs,
                    source_span: None,
                },
            ],
            terminator: Terminator::Branch {
                target: join_blk,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        else_blk,
        TirBlock {
            id: else_blk,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: join_blk,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        join_blk,
        TirBlock {
            id: join_blk,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    let ops = lower_to_simple_ir(&func);
    assert!(
        validate_labels(&ops),
        "try-region successor lowering must keep valid labels after lower_to_simple: {ops:?}"
    );
    assert!(
        !ops.iter()
            .any(|op| op.kind == "if" || op.kind == "else" || op.kind == "end_if"),
        "successors containing try_start/try_end must stay label-based instead of inlining to structured if/else: {ops:?}"
    );
}

#[test]
fn structured_if_skips_join_that_is_loop_header() {
    let mut func = TirFunction::new(
        "branch_with_loop_header_join".into(),
        vec![TirType::Bool],
        TirType::None,
    );

    let then_blk = func.fresh_block();
    let else_blk = func.fresh_block();
    let join_blk = func.fresh_block();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.terminator = Terminator::CondBranch {
        cond: ValueId(0),
        then_block: then_blk,
        then_args: vec![],
        else_block: else_blk,
        else_args: vec![],
    };

    func.blocks.insert(
        then_blk,
        TirBlock {
            id: then_blk,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: join_blk,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        else_blk,
        TirBlock {
            id: else_blk,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: join_blk,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        join_blk,
        TirBlock {
            id: join_blk,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func.loop_roles
        .insert(join_blk, crate::tir::blocks::LoopRole::LoopHeader);

    let ops = lower_to_simple_ir(&func);
    assert!(
        validate_labels(&ops),
        "loop-header join lowering must keep valid labels after lower_to_simple: {ops:?}"
    );
    assert!(
        !ops.iter()
            .any(|op| op.kind == "if" || op.kind == "else" || op.kind == "end_if"),
        "join blocks that are loop headers must stay label-based instead of inlining to structured if/else: {ops:?}"
    );
}

#[test]
fn loop_end_block_target_must_keep_its_label() {
    let mut func = TirFunction::new(
        "loop_end_block_target_must_keep_its_label".into(),
        vec![],
        TirType::None,
    );

    let target_block = func.fresh_block();
    let exit_block = func.fresh_block();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.terminator = Terminator::Branch {
        target: target_block,
        args: vec![],
    };

    func.blocks.insert(
        target_block,
        TirBlock {
            id: target_block,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: exit_block,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        exit_block,
        TirBlock {
            id: exit_block,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func.loop_roles
        .insert(target_block, crate::tir::blocks::LoopRole::LoopEnd);

    let ops = lower_to_simple_ir(&func);
    assert!(
        validate_labels(&ops),
        "loop-end block labels must survive when explicit branches still target them: {ops:?}"
    );
    assert!(
        ops.iter()
            .any(|op| matches!(op.kind.as_str(), "label" | "state_label") && op.value.is_some()),
        "expected a materialized target label for the loop-end block: {ops:?}"
    );
}

#[test]
fn eliminate_dead_loop_end_after_return() {
    let mut ops = vec![
        OpIR {
            kind: "ret".into(),
            var: Some("_ret0".into()),
            args: Some(vec!["_ret0".into()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "loop_end".into(),
            args: Some(vec![]),
            ..OpIR::default()
        },
        OpIR {
            kind: "label".into(),
            args: Some(vec![]),
            value: Some(42),
            ..OpIR::default()
        },
    ];

    eliminate_dead_labels(&mut ops);

    assert!(
        !ops.iter().any(|op| op.kind == "loop_end"),
        "dead loop_end must not survive after a real return: {ops:?}"
    );
}

#[test]
fn eliminate_dead_jump_after_return() {
    let mut ops = vec![
        OpIR {
            kind: "ret".into(),
            var: Some("_ret0".into()),
            args: Some(vec!["_ret0".into()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "jump".into(),
            value: Some(42),
            ..OpIR::default()
        },
        OpIR {
            kind: "label".into(),
            args: Some(vec![]),
            value: Some(42),
            ..OpIR::default()
        },
    ];

    eliminate_dead_labels(&mut ops);

    assert!(
        !ops.iter().any(|op| op.kind == "jump"),
        "dead jump must not survive after a real return: {ops:?}"
    );
}

#[test]
fn preserve_loop_end_after_live_labeled_raise_path() {
    let mut ops = vec![
        OpIR {
            kind: "br_if".into(),
            args: Some(vec!["cond".into()]),
            value: Some(7),
            ..OpIR::default()
        },
        OpIR {
            kind: "loop_continue".into(),
            ..OpIR::default()
        },
        OpIR {
            kind: "label".into(),
            value: Some(7),
            args: Some(vec![]),
            ..OpIR::default()
        },
        OpIR {
            kind: "raise".into(),
            args: Some(vec!["exc".into()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "loop_end".into(),
            args: Some(vec![]),
            ..OpIR::default()
        },
    ];

    eliminate_dead_labels(&mut ops);

    assert!(
        ops.iter().any(|op| op.kind == "loop_end"),
        "loop_end must survive after a live labeled terminal block because it still closes the structured loop break path: {ops:?}"
    );
}

#[test]
fn eliminate_dead_labels_preserves_explicit_post_raise_exception_transfer() {
    let mut ops = vec![
        OpIR {
            kind: "raise".into(),
            args: Some(vec!["exc".into()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_var".into(),
            var: Some("_bb7_arg0".into()),
            args: Some(vec!["total".into()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "check_exception".into(),
            value: Some(5),
            ..OpIR::default()
        },
        OpIR {
            kind: "jump".into(),
            value: Some(5),
            ..OpIR::default()
        },
        OpIR {
            kind: "label".into(),
            value: Some(5),
            args: Some(vec![]),
            ..OpIR::default()
        },
    ];

    eliminate_dead_labels(&mut ops);

    let kinds: Vec<&str> = ops.iter().map(|op| op.kind.as_str()).collect();
    assert_eq!(
        kinds,
        vec!["raise", "store_var", "check_exception", "jump", "label"],
        "raise must not delete the explicit exception transfer edge: {ops:?}"
    );
}

#[test]
fn eliminate_dead_labels_keeps_if_marker_after_dead_label_before_structured_if() {
    let mut ops = vec![
        OpIR {
            kind: "ret".into(),
            args: Some(vec!["_ret0".into()]),
            var: Some("_ret0".into()),
            ..OpIR::default()
        },
        OpIR {
            kind: "label".into(),
            value: Some(42),
            args: Some(vec![]),
            ..OpIR::default()
        },
        OpIR {
            kind: "if".into(),
            args: Some(vec!["cond".into()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "const_none".into(),
            out: Some("_v0".into()),
            ..OpIR::default()
        },
        OpIR {
            kind: "else".into(),
            ..OpIR::default()
        },
        OpIR {
            kind: "raise".into(),
            args: Some(vec!["exc".into()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "end_if".into(),
            ..OpIR::default()
        },
    ];

    eliminate_dead_labels(&mut ops);

    let kinds: Vec<&str> = ops.iter().map(|op| op.kind.as_str()).collect();
    assert_eq!(
        kinds,
        vec!["ret", "if", "const_none", "else", "raise", "end_if"],
        "dead-label elimination must not orphan structured if markers: {ops:?}"
    );
    assert!(
        validate_structured_if_markers(&ops).is_ok(),
        "structured if markers must remain balanced after dead-label elimination: {ops:?}"
    );
}

#[test]
fn validate_structured_if_markers_rejects_orphan_else() {
    let ops = vec![
        OpIR {
            kind: "ret".into(),
            ..OpIR::default()
        },
        OpIR {
            kind: "else".into(),
            ..OpIR::default()
        },
        OpIR {
            kind: "end_if".into(),
            ..OpIR::default()
        },
    ];

    let err = validate_structured_if_markers(&ops).expect_err("must reject orphan else");
    assert!(err.contains("orphan else"), "{err}");
}
