//! Make each poll invocation's exits visible to ordinary ownership analysis.
//!
//! Persistence belongs to explicit ClosureStore/ClosureLoad operations. A yield
//! or pending wait ends this invocation; it cannot carry an SSA owner into the
//! next one. Expose both exits as Return, so the same release/transfer analysis
//! governs normal completion, suspension and exceptions on every backend.

use std::collections::HashMap;

use crate::tir::blocks::{Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

use super::util::make_op;

fn state_set(state: i64, origin: &TirOp) -> TirOp {
    let mut op = make_op(OpCode::StateSet, vec![]);
    op.attrs.insert("value".into(), AttrValue::Int(state));
    op.source_span = origin.source_span.clone();
    op
}

/// Called at the terminal ownership boundary, after generator fusion and all
/// other transforms which consume the higher-level suspension operations.
pub(super) fn expose_activation_exits(func: &mut TirFunction) -> usize {
    // High-level yields must have been lowered to explicit state and frame
    // storage before terminal ownership. Retaining an SSA value here cannot
    // persist it across a future invocation.
    for op in func.blocks.values().flat_map(|block| &block.ops) {
        assert!(
            !matches!(op.opcode, OpCode::Yield | OpCode::YieldFrom),
            "{}: high-level suspension must be lowered before activation ownership",
            func.name
        );
    }
    let constants: HashMap<ValueId, i64> = func
        .blocks
        .values()
        .flat_map(|block| {
            block.ops.iter().filter_map(|op| {
                if op.opcode != OpCode::ConstInt {
                    return None;
                }
                match (op.results.first(), op.attrs.get("value")) {
                    (Some(&value), Some(AttrValue::Int(state))) => Some((value, *state)),
                    _ => None,
                }
            })
        })
        .collect();
    let sites: Vec<_> = func
        .blocks
        .iter()
        .flat_map(|(&bid, block)| {
            block.ops.iter().enumerate().filter_map(move |(index, op)| {
                matches!(op.opcode, OpCode::StateYield | OpCode::StateTransition)
                    .then_some((bid, index))
            })
        })
        .collect();
    let mut changed = 0;
    // CFG construction ends each suspension block at the suspension operation.
    // Work backwards defensively, preserving indices if an authored TIR block
    // contains more than one operation that must be split.
    for (bid, index) in sites.into_iter().rev() {
        let op = func.blocks[&bid].ops[index].clone();
        let next_state = match op.attrs.get("value") {
            Some(AttrValue::Int(state)) => *state,
            _ => panic!("{}: suspension lacks a resume state", func.name),
        };
        if op.opcode == OpCode::StateYield {
            assert_eq!(op.operands.len(), 1, "yield publishes one owned result");
            let block = func.blocks.get_mut(&bid).unwrap();
            assert_eq!(
                index + 1,
                block.ops.len(),
                "yield must end its activation block"
            );
            block.ops[index] = state_set(next_state, &op);
            block.terminator = Terminator::Return {
                values: op.operands,
            };
            changed += 1;
            continue;
        }

        let (future, slot, pending) = match op.operands.as_slice() {
            [future, pending] => (*future, None, *pending),
            [future, slot, pending] => (*future, Some(*slot), *pending),
            _ => panic!(
                "{}: wait expects future, optional slot, and resume state",
                func.name
            ),
        };
        let pending_state = *constants
            .get(&pending)
            .unwrap_or_else(|| panic!("{}: wait resume state must be a constant", func.name));
        let result = match op.results.as_slice() {
            [result] => *result,
            _ => panic!("{}: wait must define its polled result", func.name),
        };
        // Poll returns an owned boxed result, including an immediate pending
        // sentinel. It is transferred on the pending exit and released/used on
        // the ready path by ordinary Return/SSA ownership rules.
        func.value_types.insert(result, TirType::DynBox);
        let condition = func.fresh_value();
        func.value_types.insert(condition, TirType::Bool);
        let pending_block = func.fresh_block();
        let ready_block = func.fresh_block();
        let mut poll = make_op(OpCode::Call, vec![future]);
        poll.results.push(result);
        poll.attrs
            .insert("s_value".into(), AttrValue::Str("molt_future_poll".into()));
        poll.source_span = op.source_span.clone();
        let mut test = make_op(OpCode::IsPending, vec![result]);
        test.results.push(condition);
        let block = func.blocks.get_mut(&bid).unwrap();
        let suffix = block.ops.split_off(index + 1);
        block.ops.pop();
        block
            .ops
            .extend([state_set(pending_state, &op), poll, test]);
        let continuation = std::mem::replace(
            &mut block.terminator,
            Terminator::CondBranch {
                cond: condition,
                then_block: pending_block,
                then_args: vec![],
                else_block: ready_block,
                else_args: vec![],
            },
        );
        func.blocks.insert(
            pending_block,
            TirBlock {
                id: pending_block,
                args: vec![],
                ops: vec![make_op(OpCode::TaskWait, vec![future])],
                terminator: Terminator::Return {
                    values: vec![result],
                },
            },
        );
        let mut ready_ops = Vec::new();
        if let Some(slot) = slot {
            let offset = *constants
                .get(&slot)
                .unwrap_or_else(|| panic!("{}: wait result slot must be a constant", func.name));
            let frame = func.blocks[&func.entry_block]
                .args
                .first()
                .expect("poll invocation requires its frame argument")
                .id;
            let mut store = make_op(OpCode::ClosureStore, vec![frame, result]);
            store.attrs.insert("value".into(), AttrValue::Int(offset));
            ready_ops.push(store);
        }
        ready_ops.push(state_set(next_state, &op));
        ready_ops.extend(suffix);
        func.blocks.insert(
            ready_block,
            TirBlock {
                id: ready_block,
                args: vec![],
                ops: ready_ops,
                terminator: continuation,
            },
        );
        changed += 1;
    }
    changed
}
