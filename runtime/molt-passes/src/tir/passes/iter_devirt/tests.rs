use super::run;
use super::transform::make_const_int;
use crate::tir::blocks::{BlockId, LoopBreakKind, LoopRole, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

fn make_op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands,
        results,
        attrs: AttrDict::new(),
        source_span: None,
    }
}

fn make_op_with_container(
    opcode: OpCode,
    operands: Vec<ValueId>,
    results: Vec<ValueId>,
    container_type: &str,
) -> TirOp {
    let mut attrs = AttrDict::new();
    attrs.insert(
        "container_type".to_string(),
        AttrValue::Str(container_type.to_string()),
    );
    TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands,
        results,
        attrs,
        source_span: None,
    }
}

/// Build a function matching `for x in some_list: body_op(x)`.
///
/// TIR layout:
///   bb0 (entry): BuildList(...), GetIter(list), Branch -> bb1
///   bb1 (header): IterNextUnboxed(iter) -> (elem, done), CondBranch(done, bb3, bb2)
///   bb2 (body): some_op(elem), Branch -> bb1
///   bb3 (exit): Return
fn build_list_for_loop(use_build_list: bool) -> TirFunction {
    let mut func = TirFunction::new("test_list_iter".into(), vec![], TirType::None);

    let list_val = func.fresh_value();
    let iter_val = func.fresh_value();

    let mut entry_ops = Vec::new();

    if use_build_list {
        // Create list via BuildList.
        let elem_a = func.fresh_value();
        let elem_b = func.fresh_value();
        entry_ops.push(make_const_int(elem_a, 1));
        entry_ops.push(make_const_int(elem_b, 2));
        entry_ops.push(make_op(
            OpCode::BuildList,
            vec![elem_a, elem_b],
            vec![list_val],
        ));
    } else {
        // Simulate a list from a call with container_type annotation.
        // Use a dummy operand so the verifier accepts the CallBuiltin.
        let dummy = func.fresh_value();
        entry_ops.push(make_const_int(dummy, 0));
        let mut attrs = AttrDict::new();
        attrs.insert("name".to_string(), AttrValue::Str("get_data".to_string()));
        attrs.insert(
            "container_type".to_string(),
            AttrValue::Str("list".to_string()),
        );
        entry_ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::CallBuiltin,
            operands: vec![dummy],
            results: vec![list_val],
            attrs,
            source_span: None,
        });
    }

    entry_ops.push(make_op(OpCode::GetIter, vec![list_val], vec![iter_val]));

    let header_id = func.fresh_block();
    let body_id = func.fresh_block();
    let exit_id = func.fresh_block();

    // Patch entry block.
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = entry_ops;
        entry.terminator = Terminator::Branch {
            target: header_id,
            args: vec![],
        };
    }

    // Header block: IterNextUnboxed.
    let elem_val = func.fresh_value();
    let done_val = func.fresh_value();

    let header_block = TirBlock {
        id: header_id,
        args: vec![],
        ops: vec![make_op(
            OpCode::IterNextUnboxed,
            vec![iter_val],
            vec![elem_val, done_val],
        )],
        terminator: Terminator::CondBranch {
            cond: done_val,
            then_block: exit_id,
            then_args: vec![],
            else_block: body_id,
            else_args: vec![],
        },
    };
    func.blocks.insert(header_id, header_block);
    func.loop_roles.insert(header_id, LoopRole::LoopHeader);

    // Body block: use elem, branch back.
    let body_result = func.fresh_value();
    let body_block = TirBlock {
        id: body_id,
        args: vec![],
        ops: vec![make_op(
            OpCode::Add,
            vec![elem_val, elem_val],
            vec![body_result],
        )],
        terminator: Terminator::Branch {
            target: header_id,
            args: vec![],
        },
    };
    func.blocks.insert(body_id, body_block);

    // Exit block.
    let exit_block = TirBlock {
        id: exit_id,
        args: vec![],
        ops: vec![],
        terminator: Terminator::Return { values: vec![] },
    };
    func.blocks.insert(exit_id, exit_block);
    func.loop_roles.insert(exit_id, LoopRole::LoopEnd);

    func
}

