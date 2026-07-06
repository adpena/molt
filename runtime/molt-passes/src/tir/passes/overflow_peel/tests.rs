use std::collections::HashSet;

use super::*;
use crate::tir::analysis::AnalysisManager;
use crate::tir::blocks::{LoopRole, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};
use crate::tir::verify::verify_function;

/// Build the live-shape fixture: the exact CFG `peel_sum.py`'s `compute`
/// produces post-pipeline (entry consts -> 2-phi header -> guard(Lt) ->
/// linear body with two marker-wrapped Adds -> exit Return), including the
/// vestigial unreachable loop-else pred passing ConstNone.
fn live_shape_function() -> TirFunction {
    let mut func = TirFunction::new(
        "peel_fixture".into(),
        vec![TirType::DynBox],
        TirType::DynBox,
    );
    let header = func.fresh_block();
    let guard = func.fresh_block();
    let body = func.fresh_block();
    let stray = func.fresh_block();
    let exit = func.fresh_block();

    let n = ValueId(0); // entry arg (param)
    let fresh = |func: &mut TirFunction, ty: TirType| {
        let v = func.fresh_value();
        func.value_types.insert(v, ty);
        v
    };
    let c_total = fresh(&mut func, TirType::I64);
    let c_i = fresh(&mut func, TirType::I64);
    let c_one = fresh(&mut func, TirType::I64);
    let none_v = fresh(&mut func, TirType::None);
    let t_phi = fresh(&mut func, TirType::I64);
    let i_phi = fresh(&mut func, TirType::I64);
    let i_copy = fresh(&mut func, TirType::I64);
    let cond = fresh(&mut func, TirType::Bool);
    let t_in = fresh(&mut func, TirType::I64);
    let i_in = fresh(&mut func, TirType::I64);
    let t_sum = fresh(&mut func, TirType::I64);
    let t_marker = fresh(&mut func, TirType::I64);
    let i_in2 = fresh(&mut func, TirType::I64);
    let i_sum = fresh(&mut func, TirType::I64);
    let i_marker = fresh(&mut func, TirType::I64);
    let ret_copy = fresh(&mut func, TirType::I64);

    let const_op = |opcode: OpCode, value: i64, result: ValueId| {
        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Int(value));
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: vec![],
            results: vec![result],
            attrs,
            source_span: None,
        }
    };
    let op = |opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>| TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands,
        results,
        attrs: AttrDict::new(),
        source_span: None,
    };

    // entry: consts -> header(t0, i0)
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_op(OpCode::ConstInt, 0, c_total));
        entry.ops.push(const_op(OpCode::ConstInt, 0, c_i));
        entry.ops.push(const_op(OpCode::ConstInt, 1, c_one));
        entry.terminator = Terminator::Branch {
            target: header,
            args: vec![c_total, c_i],
        };
    }
    // header(t, i) -> guard
    func.blocks.insert(
        header,
        TirBlock {
            id: header,
            args: vec![
                TirValue {
                    id: t_phi,
                    ty: TirType::I64,
                },
                TirValue {
                    id: i_phi,
                    ty: TirType::I64,
                },
            ],
            ops: vec![],
            terminator: Terminator::Branch {
                target: guard,
                args: vec![],
            },
        },
    );
    // guard: i' = Copy(i); cond = Lt(i', n); CondBranch(cond, body, exit)
    func.blocks.insert(
        guard,
        TirBlock {
            id: guard,
            args: vec![],
            ops: vec![
                op(OpCode::Copy, vec![i_phi], vec![i_copy]),
                op(OpCode::Lt, vec![i_copy, n], vec![cond]),
            ],
            terminator: Terminator::CondBranch {
                cond,
                then_block: body,
                then_args: vec![],
                else_block: exit,
                else_args: vec![],
            },
        },
    );
    // body: t+i and i+1, each through marker copies -> header
    func.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![],
            ops: vec![
                op(OpCode::Copy, vec![t_phi], vec![t_in]),
                op(OpCode::Copy, vec![i_phi], vec![i_in]),
                op(OpCode::Add, vec![t_in, i_in], vec![t_sum]),
                op(OpCode::Copy, vec![t_sum, t_sum], vec![t_marker]),
                op(OpCode::Copy, vec![i_phi], vec![i_in2]),
                op(OpCode::Add, vec![i_in2, c_one], vec![i_sum]),
                op(OpCode::Copy, vec![i_sum, i_sum], vec![i_marker]),
            ],
            terminator: Terminator::Branch {
                target: header,
                args: vec![t_marker, i_marker],
            },
        },
    );
    // stray (unreachable loop-else): ConstNone -> header(None, None)
    func.blocks.insert(
        stray,
        TirBlock {
            id: stray,
            args: vec![],
            ops: vec![op(OpCode::ConstNone, vec![], vec![none_v])],
            terminator: Terminator::Branch {
                target: header,
                args: vec![none_v, none_v],
            },
        },
    );
    // exit: ret_copy = Copy(t); Return ret_copy
    func.blocks.insert(
        exit,
        TirBlock {
            id: exit,
            args: vec![],
            ops: vec![op(OpCode::Copy, vec![t_phi], vec![ret_copy])],
            terminator: Terminator::Return {
                values: vec![ret_copy],
            },
        },
    );

    func.loop_roles.insert(header, LoopRole::LoopHeader);
    func.loop_roles.insert(stray, LoopRole::LoopEnd);
    func.loop_pairs.insert(header, stray);
    func.loop_cond_blocks.insert(header, guard);
    func
}

