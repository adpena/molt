use crate::tir::blocks::{BlockId, LoopBreakKind, LoopRole, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

use super::run;
use super::transform::make_const_int;

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

fn make_call_builtin(name: &str, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
    let mut attrs = AttrDict::new();
    attrs.insert("name".to_string(), AttrValue::Str(name.to_string()));
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::CallBuiltin,
        operands,
        results,
        attrs,
        source_span: None,
    }
}

fn make_const(result: ValueId, value: i64) -> TirOp {
    make_const_int(result, value)
}

fn build_range_for_loop(range_args: &[i64]) -> TirFunction {
    let mut func = TirFunction::new("test_range".into(), vec![], TirType::None);

    let mut range_arg_vals = Vec::new();
    let mut entry_ops = Vec::new();

    for &arg in range_args {
        let val = func.fresh_value();
        entry_ops.push(make_const(val, arg));
        range_arg_vals.push(val);
    }

    let range_obj = func.fresh_value();
    entry_ops.push(make_call_builtin(
        "range",
        range_arg_vals.clone(),
        vec![range_obj],
    ));

    let iter_val = func.fresh_value();
    entry_ops.push(make_op(OpCode::GetIter, vec![range_obj], vec![iter_val]));

    let header_id = func.fresh_block();
    let body_id = func.fresh_block();
    let exit_id = func.fresh_block();

    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = entry_ops;
        entry.terminator = Terminator::Branch {
            target: header_id,
            args: vec![],
        };
    }

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
fn devirt_range_single_arg() {
    let mut func = build_range_for_loop(&[10]);
    let stats = run(&mut func);

    assert!(
        stats.ops_removed > 0 || stats.ops_added > 0 || stats.values_changed > 0,
        "pass should have transformed the range loop"
    );

    let header_id = BlockId(1);
    let header = &func.blocks[&header_id];
    assert!(
        !header
            .ops
            .iter()
            .any(|op| op.opcode == OpCode::IterNextUnboxed),
        "IterNextUnboxed should be replaced"
    );

    assert!(
        header.ops.iter().any(|op| op.opcode == OpCode::Lt),
        "header should have Lt comparison"
    );

    assert_eq!(header.args.len(), 1, "header should have induction var arg");
    assert_eq!(header.args[0].ty, TirType::I64);
    assert_eq!(
        func.value_types.get(&header.args[0].id),
        Some(&TirType::I64),
        "induction block arg must be mirrored into function value types"
    );
    let cmp_op = header
        .ops
        .iter()
        .find(|op| op.opcode == OpCode::Lt)
        .expect("range devirt comparison op");
    assert!(
        !cmp_op.attrs.contains_key("_fast_int"),
        "range comparison must use value_types for scalar proof, not _fast_int attrs"
    );
    let cmp_result = cmp_op
        .results
        .first()
        .copied()
        .expect("range devirt comparison result");
    assert_eq!(
        func.value_types.get(&cmp_result),
        Some(&TirType::Bool),
        "range comparison result must carry a bool type fact"
    );

    let entry = &func.blocks[&BlockId(0)];
    assert!(
        !entry.ops.iter().any(|op| op.opcode == OpCode::CallBuiltin),
        "CallBuiltin('range') should be removed"
    );
    assert!(
        !entry.ops.iter().any(|op| op.opcode == OpCode::GetIter),
        "GetIter should be removed"
    );

    let body_id = BlockId(2);
    let body = &func.blocks[&body_id];
    let add_count = body
        .ops
        .iter()
        .filter(|op| op.opcode == OpCode::Add)
        .count();
    assert_eq!(
        add_count, 2,
        "body should have original Add + increment Add"
    );
    let increment_op = body
        .ops
        .iter()
        .rev()
        .find(|op| op.opcode == OpCode::Add)
        .expect("range increment op");
    assert!(
        !increment_op.attrs.contains_key("_fast_int"),
        "range increment must use value_types for scalar proof, not _fast_int attrs"
    );
    let increment_result = increment_op
        .results
        .first()
        .copied()
        .expect("range increment result");
    assert_eq!(
        func.value_types.get(&increment_result),
        Some(&TirType::I64),
        "range increment result must carry an i64 type fact"
    );

    if let Terminator::Branch { target, args } = &body.terminator {
        assert_eq!(*target, header_id);
        assert_eq!(args.len(), 1, "back-edge should carry incremented value");
    } else {
        panic!("body should branch to header");
    }

    crate::tir::verify::verify_function(&func).expect("verification should pass");
}

#[test]
fn devirt_range_two_args() {
    let mut func = build_range_for_loop(&[5, 20]);
    let stats = run(&mut func);

    assert!(
        stats.values_changed > 0,
        "pass should transform range(start, stop)"
    );

    let header = &func.blocks[&BlockId(1)];
    assert!(header.ops.iter().any(|op| op.opcode == OpCode::Lt));

    crate::tir::verify::verify_function(&func).expect("verification should pass");
}

#[test]
fn devirt_range_three_args_positive_step() {
    let mut func = build_range_for_loop(&[0, 100, 3]);
    let stats = run(&mut func);

    assert!(
        stats.values_changed > 0,
        "pass should transform range(start, stop, step)"
    );

    let header = &func.blocks[&BlockId(1)];
    assert!(header.ops.iter().any(|op| op.opcode == OpCode::Lt));

    crate::tir::verify::verify_function(&func).expect("verification should pass");
}

#[test]
fn devirt_range_three_args_negative_step() {
    let mut func = build_range_for_loop(&[10, 0, -1]);
    let stats = run(&mut func);

    assert!(
        stats.values_changed > 0,
        "pass should transform range with negative step"
    );

    let header = &func.blocks[&BlockId(1)];
    assert!(
        header.ops.iter().any(|op| op.opcode == OpCode::Gt),
        "negative step should use Gt comparison"
    );

    crate::tir::verify::verify_function(&func).expect("verification should pass");
}

#[test]
fn no_devirt_non_range_loop() {
    let mut func = TirFunction::new("test".into(), vec![TirType::DynBox], TirType::None);

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

    let stats = run(&mut func);
    assert_eq!(
        stats.ops_removed, 0,
        "non-range loop should not be transformed"
    );
    assert_eq!(stats.values_changed, 0);
}

#[test]
fn devirt_preserves_loop_break_kind() {
    let mut func = build_range_for_loop(&[10]);
    run(&mut func);

    assert_eq!(
        func.loop_break_kinds.get(&BlockId(1)),
        Some(&LoopBreakKind::BreakIfFalse)
    );
}
