use super::*;

#[test]
fn structured_if_skips_join_with_external_predecessor() {
    let mut func = TirFunction::new(
        "branch_with_shared_join".into(),
        vec![TirType::Bool, TirType::Bool],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
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
        !ops.iter().any(|op| op.kind == molt_ir::OpKind::If
            || op.kind == molt_ir::OpKind::Else
            || op.kind == molt_ir::OpKind::EndIf),
        "shared-join lowering must stay label-based instead of inlining to structured if/else: {ops:?}"
    );
    assert!(
        ops.iter()
            .filter(|op| op.kind == molt_ir::OpKind::Label)
            .count()
            >= 4,
        "shared-join lowering must preserve explicit labels for the merge shape: {ops:?}"
    );
}

#[test]
fn structured_if_skips_arm_with_external_predecessor() {
    let mut func = TirFunction::new(
        "branch_with_shared_then_arm".into(),
        vec![TirType::Bool, TirType::Bool],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
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
        !ops.iter().any(|op| op.kind == molt_ir::OpKind::If
            || op.kind == molt_ir::OpKind::Else
            || op.kind == molt_ir::OpKind::EndIf),
        "shared-arm lowering must stay label-based instead of inlining to structured if/else: {ops:?}"
    );
    assert!(
        ops.iter()
            .filter(|op| op.kind == molt_ir::OpKind::Label)
            .count()
            >= 4,
        "shared-arm lowering must preserve explicit labels for the reused then-arm shape: {ops:?}"
    );
}

#[test]
fn structured_if_emits_join_arg_store_load_without_phi() {
    let mut func = TirFunction::new(
        "branch_with_join_arg".into(),
        vec![TirType::Bool],
        TirType::I64,
        molt_ir::FunctionReturnAbi::Value,
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
                origin: None,
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
                origin: None,
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
    let kinds: Vec<molt_ir::OpKind> = ops.iter().map(|op| op.kind).collect();

    assert!(kinds.contains(&molt_ir::OpKind::If), "{ops:?}");
    assert!(kinds.contains(&molt_ir::OpKind::Else), "{ops:?}");
    assert!(kinds.contains(&molt_ir::OpKind::EndIf), "{ops:?}");
    assert!(
        !kinds.contains(&molt_ir::OpKind::Phi),
        "structured if join args must round-trip as store/load, not phi: {ops:?}"
    );
    assert!(
        ops.iter()
            .filter(|op| op.kind == molt_ir::OpKind::StoreVar)
            .count()
            >= 2,
        "structured if join args must emit branch-site stores: {ops:?}"
    );
    assert!(
        ops.iter().any(|op| op.kind == molt_ir::OpKind::LoadVar),
        "structured if join args must reload the merged value after end_if: {ops:?}"
    );
}

#[test]
fn check_exception_materializes_handler_arg_stores() {
    let mut func = TirFunction::new(
        "check_exception_handler_args".into(),
        vec![],
        TirType::I64,
        molt_ir::FunctionReturnAbi::Value,
    );

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
        origin: None,
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![value],
        attrs: const_attrs,
        source_span: None,
    });
    entry.ops.push(TirOp {
        origin: None,
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
            op.kind == molt_ir::OpKind::StoreVar
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
            op.kind == molt_ir::OpKind::LoadVar
                && op.var.as_deref() == Some(handler_param.as_str())
                && op.out.as_deref() == Some(handler_value.as_str())
        }),
        "handler block must still reload its synthesized arg slot: {ops:?}"
    );
}

#[test]
fn try_start_materializes_handler_arg_stores() {
    let mut func = TirFunction::new(
        "try_start_handler_args".into(),
        vec![],
        TirType::I64,
        molt_ir::FunctionReturnAbi::Value,
    );

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
        origin: None,
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![value],
        attrs: const_attrs,
        source_span: None,
    });
    entry.ops.push(TirOp {
        origin: None,
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
            op.kind == molt_ir::OpKind::StoreVar
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
            op.kind == molt_ir::OpKind::LoadVar
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
        molt_ir::FunctionReturnAbi::Void,
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
        !ops.iter().any(|op| op.kind == molt_ir::OpKind::If
            || op.kind == molt_ir::OpKind::Else
            || op.kind == molt_ir::OpKind::EndIf),
        "mixed return/fallthrough shape must stay label-based until region analysis proves it safe: {ops:?}"
    );
}