#[test]
fn devirt_list_from_build_list() {
    let mut func = build_list_for_loop(true);
    let stats = run(&mut func);

    // Should have transformed.
    assert!(
        stats.ops_added > 0 || stats.values_changed > 0,
        "pass should have transformed the list loop"
    );

    // Header should no longer contain IterNextUnboxed.
    let header_id = BlockId(1);
    let header = &func.blocks[&header_id];
    assert!(
        !header
            .ops
            .iter()
            .any(|op| op.opcode == OpCode::IterNextUnboxed),
        "IterNextUnboxed should be replaced"
    );

    // Header should contain Lt comparison.
    assert!(
        header.ops.iter().any(|op| op.opcode == OpCode::Lt),
        "header should have Lt comparison"
    );

    // Header should have a block argument (index variable).
    assert_eq!(header.args.len(), 1, "header should have index var arg");
    assert_eq!(header.args[0].ty, TirType::I64);
    assert_eq!(
        func.value_types.get(&header.args[0].id),
        Some(&TirType::I64),
        "index block arg must be mirrored into function value types"
    );
    let cmp_result = header
        .ops
        .iter()
        .find(|op| op.opcode == OpCode::Lt)
        .and_then(|op| op.results.first())
        .copied()
        .expect("list devirt comparison result");
    assert_eq!(
        func.value_types.get(&cmp_result),
        Some(&TirType::Bool),
        "list-loop comparison result must carry a bool type fact"
    );

    // Entry block should not have GetIter.
    let entry = &func.blocks[&BlockId(0)];
    assert!(
        !entry.ops.iter().any(|op| op.opcode == OpCode::GetIter),
        "GetIter should be replaced with len"
    );

    // Entry block should have CallBuiltin("len").
    assert!(
        entry.ops.iter().any(|op| {
            op.opcode == OpCode::CallBuiltin
                && op.attrs.get("name") == Some(&AttrValue::Str("len".to_string()))
        }),
        "entry should have CallBuiltin('len')"
    );

    // Body block should have Index op at position 0.
    let body_id = BlockId(2);
    let body = &func.blocks[&body_id];
    assert_eq!(
        body.ops[0].opcode,
        OpCode::Index,
        "body should start with Index op"
    );

    // Body should have Add for the index increment.
    let add_count = body
        .ops
        .iter()
        .filter(|op| op.opcode == OpCode::Add)
        .count();
    assert_eq!(
        add_count, 2,
        "body should have original Add + increment Add"
    );
    let increment_result = body
        .ops
        .iter()
        .rev()
        .find(|op| op.opcode == OpCode::Add)
        .and_then(|op| op.results.first())
        .copied()
        .expect("list index increment result");
    assert_eq!(
        func.value_types.get(&increment_result),
        Some(&TirType::I64),
        "list index increment result must carry an i64 type fact"
    );

    // The body's branch back to header should carry the next index value.
    if let Terminator::Branch { target, args } = &body.terminator {
        assert_eq!(*target, header_id);
        assert_eq!(args.len(), 1, "back-edge should carry incremented value");
    } else {
        panic!("body should branch to header");
    }

    // Verify function passes TIR verification.
    crate::tir::verify::verify_function(&func).expect("verification should pass");
}

#[test]
fn no_devirt_from_container_type_only() {
    let mut func = build_list_for_loop(false);
    let stats = run(&mut func);

    assert_eq!(
        stats.values_changed, 0,
        "transport-only container_type=list must not prove list iteration"
    );
    assert!(
        func.blocks[&BlockId(1)]
            .ops
            .iter()
            .any(|op| op.opcode == OpCode::IterNextUnboxed),
        "transport-only list metadata must leave iterator protocol intact"
    );

    crate::tir::verify::verify_function(&func).expect("verification should pass");
}