fn run_peel(func: &mut TirFunction) -> PassStats {
    let mut am = AnalysisManager::new();
    run(func, &mut am)
}

#[test]
fn live_shape_peels_and_verifies() {
    let mut func = live_shape_function();
    let blocks_before = func.blocks.len();
    let stats = run_peel(&mut func);
    assert!(stats.ops_added > 0, "the live shape must peel");
    // Slow loop (3) + dispatch + slow_entry.
    assert_eq!(func.blocks.len(), blocks_before + 5);
    verify_function(&func).expect("peeled function must verify");

    // Both body Adds became CheckedAdds with 2 results.
    let body_checked: usize = func
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| op.opcode == OpCode::CheckedAdd)
        .count();
    assert_eq!(body_checked, 2, "both phi updates become CheckedAdd");

    // Exactly one slow loop with plain Adds survives.
    let plain_adds: usize = func
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| op.opcode == OpCode::Add)
        .count();
    assert_eq!(plain_adds, 2, "the slow clone keeps the plain Adds");

    // The fast header now carries 2 + 1 + 2 phis.
    let header = func
        .loop_roles
        .iter()
        .find(|(_, r)| **r == LoopRole::LoopHeader)
        .map(|(b, _)| *b)
        .unwrap();
    assert_eq!(func.blocks[&header].args.len(), 5);

    // The stray pred no longer passes ConstNone to the header.
    let stray_args = func
        .blocks
        .values()
        .filter_map(|b| match &b.terminator {
            Terminator::Branch { target, args }
                if *target == header && b.ops.iter().any(|o| o.opcode == OpCode::ConstNone) =>
            {
                Some(args.clone())
            }
            _ => None,
        })
        .next()
        .expect("stray pred still targets the header");
    assert_eq!(stray_args.len(), 5);
    let none_ids: HashSet<ValueId> = func
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|o| o.opcode == OpCode::ConstNone)
        .flat_map(|o| o.results.iter().copied())
        .collect();
    assert!(
        stray_args.iter().all(|a| !none_ids.contains(a)),
        "no ConstNone reaches the header phis"
    );

    // The exit block gained an arg and the post-loop use was rewired:
    // the block that Returns must no longer read the header phi
    // (ValueId(5) = t_phi in this fixture) - in-loop uses keep it.
    let exit_block = func
        .blocks
        .values()
        .find(|b| matches!(b.terminator, Terminator::Return { .. }) && !b.ops.is_empty())
        .expect("exit block exists");
    assert_eq!(exit_block.args.len(), 1, "exit gains one arg");
    assert!(
        !exit_block
            .ops
            .iter()
            .any(|op| op.operands.contains(&ValueId(5))),
        "post-loop Copy must read the exit arg, not the header phi"
    );
}

#[test]
fn live_shape_mul_updates_become_checked_mul() {
    let mut func = live_shape_function();
    for block in func.blocks.values_mut() {
        for op in &mut block.ops {
            if op.opcode == OpCode::Add {
                op.opcode = OpCode::Mul;
            }
        }
    }

    let stats = run_peel(&mut func);
    assert!(stats.ops_added > 0, "the multiply shape must peel");
    verify_function(&func).expect("peeled multiply function must verify");

    let checked_mul: usize = func
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| op.opcode == OpCode::CheckedMul)
        .count();
    assert_eq!(checked_mul, 2, "both phi updates become CheckedMul");

    let plain_mul: usize = func
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| op.opcode == OpCode::Mul)
        .count();
    assert_eq!(plain_mul, 2, "the slow clone keeps the plain Muls");
}

