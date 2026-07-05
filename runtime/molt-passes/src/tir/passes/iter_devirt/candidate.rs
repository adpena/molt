use std::collections::HashMap;

use crate::tir::blocks::{BlockId, LoopRole, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::ops::OpCode;
use crate::tir::values::ValueId;

use super::source::{infer_list_container_type, is_list_source};

/// Describes a recognized list-loop pattern ready for devirtualization.
pub(super) struct ListLoopCandidate {
    /// Block containing the GetIter op.
    pub(super) setup_block: BlockId,
    /// Index of the GetIter op within setup_block.
    pub(super) get_iter_idx: usize,
    /// The ValueId of the list being iterated.
    pub(super) list_val: ValueId,
    /// The ValueId produced by GetIter — the iterator.
    _iter_val: ValueId,
    /// Loop header block containing IterNextUnboxed.
    pub(super) header_block: BlockId,
    /// Index of the IterNextUnboxed op within header_block.
    pub(super) iter_next_idx: usize,
    /// The element ValueId produced by IterNextUnboxed (results[0]).
    pub(super) elem_val: ValueId,
    /// The done-flag ValueId produced by IterNextUnboxed (results[1]).
    pub(super) done_val: ValueId,
    /// The exit block (where done=true branches to).
    pub(super) exit_block: BlockId,
    /// The body block (where done=false branches to).
    pub(super) body_block: BlockId,
    /// Semantic container type proven by function-owned type facts or a
    /// structural list producer. Propagated to the synthesized Index op so
    /// legacy SimpleIR consumers preserve the known list shape.
    pub(super) container_type: Option<String>,
}

/// Scan the function for list-loop patterns.
pub(super) fn find_candidates(func: &TirFunction) -> Vec<ListLoopCandidate> {
    // Phase 1: Build definition map for GetIter ops.
    // Map iter_val -> (setup_block, op_index, source_val)
    let mut get_iter_defs: HashMap<ValueId, (BlockId, usize, ValueId)> = HashMap::new();

    let mut block_ids: Vec<BlockId> = func.blocks.keys().copied().collect();
    block_ids.sort_by_key(|b| b.0);

    for &bid in &block_ids {
        let block = &func.blocks[&bid];
        for (op_idx, op) in block.ops.iter().enumerate() {
            if op.opcode == OpCode::GetIter && !op.operands.is_empty() && !op.results.is_empty() {
                get_iter_defs.insert(op.results[0], (bid, op_idx, op.operands[0]));
            }
        }
    }

    // Phase 2: Find loop headers with IterNextUnboxed that trace back to
    // a GetIter on a known list.
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
            let (elem_val, done_val) = match op.opcode {
                OpCode::IterNextUnboxed if op.results.len() == 2 && !op.operands.is_empty() => {
                    (op.results[0], op.results[1])
                }
                _ => continue,
            };

            let iter_val = op.operands[0];

            // Trace: iter_val -> GetIter(source)
            let Some(&(setup_block, get_iter_idx, source_val)) = get_iter_defs.get(&iter_val)
            else {
                continue;
            };

            // Check if source is known to be a list from typed facts or a
            // structural list producer. Transport-only container metadata is
            // intentionally ignored.
            if !is_list_source(func, source_val, &block_ids) {
                // Not a list — skip. This avoids transforming dict/set/generator
                // iteration which has different semantics.
                continue;
            }

            // Determine the container_type for the synthesized Index op from
            // the same typed/structural proof used for devirtualization.
            let container_type = infer_list_container_type(func, source_val, &block_ids);

            // Reject if source_val is defined INSIDE the loop (mutation risk).
            // The list must be defined before the loop header.
            let source_in_loop = {
                let mut in_loop = false;
                // Check if source is defined in the header or body.
                // A conservative check: if defined in setup_block, it's fine.
                // If defined elsewhere, check if that block has a LoopRole.
                'outer: for &bid in &block_ids {
                    if bid == setup_block {
                        continue;
                    }
                    if let Some(block) = func.blocks.get(&bid) {
                        for def_op in &block.ops {
                            if def_op.results.contains(&source_val) {
                                // Check if this block is part of the loop.
                                if func.loop_roles.contains_key(&bid) && bid != header {
                                    // Defined in a loop-related block that isn't
                                    // the header — could be the body.
                                    in_loop = true;
                                }
                                break 'outer;
                            }
                        }
                    }
                }
                in_loop
            };

            if source_in_loop {
                continue;
            }

            // The header's terminator must be a CondBranch on done_val.
            let (exit_block, body_block) = match &header_block.terminator {
                Terminator::CondBranch {
                    cond,
                    then_block,
                    else_block,
                    ..
                } if *cond == done_val => {
                    // done=true -> then_block (exit), done=false -> else_block (body)
                    (*then_block, *else_block)
                }
                _ => continue,
            };

            candidates.push(ListLoopCandidate {
                setup_block,
                get_iter_idx,
                list_val: source_val,
                _iter_val: iter_val,
                header_block: header,
                iter_next_idx: op_idx,
                elem_val,
                done_val,
                exit_block,
                body_block,
                container_type,
            });

            // Only process the first IterNextUnboxed per header.
            break;
        }
    }

    candidates
}