#[test]
fn devirt_list_from_function_value_type() {
    let mut func = build_list_for_loop(false);
    func.value_types
        .insert(ValueId(0), TirType::List(Box::new(TirType::DynBox)));
    let stats = run(&mut func);

    assert!(
        stats.values_changed > 0,
        "function-owned TirType::List fact should transform list loop"
    );

    let header = &func.blocks[&BlockId(1)];
    let cmp_op = header
        .ops
        .iter()
        .find(|op| op.opcode == OpCode::Lt)
        .expect("typed-list devirt should synthesize Lt compare");
    assert!(
        !cmp_op.attrs.contains_key("_fast_int"),
        "list compare must use value_types for scalar proof, not _fast_int attrs"
    );
    let cmp_result = cmp_op
        .results
        .first()
        .copied()
        .expect("list compare result");
    assert_eq!(
        func.value_types.get(&cmp_result),
        Some(&TirType::Bool),
        "list compare result must carry a bool type fact"
    );
    let entry = &func.blocks[&func.entry_block];
    let len_op = entry
        .ops
        .iter()
        .find(|op| op.opcode == OpCode::CallBuiltin)
        .expect("typed-list devirt should synthesize len call");
    assert!(
        !len_op.attrs.contains_key("_fast_int"),
        "synthesized len must use value_types for scalar proof, not _fast_int attrs"
    );
    let body = &func.blocks[&BlockId(2)];
    let index_op = body
        .ops
        .iter()
        .find(|op| op.opcode == OpCode::Index)
        .expect("typed-list devirt should synthesize Index");
    assert_eq!(
        index_op.attrs.get("container_type"),
        Some(&AttrValue::Str("list".to_string())),
        "synthesized Index should carry semantic list metadata derived from the typed proof"
    );
    let increment_op = body
        .ops
        .iter()
        .rev()
        .find(|op| op.opcode == OpCode::Add)
        .expect("typed-list devirt should synthesize index increment");
    assert!(
        !increment_op.attrs.contains_key("_fast_int"),
        "list increment must use value_types for scalar proof, not _fast_int attrs"
    );
    let increment_result = increment_op
        .results
        .first()
        .copied()
        .expect("list increment result");
    assert_eq!(
        func.value_types.get(&increment_result),
        Some(&TirType::I64),
        "list increment result must carry an i64 type fact"
    );

    crate::tir::verify::verify_function(&func).expect("verification should pass");
}

#[test]
fn no_devirt_from_legacy_list_int_container_type() {
    let mut func = build_list_for_loop(false);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    let source_op = entry
        .ops
        .iter_mut()
        .find(|op| op.opcode == OpCode::CallBuiltin)
        .expect("source op exists");
    source_op.attrs.insert(
        "container_type".to_string(),
        AttrValue::Str("list_int".to_string()),
    );

    let stats = run(&mut func);

    assert_eq!(
        stats.values_changed, 0,
        "legacy flat-list storage must not be accepted as semantic container metadata"
    );
    crate::tir::verify::verify_function(&func).expect("verification should pass");
}

#[test]
fn no_devirt_from_get_iter_container_type_only() {
    // Transport-only container_type on the GetIter op itself is not proof.
    let mut func = TirFunction::new("test".into(), vec![TirType::DynBox], TirType::None);

    let param = ValueId(0);
    let iter_val = func.fresh_value();

    let header_id = func.fresh_block();
    let body_id = func.fresh_block();
    let exit_id = func.fresh_block();

    // Entry: GetIter with container_type="list" on param (not BuildList).
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = vec![make_op_with_container(
            OpCode::GetIter,
            vec![param],
            vec![iter_val],
            "list",
        )];
        entry.terminator = Terminator::Branch {
            target: header_id,
            args: vec![],
        };
    }

    let elem_val = func.fresh_value();
    let done_val = func.fresh_value();
    let header = TirBlock {
        id: header_id,
        args: vec![],
        ops: vec![make_op(
            OpCode::IterNextUnboxed,
            vec![iter_val],
            vec![elem_val, done_val],
        )],
        terminator: Terminator::CondBranch {
            cond: done_val,
            then_block: exit_id,
            then_args: vec![],
            else_block: body_id,
            else_args: vec![],
        },
    };
    func.blocks.insert(header_id, header);
    func.loop_roles.insert(header_id, LoopRole::LoopHeader);

    let body = TirBlock {
        id: body_id,
        args: vec![],
        ops: vec![],
        terminator: Terminator::Branch {
            target: header_id,
            args: vec![],
        },
    };
    func.blocks.insert(body_id, body);

    let exit = TirBlock {
        id: exit_id,
        args: vec![],
        ops: vec![],
        terminator: Terminator::Return { values: vec![] },
    };
    func.blocks.insert(exit_id, exit);
    func.loop_roles.insert(exit_id, LoopRole::LoopEnd);

    let stats = run(&mut func);
    assert_eq!(
        stats.values_changed, 0,
        "GetIter container_type=list must not prove list iteration"
    );
    assert!(
        func.blocks[&header_id]
            .ops
            .iter()
            .any(|op| op.opcode == OpCode::IterNextUnboxed),
        "transport-only GetIter metadata must leave iterator protocol intact"
    );
}

