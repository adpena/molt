use std::collections::HashMap;

use crate::tir::blocks::{BlockId, LoopRole, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{RangeDevirtRole, opcode_range_devirt_role_table};
use crate::tir::ops::{AttrValue, OpCode};
use crate::tir::values::ValueId;

/// Describes a recognized range-loop pattern ready for devirtualization.
pub(super) struct RangeLoopCandidate {
    /// Block containing the CallBuiltin("range") and GetIter ops.
    pub(super) setup_block: BlockId,
    /// Index of the CallBuiltin("range") op within setup_block.
    pub(super) call_range_idx: usize,
    /// Index of the GetIter op within setup_block.
    pub(super) get_iter_idx: usize,
    /// The ValueId produced by CallBuiltin("range") - the range object.
    pub(super) _range_obj: ValueId,
    /// The ValueId produced by GetIter - the iterator.
    pub(super) _iter_val: ValueId,
    /// Loop header block containing IterNextUnboxed/ForIter.
    pub(super) header_block: BlockId,
    /// Index of the IterNextUnboxed/ForIter op within header_block.
    pub(super) iter_next_idx: usize,
    /// The element ValueId produced by IterNextUnboxed (results[0]).
    pub(super) elem_val: ValueId,
    /// The done-flag ValueId produced by IterNextUnboxed (results[1]).
    pub(super) done_val: ValueId,
    /// Whether this uses IterNextUnboxed (2 results) vs ForIter (1 result).
    pub(super) _uses_unboxed: bool,
    /// Range arguments: start, stop, step ValueIds.
    pub(super) start_val: ValueId,
    pub(super) stop_val: ValueId,
    pub(super) step_val: ValueId,
    /// Whether the step is a known constant.
    pub(super) step_const: Option<i64>,
    /// The exit block (where done=true branches to).
    pub(super) exit_block: BlockId,
    /// The body block (where done=false branches to).
    pub(super) body_block: BlockId,
}

/// Scan the function for range-loop patterns.
pub(super) fn find_candidates(func: &TirFunction) -> Vec<RangeLoopCandidate> {
    let mut call_builtin_defs: HashMap<ValueId, (BlockId, usize, Vec<ValueId>)> = HashMap::new();
    let mut get_iter_defs: HashMap<ValueId, (BlockId, usize, ValueId)> = HashMap::new();

    let mut block_ids: Vec<BlockId> = func.blocks.keys().copied().collect();
    block_ids.sort_by_key(|b| b.0);

    for &bid in &block_ids {
        let block = &func.blocks[&bid];
        for (op_idx, op) in block.ops.iter().enumerate() {
            match opcode_range_devirt_role_table(op.opcode) {
                RangeDevirtRole::RangeCallCandidate => {
                    let name = op
                        .attrs
                        .get("name")
                        .and_then(|v| match v {
                            AttrValue::Str(s) => Some(s.as_str()),
                            _ => None,
                        })
                        .unwrap_or("");
                    if (name == "range" || name == "builtin_range" || name == "molt_range")
                        && !op.results.is_empty()
                        && (1..=3).contains(&op.operands.len())
                    {
                        call_builtin_defs.insert(op.results[0], (bid, op_idx, op.operands.clone()));
                    }
                }
                RangeDevirtRole::IteratorCandidate
                    if !op.operands.is_empty() && !op.results.is_empty() =>
                {
                    get_iter_defs.insert(op.results[0], (bid, op_idx, op.operands[0]));
                }
                _ => {}
            }
        }
    }

    let loop_headers: Vec<BlockId> = func
        .loop_roles
        .iter()
        .filter_map(|(bid, role)| (*role == LoopRole::LoopHeader).then_some(*bid))
        .collect();

    let mut candidates = Vec::new();

    for header in loop_headers {
        let Some(header_block) = func.blocks.get(&header) else {
            continue;
        };

        for (op_idx, op) in header_block.ops.iter().enumerate() {
            let (uses_unboxed, elem_val, done_val) = match opcode_range_devirt_role_table(op.opcode)
            {
                RangeDevirtRole::NextUnboxedCandidate
                    if op.results.len() == 2 && !op.operands.is_empty() =>
                {
                    (true, op.results[0], op.results[1])
                }
                _ => continue,
            };

            let iter_val = op.operands[0];

            let Some(&(get_iter_block, get_iter_idx, source_val)) = get_iter_defs.get(&iter_val)
            else {
                continue;
            };

            let Some(&(call_block, call_idx, ref range_args)) = call_builtin_defs.get(&source_val)
            else {
                continue;
            };

            if get_iter_block != call_block {
                continue;
            }

            let (start_val, stop_val, step_val, step_const) =
                extract_range_args(func, &block_ids, range_args);

            let (exit_block, body_block) = match &header_block.terminator {
                Terminator::CondBranch {
                    cond,
                    then_block,
                    else_block,
                    ..
                } if *cond == done_val => (*then_block, *else_block),
                _ => continue,
            };

            candidates.push(RangeLoopCandidate {
                setup_block: call_block,
                call_range_idx: call_idx,
                get_iter_idx,
                _range_obj: source_val,
                _iter_val: iter_val,
                header_block: header,
                iter_next_idx: op_idx,
                elem_val,
                done_val,
                _uses_unboxed: uses_unboxed,
                start_val,
                stop_val,
                step_val,
                step_const,
                exit_block,
                body_block,
            });

            break;
        }
    }

    candidates
}

fn extract_range_args(
    func: &TirFunction,
    block_ids: &[BlockId],
    range_args: &[ValueId],
) -> (ValueId, ValueId, ValueId, Option<i64>) {
    let const_map = build_const_map(func, block_ids);

    match range_args.len() {
        1 => {
            let stop = range_args[0];
            (ValueId(u32::MAX - 1), stop, ValueId(u32::MAX), Some(1))
        }
        2 => {
            let start = range_args[0];
            let stop = range_args[1];
            (start, stop, ValueId(u32::MAX), Some(1))
        }
        3 => {
            let start = range_args[0];
            let stop = range_args[1];
            let step = range_args[2];
            let step_const = const_map.get(&step).copied();
            (start, stop, step, step_const)
        }
        _ => unreachable!("range_args len already validated as 1..=3"),
    }
}

fn build_const_map(func: &TirFunction, block_ids: &[BlockId]) -> HashMap<ValueId, i64> {
    let mut map = HashMap::new();
    for &bid in block_ids {
        if let Some(block) = func.blocks.get(&bid) {
            for op in &block.ops {
                if op.opcode == OpCode::ConstInt
                    && !op.results.is_empty()
                    && let Some(AttrValue::Int(v)) = op.attrs.get("value")
                {
                    map.insert(op.results[0], *v);
                }
            }
        }
    }
    map
}
