use super::compute_liveness;
use crate::tir::blocks::{LoopRole, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};

fn op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands,
        results,
        attrs: AttrDict::new(),
        source_span: None,
    }
}

fn const_str(result: ValueId) -> TirOp {
    let mut attrs = AttrDict::new();
    attrs.insert("s_value".into(), AttrValue::Str("x".into()));
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstStr,
        operands: vec![],
        results: vec![result],
        attrs,
        source_span: None,
    }
}

/// Straight-line: v1 = Call(); v2 = Call(v1); Return(v2). v1's last op-use is
/// op index 1 and v1 is dead afterward (not live-out, not in Return).
#[test]
fn straight_line_last_use() {
    let mut func = TirFunction::new("sl".into(), vec![], TirType::DynBox);
    let v0 = func.fresh_value(); // some root str
    let v1 = func.fresh_value();
    let v2 = func.fresh_value();
    func.value_types.insert(v0, TirType::Str);
    func.value_types.insert(v1, TirType::Str);
    func.value_types.insert(v2, TirType::Str);
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops.push(const_str(v0));
        b.ops.push(op(OpCode::Call, vec![v0], vec![v1]));
        b.ops.push(op(OpCode::Call, vec![v1], vec![v2]));
        b.terminator = Terminator::Return { values: vec![v2] };
    }
    let res = compute_liveness(&func);
    let block = &func.blocks[&entry];
    assert_eq!(res.last_use_in_block(block, v1), Some(2));
    // v1 is not live-out of entry (no successors) and dead after op 2.
    assert!(!res.is_live_out(entry, v1));
    // v2 is defined AND used (returned) within entry → not upward-exposed,
    // so it is neither live-in nor live-out: a within-block value the drop
    // pass handles purely by straight-line last-use, not via the live sets.
    assert!(!res.live_in[&entry].contains(&v2));
    assert!(!res.is_live_out(entry, v2));
}

/// CondBranch: value used in both arms and live-out of the cond block.
#[test]
fn used_in_both_branches_is_live_out() {
    let mut func = TirFunction::new("br".into(), vec![], TirType::DynBox);
    let cond = func.fresh_value();
    let x = func.fresh_value();
    let r1 = func.fresh_value();
    let r2 = func.fresh_value();
    func.value_types.insert(cond, TirType::Bool);
    func.value_types.insert(x, TirType::Str);
    func.value_types.insert(r1, TirType::Str);
    func.value_types.insert(r2, TirType::Str);
    let entry = func.entry_block;
    let then_b = func.fresh_block();
    let else_b = func.fresh_block();
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops.push(const_str(x));
        b.ops.push(op(OpCode::ConstBool, vec![], vec![cond]));
        b.terminator = Terminator::CondBranch {
            cond,
            then_block: then_b,
            then_args: vec![],
            else_block: else_b,
            else_args: vec![],
        };
    }
    func.blocks.insert(
        then_b,
        TirBlock {
            id: then_b,
            args: vec![],
            ops: vec![op(OpCode::Call, vec![x], vec![r1])],
            terminator: Terminator::Return { values: vec![r1] },
        },
    );
    func.blocks.insert(
        else_b,
        TirBlock {
            id: else_b,
            args: vec![],
            ops: vec![op(OpCode::Call, vec![x], vec![r2])],
            terminator: Terminator::Return { values: vec![r2] },
        },
    );
    let res = compute_liveness(&func);
    // x is used in both successors → live-out of entry.
    assert!(res.is_live_out(entry, x));
    assert!(res.is_live_in(then_b, x));
    assert!(res.is_live_in(else_b, x));
}