#[test]
fn devirt_list_from_typed_param() {
    let mut func = TirFunction::new(
        "test".into(),
        vec![TirType::List(Box::new(TirType::DynBox))],
        TirType::None,
    );

    let param = ValueId(0);
    let iter_val = func.fresh_value();

    let header_id = func.fresh_block();
    let body_id = func.fresh_block();
    let exit_id = func.fresh_block();

    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = vec![make_op(OpCode::GetIter, vec![param], vec![iter_val])];
        entry.terminator = Terminator::Branch {
            target: header_id,
            args: vec![],
        };
    }

    let elem_val = func.fresh_value();
    let done_val = func.fresh_value();
    let header = TirBlock {
        id: header_id,
        args: vec![],
        ops: vec![make_op(
            OpCode::IterNextUnboxed,
            vec![iter_val],
            vec![elem_val, done_val],
        )],
        terminator: Terminator::CondBranch {
            cond: done_val,
            then_block: exit_id,
            then_args: vec![],
            else_block: body_id,
            else_args: vec![],
        },
    };
    func.blocks.insert(header_id, header);
    func.loop_roles.insert(header_id, LoopRole::LoopHeader);

    let body = TirBlock {
        id: body_id,
        args: vec![],
        ops: vec![],
        terminator: Terminator::Branch {
            target: header_id,
            args: vec![],
        },
    };
    func.blocks.insert(body_id, body);

    let exit = TirBlock {
        id: exit_id,
        args: vec![],
        ops: vec![],
        terminator: Terminator::Return { values: vec![] },
    };
    func.blocks.insert(exit_id, exit);
    func.loop_roles.insert(exit_id, LoopRole::LoopEnd);

    let stats = run(&mut func);
    assert!(
        stats.values_changed > 0,
        "typed list parameter should devirtualize without container_type metadata"
    );

    crate::tir::verify::verify_function(&func).expect("verification should pass");
}

#[test]
fn no_devirt_non_list_loop() {
    // A loop with GetIter on a non-list source should not be transformed.
    let mut func = TirFunction::new("test".into(), vec![TirType::DynBox], TirType::None);

    let param = ValueId(0);
    let iter_val = func.fresh_value();

    let header_id = func.fresh_block();
    let body_id = func.fresh_block();
    let exit_id = func.fresh_block();

    // Entry: GetIter on parameter (not list, no container_type).
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = vec![make_op(OpCode::GetIter, vec![param], vec![iter_val])];
        entry.terminator = Terminator::Branch {
            target: header_id,
            args: vec![],
        };
    }

    let elem_val = func.fresh_value();
    let done_val = func.fresh_value();
    let header = TirBlock {
        id: header_id,
        args: vec![],
        ops: vec![make_op(
            OpCode::IterNextUnboxed,
            vec![iter_val],
            vec![elem_val, done_val],
        )],
        terminator: Terminator::CondBranch {
            cond: done_val,
            then_block: exit_id,
            then_args: vec![],
            else_block: body_id,
            else_args: vec![],
        },
    };
    func.blocks.insert(header_id, header);
    func.loop_roles.insert(header_id, LoopRole::LoopHeader);

    let body = TirBlock {
        id: body_id,
        args: vec![],
        ops: vec![],
        terminator: Terminator::Branch {
            target: header_id,
            args: vec![],
        },
    };
    func.blocks.insert(body_id, body);

    let exit = TirBlock {
        id: exit_id,
        args: vec![],
        ops: vec![],
        terminator: Terminator::Return { values: vec![] },
    };
    func.blocks.insert(exit_id, exit);

    let stats = run(&mut func);
    assert_eq!(
        stats.ops_removed, 0,
        "non-list loop should not be transformed"
    );
    assert_eq!(stats.values_changed, 0);
}

#[test]
fn devirt_preserves_loop_break_kind() {
    let mut func = build_list_for_loop(true);
    run(&mut func);

    // After devirt, the loop should have BreakIfFalse.
    assert_eq!(
        func.loop_break_kinds.get(&BlockId(1)),
        Some(&LoopBreakKind::BreakIfFalse)
    );
}