#[test]
fn exception_handler_function_refuses() {
    let mut func = live_shape_function();
    // Inject a TryStart anywhere - has_exception_handlers() turns on.
    func.blocks
        .get_mut(&func.entry_block)
        .unwrap()
        .ops
        .push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::TryStart,
            operands: vec![],
            results: vec![],
            attrs: AttrDict::new(),
            source_span: None,
        });
    let blocks_before = func.blocks.len();
    let stats = run_peel(&mut func);
    assert_eq!(stats.ops_added, 0);
    assert_eq!(func.blocks.len(), blocks_before);
}

#[test]
fn impure_body_refuses() {
    let mut func = live_shape_function();
    // A Call in the body breaks re-execution safety.
    let dead = func.fresh_value();
    for block in func.blocks.values_mut() {
        if matches!(&block.terminator, Terminator::Branch { target, .. }
            if func.loop_roles.get(target) == Some(&LoopRole::LoopHeader))
            && block.ops.iter().any(|o| o.opcode == OpCode::Add)
        {
            block.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Call,
                operands: vec![],
                results: vec![dead],
                attrs: AttrDict::new(),
                source_span: None,
            });
        }
    }
    let stats = run_peel(&mut func);
    assert_eq!(stats.ops_added, 0, "a Call in the body must refuse");
}

#[test]
fn non_const_init_refuses() {
    let mut func = live_shape_function();
    // Seed the accumulator from the function parameter (unproven).
    if let Some(entry) = func.blocks.get_mut(&func.entry_block)
        && let Terminator::Branch { args, .. } = &mut entry.terminator
    {
        args[0] = ValueId(0); // the DynBox param
    }
    func.value_types.insert(ValueId(0), TirType::I64);
    let stats = run_peel(&mut func);
    assert_eq!(stats.ops_added, 0, "param-seeded accumulator must refuse");
}

/// A product accumulator (`t = t * i`) peels exactly like an add
/// accumulator: the fast body update swaps to `CheckedMul` (the
/// hardware-overflow-flagged multiply) and the boxed slow clone keeps the
/// plain `Mul` (BigInt-exact on re-execution). The co-resident IV update
/// stays `Add`/`CheckedAdd`, proving the swap is keyed per-op on the
/// recorded update opcode, not globally.
#[test]
fn mul_accumulator_peels_to_checked_mul() {
    let mut func = live_shape_function();
    // Convert the first accumulator's update `t = t + i` to `t = t * i`.
    // The fixture's body is a single linear block latching to the header;
    // its first `Add` (t_in + i_in -> t_sum) is the accumulator update.
    let header = func
        .loop_roles
        .iter()
        .find(|(_, r)| **r == LoopRole::LoopHeader)
        .map(|(b, _)| *b)
        .unwrap();
    let body = match &func.blocks[&func.loop_cond_blocks[&header]].terminator {
        Terminator::CondBranch { then_block, .. } => *then_block,
        _ => panic!("guard must end in a CondBranch"),
    };
    {
        let body_block = func.blocks.get_mut(&body).expect("body exists");
        let first_add = body_block
            .ops
            .iter_mut()
            .find(|o| o.opcode == OpCode::Add)
            .expect("the accumulator update is an Add");
        first_add.opcode = OpCode::Mul;
    }

    let blocks_before = func.blocks.len();
    let stats = run_peel(&mut func);
    assert!(stats.ops_added > 0, "the product accumulator must peel");
    assert_eq!(func.blocks.len(), blocks_before + 5);
    verify_function(&func).expect("peeled function must verify");

    let count = |opcode: OpCode| -> usize {
        func.blocks
            .values()
            .flat_map(|b| b.ops.iter())
            .filter(|op| op.opcode == opcode)
            .count()
    };
    // Fast loop: one CheckedMul (the product update) + one CheckedAdd (the
    // IV update). Each carries 2 results (wrapping value + overflow flag).
    assert_eq!(count(OpCode::CheckedMul), 1, "product update -> CheckedMul");
    assert_eq!(count(OpCode::CheckedAdd), 1, "IV update -> CheckedAdd");
    for op in func.blocks.values().flat_map(|b| b.ops.iter()) {
        if matches!(op.opcode, OpCode::CheckedMul | OpCode::CheckedAdd) {
            assert_eq!(op.results.len(), 2, "checked op must have 2 results");
        }
    }
    // Slow clone: the plain Mul + plain Add survive, BigInt-exact.
    assert_eq!(count(OpCode::Mul), 1, "slow clone keeps the plain Mul");
    assert_eq!(count(OpCode::Add), 1, "slow clone keeps the plain Add");
}