#[test]
fn structured_if_skips_successor_with_nested_scf() {
    let mut func = TirFunction::new(
        "branch_with_nested_scf_successor".into(),
        vec![TirType::Bool],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
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
                origin: None,
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
        !ops.iter().any(|op| op.kind == molt_ir::OpKind::If
            || op.kind == molt_ir::OpKind::Else
            || op.kind == molt_ir::OpKind::EndIf),
        "successors containing nested SCF must stay label-based instead of inlining to structured if/else: {ops:?}"
    );
}

#[test]
fn structured_if_skips_successor_with_try_region_markers() {
    let mut func = TirFunction::new(
        "branch_with_try_region_successor".into(),
        vec![TirType::Bool],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );

    let then_blk = func.fresh_block();
    let else_blk = func.fresh_block();
    let join_blk = func.fresh_block();
    let handler_blk = func.fresh_block();
    func.has_exception_handling = true;

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
                    origin: None,
                    dialect: Dialect::Molt,
                    opcode: OpCode::TryStart,
                    operands: vec![],
                    results: vec![],
                    attrs: try_attrs.clone(),
                    source_span: None,
                },
                TirOp {
                    origin: None,
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
    func.label_id_map.insert(handler_blk.0, 100);
    func.blocks.insert(
        handler_blk,
        TirBlock {
            id: handler_blk,
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
        !ops.iter().any(|op| op.kind == molt_ir::OpKind::If
            || op.kind == molt_ir::OpKind::Else
            || op.kind == molt_ir::OpKind::EndIf),
        "successors containing try_start/try_end must stay label-based instead of inlining to structured if/else: {ops:?}"
    );
}

#[test]
fn structured_if_skips_join_that_is_loop_header() {
    let mut func = TirFunction::new(
        "branch_with_loop_header_join".into(),
        vec![TirType::Bool],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
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
        !ops.iter().any(|op| op.kind == molt_ir::OpKind::If
            || op.kind == molt_ir::OpKind::Else
            || op.kind == molt_ir::OpKind::EndIf),
        "join blocks that are loop headers must stay label-based instead of inlining to structured if/else: {ops:?}"
    );
}

#[test]
fn loop_end_block_target_must_keep_its_label() {
    let mut func = TirFunction::new(
        "loop_end_block_target_must_keep_its_label".into(),
        vec![],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
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
        ops.iter().any(|op| matches!(
            op.kind,
            molt_ir::OpKind::Label | molt_ir::OpKind::StateLabel
        ) && op.value.is_some()),
        "expected a materialized target label for the loop-end block: {ops:?}"
    );
}

#[test]
fn eliminate_dead_loop_end_after_return() {
    let mut ops = vec![
        OpIR {
            kind: molt_ir::OpKind::Ret,
            args: Some(vec!["_ret0".into()]),
            ..OpIR::new(molt_ir::OpKind::Ret)
        },
        OpIR {
            kind: molt_ir::OpKind::LoopEnd,
            args: Some(vec![]),
            ..OpIR::new(molt_ir::OpKind::LoopEnd)
        },
        OpIR {
            kind: molt_ir::OpKind::Label,
            args: Some(vec![]),
            value: Some(42),
            ..OpIR::new(molt_ir::OpKind::Label)
        },
    ];

    eliminate_dead_labels(&mut ops);

    assert!(
        !ops.iter().any(|op| op.kind == molt_ir::OpKind::LoopEnd),
        "dead loop_end must not survive after a real return: {ops:?}"
    );
}

#[test]
fn paired_loop_closers_survive_terminal_body_paths() {
    for terminal in [
        molt_ir::OpKind::Ret,
        molt_ir::OpKind::RetVoid,
        molt_ir::OpKind::Jump,
        molt_ir::OpKind::Goto,
        molt_ir::OpKind::LoopBreak,
        molt_ir::OpKind::LoopContinue,
    ] {
        for depth in 1..=2 {
            let mut ops = Vec::new();
            for _ in 0..depth {
                for kind in [
                    molt_ir::OpKind::LoopStart,
                    molt_ir::OpKind::LoopBreakIfFalse,
                ] {
                    ops.push(OpIR {
                        kind,
                        ..OpIR::new(kind)
                    });
                }
            }
            ops.push(OpIR {
                kind: terminal,
                value: matches!(terminal, molt_ir::OpKind::Jump | molt_ir::OpKind::Goto)
                    .then_some(42),
                ..OpIR::new(terminal)
            });
            ops.push(OpIR {
                kind: molt_ir::OpKind::ConstInt,
                out: Some("dead_body_value".into()),
                value: Some(1),
                ..OpIR::new(molt_ir::OpKind::ConstInt)
            });
            for _ in 0..depth {
                ops.push(OpIR {
                    kind: molt_ir::OpKind::LoopEnd,
                    ..OpIR::new(molt_ir::OpKind::LoopEnd)
                });
            }
            ops.push(OpIR {
                kind: molt_ir::OpKind::Label,
                value: Some(42),
                ..OpIR::new(molt_ir::OpKind::Label)
            });
            ops.push(OpIR {
                kind: molt_ir::OpKind::ConstInt,
                out: Some("after_loop_value".into()),
                value: Some(2),
                ..OpIR::new(molt_ir::OpKind::ConstInt)
            });

            eliminate_dead_labels(&mut ops);

            assert_eq!(
                ops.iter()
                    .filter(|op| op.kind == molt_ir::OpKind::LoopEnd)
                    .count(),
                depth,
                "{terminal} at depth {depth} must close every retained loop: {ops:?}"
            );
            assert!(
                !ops.iter()
                    .any(|op| op.out.as_deref() == Some("dead_body_value")),
                "{terminal} must kill straight-line body work: {ops:?}"
            );
            assert_eq!(ops.last().unwrap().out.as_deref(), Some("after_loop_value"));
            let mut errors = Vec::new();
            molt_ir::simple_verify::verify_simple_ir_structure("terminal_loop", &ops, &mut errors);
            assert!(errors.is_empty(), "{terminal} at depth {depth}: {errors:?}");
        }
    }
}

#[test]
fn eliminate_dead_jump_after_return() {
    let mut ops = vec![
        OpIR {
            kind: molt_ir::OpKind::Ret,
            args: Some(vec!["_ret0".into()]),
            ..OpIR::new(molt_ir::OpKind::Ret)
        },
        OpIR {
            kind: molt_ir::OpKind::Jump,
            value: Some(42),
            ..OpIR::new(molt_ir::OpKind::Jump)
        },
        OpIR {
            kind: molt_ir::OpKind::Label,
            args: Some(vec![]),
            value: Some(42),
            ..OpIR::new(molt_ir::OpKind::Label)
        },
    ];

    eliminate_dead_labels(&mut ops);

    assert!(
        !ops.iter().any(|op| op.kind == molt_ir::OpKind::Jump),
        "dead jump must not survive after a real return: {ops:?}"
    );
}

#[test]
fn preserve_loop_end_after_live_labeled_raise_path() {
    let mut ops = vec![
        OpIR {
            kind: molt_ir::OpKind::BrIf,
            args: Some(vec!["cond".into()]),
            value: Some(7),
            ..OpIR::new(molt_ir::OpKind::BrIf)
        },
        OpIR {
            kind: molt_ir::OpKind::LoopContinue,
            ..OpIR::new(molt_ir::OpKind::LoopContinue)
        },
        OpIR {
            kind: molt_ir::OpKind::Label,
            value: Some(7),
            args: Some(vec![]),
            ..OpIR::new(molt_ir::OpKind::Label)
        },
        OpIR {
            kind: molt_ir::OpKind::Raise,
            args: Some(vec!["exc".into()]),
            ..OpIR::new(molt_ir::OpKind::Raise)
        },
        OpIR {
            kind: molt_ir::OpKind::LoopEnd,
            args: Some(vec![]),
            ..OpIR::new(molt_ir::OpKind::LoopEnd)
        },
    ];

    eliminate_dead_labels(&mut ops);

    assert!(
        ops.iter().any(|op| op.kind == molt_ir::OpKind::LoopEnd),
        "loop_end must survive after a live labeled terminal block because it still closes the structured loop break path: {ops:?}"
    );
}

#[test]
fn eliminate_dead_labels_preserves_explicit_post_raise_exception_transfer() {
    let mut ops = vec![
        OpIR {
            kind: molt_ir::OpKind::Raise,
            args: Some(vec!["exc".into()]),
            ..OpIR::new(molt_ir::OpKind::Raise)
        },
        OpIR {
            kind: molt_ir::OpKind::StoreVar,
            var: Some("_bb7_arg0".into()),
            args: Some(vec!["total".into()]),
            ..OpIR::new(molt_ir::OpKind::StoreVar)
        },
        OpIR {
            kind: molt_ir::OpKind::CheckException,
            value: Some(5),
            ..OpIR::new(molt_ir::OpKind::CheckException)
        },
        OpIR {
            kind: molt_ir::OpKind::Jump,
            value: Some(5),
            ..OpIR::new(molt_ir::OpKind::Jump)
        },
        OpIR {
            kind: molt_ir::OpKind::Label,
            value: Some(5),
            args: Some(vec![]),
            ..OpIR::new(molt_ir::OpKind::Label)
        },
    ];

    eliminate_dead_labels(&mut ops);

    let kinds: Vec<molt_ir::OpKind> = ops.iter().map(|op| op.kind).collect();
    assert_eq!(
        kinds,
        vec![
            molt_ir::OpKind::Raise,
            molt_ir::OpKind::StoreVar,
            molt_ir::OpKind::CheckException,
            molt_ir::OpKind::Jump,
            molt_ir::OpKind::Label
        ],
        "raise must not delete the explicit exception transfer edge: {ops:?}"
    );
}

#[test]
fn eliminate_dead_labels_keeps_if_marker_after_dead_label_before_structured_if() {
    let mut ops = vec![
        OpIR {
            kind: molt_ir::OpKind::Ret,
            args: Some(vec!["_ret0".into()]),
            ..OpIR::new(molt_ir::OpKind::Ret)
        },
        OpIR {
            kind: molt_ir::OpKind::Label,
            value: Some(42),
            args: Some(vec![]),
            ..OpIR::new(molt_ir::OpKind::Label)
        },
        OpIR {
            kind: molt_ir::OpKind::If,
            args: Some(vec!["cond".into()]),
            ..OpIR::new(molt_ir::OpKind::If)
        },
        OpIR {
            kind: molt_ir::OpKind::ConstNone,
            out: Some("_v0".into()),
            ..OpIR::new(molt_ir::OpKind::ConstNone)
        },
        OpIR {
            kind: molt_ir::OpKind::Else,
            ..OpIR::new(molt_ir::OpKind::Else)
        },
        OpIR {
            kind: molt_ir::OpKind::Raise,
            args: Some(vec!["exc".into()]),
            ..OpIR::new(molt_ir::OpKind::Raise)
        },
        OpIR {
            kind: molt_ir::OpKind::EndIf,
            ..OpIR::new(molt_ir::OpKind::EndIf)
        },
    ];

    eliminate_dead_labels(&mut ops);

    let kinds: Vec<molt_ir::OpKind> = ops.iter().map(|op| op.kind).collect();
    assert_eq!(
        kinds,
        vec![
            molt_ir::OpKind::Ret,
            molt_ir::OpKind::If,
            molt_ir::OpKind::ConstNone,
            molt_ir::OpKind::Else,
            molt_ir::OpKind::Raise,
            molt_ir::OpKind::EndIf
        ],
        "dead-label elimination must not orphan structured if markers: {ops:?}"
    );
    let mut errors = Vec::new();
    molt_ir::simple_verify::verify_simple_ir_structure("terminal_if", &ops, &mut errors);
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn shared_structure_verifier_rejects_orphaned_and_crossed_regions() {
    for kinds in [
        vec![
            molt_ir::OpKind::Ret,
            molt_ir::OpKind::Else,
            molt_ir::OpKind::EndIf,
        ],
        vec![molt_ir::OpKind::LoopStart, molt_ir::OpKind::Ret],
        vec![molt_ir::OpKind::LoopEnd],
        vec![
            molt_ir::OpKind::If,
            molt_ir::OpKind::LoopStart,
            molt_ir::OpKind::EndIf,
            molt_ir::OpKind::LoopEnd,
        ],
    ] {
        let ops: Vec<_> = kinds
            .iter()
            .map(|kind| OpIR {
                kind: (*kind),
                ..OpIR::new(*kind)
            })
            .collect();
        let mut errors = Vec::new();
        molt_ir::simple_verify::verify_simple_ir_structure("invalid_regions", &ops, &mut errors);
        assert!(
            errors
                .iter()
                .any(|error| error.kind == "unbalanced-control-flow"),
            "invalid lexical nesting must be rejected: {kinds:?}, {errors:?}"
        );
    }
}

#[test]
fn terminal_structured_if_roundtrips_without_a_fabricated_return() {
    let mut func = TirFunction::new(
        "two_terminal_arms".into(),
        vec![TirType::Bool, TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    func.param_names = vec!["condition".into(), "left".into(), "right".into()];
    let entry = func.entry_block;
    let then_block = func.fresh_block();
    let else_block = func.fresh_block();
    let parameters: Vec<_> = func.blocks[&entry].args.iter().map(|arg| arg.id).collect();
    func.blocks.get_mut(&entry).unwrap().terminator = Terminator::CondBranch {
        cond: parameters[0],
        then_block,
        then_args: vec![],
        else_block,
        else_args: vec![],
    };
    for (id, value) in [(then_block, parameters[1]), (else_block, parameters[2])] {
        func.blocks.insert(
            id,
            TirBlock {
                id,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return {
                    values: vec![value],
                },
            },
        );
    }
    let ops = lower_to_simple_ir(&func);
    assert_eq!(ops.last().unwrap().kind, molt_ir::OpKind::EndIf, "{ops:?}");
    assert_eq!(
        ops.iter()
            .filter(|op| op.kind == molt_ir::OpKind::Ret)
            .count(),
        2
    );
    assert!(!ops.iter().any(|op| op.kind == molt_ir::OpKind::RetVoid));
    let document = molt_ir::SimpleIR {
        functions: vec![FunctionIR {
            name: func.name.clone(),
            params: func.param_names.clone(),
            return_abi: func.return_abi,
            ops,
            ..FunctionIR::default()
        }],
        profile: None,
    };
    molt_ir::validate_simple_ir(&document).expect("lowered wire contract");
    let report = molt_ir::verify_simple_ir(&document);
    assert!(report.is_ok(), "{report:?}");
    let lifted = lower_to_tir(&document.functions[0]);
    let entry = &lifted.blocks[&lifted.entry_block];
    let Terminator::CondBranch {
        cond,
        then_block,
        else_block,
        ..
    } = entry.terminator
    else {
        panic!("expected preserved branch: {:?}", entry.terminator);
    };
    assert_eq!(cond, entry.args[0].id);
    for (id, parameter) in [(then_block, 1), (else_block, 2)] {
        let Terminator::Return { ref values } = lifted.blocks[&id].terminator else {
            panic!("expected preserved arm return");
        };
        assert_eq!(
            values,
            &[entry.args[parameter].id],
            "branch polarity selects its original parameter"
        );
    }
}

#[test]
fn structured_terminal_arms_preserve_generic_unreachable_emission() {
    for suspend in [false, true] {
        let mut func = TirFunction::new(
            "terminal_arm".into(),
            vec![TirType::Bool, TirType::DynBox],
            TirType::None,
            molt_ir::FunctionReturnAbi::Void,
        );
        let entry = func.entry_block;
        let then_block = func.fresh_block();
        let else_block = func.fresh_block();
        let flag = func.blocks[&entry].args[0].id;
        let value = func.blocks[&entry].args[1].id;
        func.blocks.get_mut(&entry).unwrap().terminator = Terminator::CondBranch {
            cond: flag,
            then_block,
            then_args: vec![],
            else_block,
            else_args: vec![],
        };
        func.blocks.insert(
            then_block,
            TirBlock {
                id: then_block,
                args: vec![],
                ops: if suspend {
                    vec![TirOp {
                        origin: None,
                        dialect: Dialect::Molt,
                        opcode: OpCode::StateYield,
                        operands: vec![value],
                        results: vec![],
                        attrs: AttrDict::from([("value".into(), AttrValue::Int(7))]),
                        source_span: None,
                    }]
                } else {
                    vec![]
                },
                terminator: Terminator::Unreachable,
            },
        );
        func.blocks.insert(
            else_block,
            TirBlock {
                id: else_block,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        let ops = lower_to_simple_ir(&func);
        assert_eq!(ops.last().unwrap().kind, molt_ir::OpKind::EndIf);
        assert_eq!(
            ops.iter()
                .filter(|op| op.kind == molt_ir::OpKind::Unreachable)
                .count(),
            usize::from(!suspend)
        );
        assert_eq!(
            ops.iter()
                .filter(|op| op.kind == molt_ir::OpKind::StateYield)
                .count(),
            usize::from(suspend)
        );
        assert_eq!(
            ops.iter()
                .filter(|op| op.kind == molt_ir::OpKind::RetVoid)
                .count(),
            1
        );
        // This asserts transport preservation only. An unreachable opcode is
        // not thereby admitted as a supported terminal on every backend.
    }
}