#[test]
fn no_devirt_dict_with_container_type() {
    // A loop with GetIter on a dict should not be transformed.
    let mut func = TirFunction::new("test".into(), vec![], TirType::None);

    let dict_val = func.fresh_value();
    let iter_val = func.fresh_value();

    let header_id = func.fresh_block();
    let body_id = func.fresh_block();
    let exit_id = func.fresh_block();

    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = vec![
            make_op(OpCode::BuildDict, vec![], vec![dict_val]),
            make_op_with_container(OpCode::GetIter, vec![dict_val], vec![iter_val], "dict"),
        ];
        entry.terminator = Terminator::Branch {
            target: header_id,
            args: vec![],
        };
    }

    let elem_val = func.fresh_value();
    let done_val = func.fresh_value();
    let header = TirBlock {
        id: header_id,
        args: vec![],
        ops: vec![make_op(
            OpCode::IterNextUnboxed,
            vec![iter_val],
            vec![elem_val, done_val],
        )],
        terminator: Terminator::CondBranch {
            cond: done_val,
            then_block: exit_id,
            then_args: vec![],
            else_block: body_id,
            else_args: vec![],
        },
    };
    func.blocks.insert(header_id, header);
    func.loop_roles.insert(header_id, LoopRole::LoopHeader);

    let body = TirBlock {
        id: body_id,
        args: vec![],
        ops: vec![],
        terminator: Terminator::Branch {
            target: header_id,
            args: vec![],
        },
    };
    func.blocks.insert(body_id, body);

    let exit = TirBlock {
        id: exit_id,
        args: vec![],
        ops: vec![],
        terminator: Terminator::Return { values: vec![] },
    };
    func.blocks.insert(exit_id, exit);

    let stats = run(&mut func);
    assert_eq!(
        stats.values_changed, 0,
        "dict loop should not be transformed"
    );
}

#[test]
fn devirt_list_repeat_mul_build_list() {
    // `for x in [True] * n` should be devirtualized.
    // Source: Mul(BuildList([True]), n) — recognized as a list via
    // is_list_source tracing through Mul to BuildList.
    let mut func = TirFunction::new("test_mul_list".into(), vec![], TirType::None);

    let true_val = func.fresh_value();
    let list_1 = func.fresh_value();
    let n = func.fresh_value();
    let is_prime = func.fresh_value();
    let iter_val = func.fresh_value();

    let header_id = func.fresh_block();
    let body_id = func.fresh_block();
    let exit_id = func.fresh_block();

    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = vec![
            make_const_int(true_val, 1),
            make_op(OpCode::BuildList, vec![true_val], vec![list_1]),
            make_const_int(n, 100),
            make_op(OpCode::Mul, vec![list_1, n], vec![is_prime]),
            make_op(OpCode::GetIter, vec![is_prime], vec![iter_val]),
        ];
        entry.terminator = Terminator::Branch {
            target: header_id,
            args: vec![],
        };
    }

    let elem_val = func.fresh_value();
    let done_val = func.fresh_value();
    let header = TirBlock {
        id: header_id,
        args: vec![],
        ops: vec![make_op(
            OpCode::IterNextUnboxed,
            vec![iter_val],
            vec![elem_val, done_val],
        )],
        terminator: Terminator::CondBranch {
            cond: done_val,
            then_block: exit_id,
            then_args: vec![],
            else_block: body_id,
            else_args: vec![],
        },
    };
    func.blocks.insert(header_id, header);
    func.loop_roles.insert(header_id, LoopRole::LoopHeader);

    let body = TirBlock {
        id: body_id,
        args: vec![],
        ops: vec![],
        terminator: Terminator::Branch {
            target: header_id,
            args: vec![],
        },
    };
    func.blocks.insert(body_id, body);

    let exit = TirBlock {
        id: exit_id,
        args: vec![],
        ops: vec![],
        terminator: Terminator::Return { values: vec![] },
    };
    func.blocks.insert(exit_id, exit);
    func.loop_roles.insert(exit_id, LoopRole::LoopEnd);

    let stats = run(&mut func);
    assert!(
        stats.values_changed > 0,
        "Mul(BuildList, count) should be recognized as a list for iter_devirt"
    );

    // The body should now contain an Index op with container_type="list".
    let body_block = &func.blocks[&body_id];
    let index_op = body_block.ops.iter().find(|op| op.opcode == OpCode::Index);
    assert!(index_op.is_some(), "Body must contain synthesized Index op");
    let idx_op = index_op.unwrap();
    assert_eq!(
        idx_op.attrs.get("container_type"),
        Some(&AttrValue::Str("list".to_string())),
        "Synthesized Index must carry container_type=list"
    );

    crate::tir::verify::verify_function(&func).expect("verification should pass");
}