/// Loop-carried block arg: value live-in at the header, propagated via the
/// back-edge arg.
#[test]
fn loop_carried_block_arg_live() {
    let mut func = TirFunction::new("loop".into(), vec![], TirType::DynBox);
    let header = func.fresh_block();
    let body = func.fresh_block();
    let exit = func.fresh_block();
    let acc0 = func.fresh_value();
    let acc_phi = func.fresh_value();
    let cond = func.fresh_value();
    let acc_next = func.fresh_value();
    for v in [acc0, acc_phi, acc_next] {
        func.value_types.insert(v, TirType::Str);
    }
    func.value_types.insert(cond, TirType::Bool);
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops.push(const_str(acc0));
        b.terminator = Terminator::Branch {
            target: header,
            args: vec![acc0],
        };
    }
    func.blocks.insert(
        header,
        TirBlock {
            id: header,
            args: vec![TirValue {
                id: acc_phi,
                ty: TirType::Str,
            }],
            ops: vec![op(OpCode::ConstBool, vec![], vec![cond])],
            terminator: Terminator::CondBranch {
                cond,
                then_block: body,
                then_args: vec![],
                else_block: exit,
                else_args: vec![],
            },
        },
    );
    func.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![],
            ops: vec![op(OpCode::Call, vec![acc_phi], vec![acc_next])],
            terminator: Terminator::Branch {
                target: header,
                args: vec![acc_next],
            },
        },
    );
    func.blocks.insert(
        exit,
        TirBlock {
            id: exit,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return {
                values: vec![acc_phi],
            },
        },
    );
    func.loop_roles.insert(header, LoopRole::LoopHeader);
    let res = compute_liveness(&func);
    // acc_phi is a block ARG of the header → defined (killed) at header
    // entry, so it is NOT live-in to its own block (standard MLIR block-arg
    // dataflow). Its liveness manifests as: it is live-out of the header
    // (used in the body successor), and it is live-in to the body.
    assert!(!res.is_live_in(header, acc_phi));
    assert!(res.is_live_out(header, acc_phi));
    assert!(res.is_live_in(body, acc_phi));
    // acc_next is passed on the back-edge → live-out of body.
    assert!(res.is_live_out(body, acc_next));
}

/// A raw i64 value (proven inline by value-range) is excluded from the live
/// sets even when used.
#[test]
fn raw_i64_excluded_from_live_sets() {
    let mut func = TirFunction::new("raw".into(), vec![], TirType::I64);
    let c0 = func.fresh_value();
    let c1 = func.fresh_value();
    let s = func.fresh_value();
    for v in [c0, c1, s] {
        func.value_types.insert(v, TirType::I64);
    }
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        let mut a0 = AttrDict::new();
        a0.insert("value".into(), AttrValue::Int(3));
        b.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![c0],
            attrs: a0,
            source_span: None,
        });
        let mut a1 = AttrDict::new();
        a1.insert("value".into(), AttrValue::Int(4));
        b.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![c1],
            attrs: a1,
            source_span: None,
        });
        b.ops.push(op(OpCode::Add, vec![c0, c1], vec![s]));
        b.terminator = Terminator::Return { values: vec![s] };
    }
    let res = compute_liveness(&func);
    // c0 / c1 are small inline ints (range-proven) → raw scalars, excluded.
    assert!(res.is_raw_scalar(c0));
    assert!(res.is_raw_scalar(c1));
    assert!(!res.live_in[&entry].contains(&c0));
    assert!(!res.live_in[&entry].contains(&c1));
}

/// A Bool value is filtered out of the live sets by the by-type floor.
#[test]
fn bool_excluded_from_live_sets() {
    let mut func = TirFunction::new("b".into(), vec![], TirType::Bool);
    let c = func.fresh_value();
    func.value_types.insert(c, TirType::Bool);
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops.push(op(OpCode::ConstBool, vec![], vec![c]));
        b.terminator = Terminator::Return { values: vec![c] };
    }
    let res = compute_liveness(&func);
    assert!(res.is_raw_scalar(c));
    assert!(!res.live_in[&entry].contains(&c));
}

/// A None sentinel uses the generic i64 transport carrier but has no
/// refcounted heap ownership obligation, so RC placement must ignore it.
#[test]
fn none_excluded_from_live_sets() {
    let mut func = TirFunction::new("none".into(), vec![], TirType::None);
    let n = func.fresh_value();
    func.value_types.insert(n, TirType::None);
    let entry = func.entry_block;
    {
        let b = func.blocks.get_mut(&entry).unwrap();
        b.ops.push(op(OpCode::ConstNone, vec![], vec![n]));
        b.terminator = Terminator::Return { values: vec![n] };
    }
    let res = compute_liveness(&func);
    assert!(res.is_raw_scalar(n));
    assert!(!res.live_in[&entry].contains(&n));
}
