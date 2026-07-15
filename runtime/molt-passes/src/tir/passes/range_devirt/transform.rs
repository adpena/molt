use crate::tir::blocks::{BlockId, LoopBreakKind, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};

use super::super::PassStats;
use super::candidate::RangeLoopCandidate;

/// Apply the range devirtualization transform to a single candidate.
pub(super) fn apply_transform(
    func: &mut TirFunction,
    c: &RangeLoopCandidate,
    stats: &mut PassStats,
) {
    let start_val = if c.start_val.0 == u32::MAX - 1 {
        let val = func.fresh_value();
        func.value_types.insert(val, TirType::I64);
        let const_op = make_const_int(val, 0);
        if let Some(block) = func.blocks.get_mut(&c.setup_block) {
            block.ops.insert(c.call_range_idx, const_op);
        }
        stats.ops_added += 1;
        val
    } else {
        c.start_val
    };

    let offset = if c.start_val.0 == u32::MAX - 1 { 1 } else { 0 };

    let step_val = if c.step_val.0 == u32::MAX {
        let val = func.fresh_value();
        func.value_types.insert(val, TirType::I64);
        let const_op = make_const_int(val, 1);
        if let Some(block) = func.blocks.get_mut(&c.setup_block) {
            block.ops.insert(c.call_range_idx + offset, const_op);
        }
        stats.ops_added += 1;
        val
    } else {
        c.step_val
    };

    let offset2 = offset + if c.step_val.0 == u32::MAX { 1 } else { 0 };

    if let Some(block) = func.blocks.get_mut(&c.setup_block) {
        let call_idx = c.call_range_idx + offset2;
        let iter_idx = c.get_iter_idx + offset2;

        let (first_remove, second_remove) = if call_idx > iter_idx {
            (call_idx, iter_idx)
        } else {
            (iter_idx, call_idx)
        };

        if first_remove < block.ops.len() {
            block.ops.remove(first_remove);
            stats.ops_removed += 1;
        }
        if second_remove < block.ops.len() {
            block.ops.remove(second_remove);
            stats.ops_removed += 1;
        }
    }

    if let Some(block) = func.blocks.get_mut(&c.setup_block) {
        block.terminator.for_each_edge_mut(|target, args| {
            if *target == c.header_block {
                args.push(start_val);
            }
        });
    }

    let ind_var = c.elem_val;
    func.value_types.insert(ind_var, TirType::I64);

    if let Some(header) = func.blocks.get_mut(&c.header_block) {
        header.args.push(TirValue {
            id: ind_var,
            ty: TirType::I64,
        });

        let cmp_opcode = match c.step_const {
            Some(s) if s < 0 => OpCode::Gt,
            _ => OpCode::Lt,
        };

        let cond_val = c.done_val;
        func.value_types.insert(cond_val, TirType::Bool);
        let cmp_op = TirOp {
            dialect: Dialect::Molt,
            opcode: cmp_opcode,
            operands: vec![ind_var, c.stop_val],
            results: vec![cond_val],
            attrs: AttrDict::new(),
            source_span: None,
        };

        header.ops[c.iter_next_idx] = cmp_op;
        stats.values_changed += 1;

        header.terminator = Terminator::CondBranch {
            cond: cond_val,
            then_block: c.body_block,
            then_args: vec![],
            else_block: c.exit_block,
            else_args: vec![],
        };
    }

    let back_edge_blocks: Vec<BlockId> = {
        let mut result = Vec::new();
        for (&bid, block) in &func.blocks {
            if bid == c.header_block {
                continue;
            }
            let branches_to_header = block.terminator.has_successor(c.header_block);
            if branches_to_header {
                result.push(bid);
            }
        }
        result
    };

    for back_bid in back_edge_blocks {
        if back_bid == c.setup_block {
            continue;
        }

        let next_val = func.fresh_value();
        func.value_types.insert(next_val, TirType::I64);

        if let Some(block) = func.blocks.get_mut(&back_bid) {
            let nsw_safe = matches!(c.step_const, Some(1) | Some(-1));
            let add_op = TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Add,
                operands: vec![ind_var, step_val],
                results: vec![next_val],
                attrs: {
                    let mut a = AttrDict::new();
                    if nsw_safe {
                        a.insert("no_signed_wrap".to_string(), AttrValue::Bool(true));
                    }
                    a
                },
                source_span: None,
            };
            block.ops.push(add_op);
            stats.ops_added += 1;

            block.terminator.for_each_edge_mut(|target, args| {
                if *target == c.header_block {
                    args.push(next_val);
                }
            });
        }
    }

    func.loop_break_kinds
        .insert(c.header_block, LoopBreakKind::BreakIfFalse);
}

pub(super) fn make_const_int(result: ValueId, value: i64) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![result],
        attrs: {
            let mut m = AttrDict::new();
            m.insert("value".to_string(), AttrValue::Int(value));
            m
        },
        source_span: None,
    }
}
