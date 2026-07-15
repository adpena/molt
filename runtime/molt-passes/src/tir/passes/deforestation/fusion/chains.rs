use std::collections::HashMap;

use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode};
use crate::tir::values::ValueId;

use super::model::{FusableBuiltin, IteratorChain, is_fusable_body};

/// Scan the function for fusable iterator chains.
pub(super) fn find_fusable_chains(
    func: &TirFunction,
    def_map: &HashMap<ValueId, (BlockId, usize)>,
    get_iter_sources: &HashMap<ValueId, ValueId>,
) -> Vec<IteratorChain> {
    let mut chains = Vec::new();

    for (&bid, block) in &func.blocks {
        for (i, op) in block.ops.iter().enumerate() {
            // Look for CallBuiltin with a known fusable name.
            if op.opcode != OpCode::CallBuiltin {
                continue;
            }
            let builtin_name = match op.attrs.get("name") {
                Some(AttrValue::Str(s)) => s.as_str(),
                _ => continue,
            };
            let builtin = match FusableBuiltin::from_name(builtin_name) {
                Some(b) => b,
                None => continue,
            };

            // The builtin must have exactly one operand (the iterator argument)
            // and one result.
            if op.operands.len() != 1 || op.results.is_empty() {
                continue;
            }
            let arg_value = op.operands[0];
            let result_value = op.results[0];

            // Trace back: the argument should come from a ForIter loop.
            // Find the ForIter that produces arg_value.
            let (for_block, for_idx) = match def_map.get(&arg_value) {
                Some(&loc) => loc,
                None => continue,
            };

            let for_iter_op = match func.blocks.get(&for_block) {
                Some(b) => match b.ops.get(for_idx) {
                    Some(op) if op.opcode == OpCode::ForIter => op,
                    _ => continue,
                },
                None => continue,
            };

            // ForIter takes an iterator value as operand and yields the element.
            if for_iter_op.operands.is_empty() || for_iter_op.results.is_empty() {
                continue;
            }
            let iter_value = for_iter_op.operands[0];
            let element_value = for_iter_op.results[0];

            // The iterator value should come from a GetIter.
            let source_iterable = match get_iter_sources.get(&iter_value) {
                Some(&src) => src,
                None => continue,
            };

            // Find the loop body block. The ForIter block's terminator should
            // branch to a body block on success.
            let loop_body_block = match &func.blocks[&for_block].terminator {
                Terminator::CondBranch { then_block, .. } => *then_block,
                Terminator::Branch { target, .. } => *target,
                Terminator::Switch { .. }
                | Terminator::StateDispatch { .. }
                | Terminator::Return { .. }
                | Terminator::Unreachable => continue,
            };

            // Check purity of the loop body.
            let body_block = match func.blocks.get(&loop_body_block) {
                Some(b) => b,
                None => continue,
            };
            if !is_fusable_body(&body_block.ops) {
                continue;
            }

            chains.push(IteratorChain {
                consumer_block: bid,
                consumer_op_idx: i,
                builtin,
                loop_header_block: for_block,
                for_iter_op_idx: for_idx,
                loop_body_block,
                iter_value,
                element_value,
                result_value,
                source_iterable,
            });
        }
    }

    chains
}
