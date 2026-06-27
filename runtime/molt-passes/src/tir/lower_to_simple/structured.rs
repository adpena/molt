use std::collections::{HashMap, HashSet};

use crate::ir::OpIR;
use crate::tir::blocks::{BlockId, LoopBreakKind, LoopRole, Terminator, TirBlock};
use crate::tir::dominators;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode};
use crate::tir::simple_value_names::value_var;
use crate::tir::values::ValueId;

use super::cfg::{collect_guard_raise_path_blocks, successors_of};
use super::op_lowering::lower_op_many;
use super::op_utils::{annotate_lowered_op, attr_int};

/// A detected natural-loop region, keyed by the loop header block.
/// Used by structured loop emission to re-wrap linearised TIR control
/// flow into loop_start/loop_break_if_X/loop_continue/loop_end sequences.
pub(super) struct LoopRegion {
    /// Guard blocks with CondBranch terminators (type checks, bounds
    /// checks) in the header chain.  These are emitted inline in the
    /// header region (before break) with br_if + raise-path handling.
    pub(super) guard_chain: Vec<BlockId>,
    /// Raise-path blocks reachable from guard CondBranches.
    /// Consumed so they are not double-emitted in the main loop.
    pub(super) guard_raise_blocks: Vec<BlockId>,
    /// The block whose CondBranch controls the loop (body vs exit).
    pub(super) cond_block: BlockId,
    pub(super) body_entry: BlockId,
    pub(super) exit_block: BlockId,
    pub(super) body_set: HashSet<BlockId>,
    pub(super) break_kind: LoopBreakKind,
    pub(super) cond: ValueId,
    pub(super) body_args: Vec<ValueId>,
    pub(super) exit_args: Vec<ValueId>,
}

// ---------------------------------------------------------------------------
// Structured loop emission
// ---------------------------------------------------------------------------

/// Emit a structured loop region: label → loop_start → header ops →
/// loop_break_if_X → body blocks → loop_continue → loop_end.
///
/// Body blocks are emitted in RPO order with labels for internal control
/// flow.  Nested loops within the body are emitted recursively.
/// Back-edges (Branch → header) become `loop_continue`.
/// Branches to the exit block become `loop_break`.
pub(super) fn emit_structured_loop_region(
    header: BlockId,
    func: &TirFunction,
    loop_regions: &HashMap<BlockId, LoopRegion>,
    rpo: &[BlockId],
    block_param_vars: &HashMap<BlockId, Vec<String>>,
    block_label_id: &dyn Fn(&BlockId) -> i64,
    if_inlined_blocks: &HashSet<BlockId>,
    original_to_new_label: &HashMap<i64, i64>,
    label_to_block: &HashMap<i64, BlockId>,
    out: &mut Vec<OpIR>,
    _loop_consumed: &mut HashSet<BlockId>,
) {
    let region = loop_regions.get(&header).expect("loop region missing");
    let block = func.blocks.get(&header).expect("loop header block missing");
    let original_has_ret = func
        .attrs
        .get("_original_has_ret")
        .map(|v| matches!(v, AttrValue::Bool(true)))
        .unwrap_or(false);

    // Collect deferred raise-path blocks from guard CondBranches
    // where the raise successor is targeted by br_if (not fallthrough).
    // These are emitted as labeled blocks before loop_end.
    let mut deferred_raise_paths: Vec<(BlockId, Vec<ValueId>, HashSet<BlockId>)> = Vec::new();
    let mut loop_inline_blocks: HashSet<BlockId> = if_inlined_blocks.clone();

    // 1. Emit label for forward jumps to the header (entry path).
    //    The native backend's pre-analysis registers label IDs from `label`
    //    ops to create Cranelift blocks; without this, `jump(header_label)`
    //    from the entry path would reference a non-existent block.
    if header != func.entry_block {
        out.push(OpIR {
            kind: "label".to_string(),
            value: Some(block_label_id(&header)),
            ..OpIR::default()
        });
    }

    // 2. loop_start — creates loop_block, body_block, after_block in the
    //    native backend and pushes a LoopFrame.
    out.push(OpIR {
        kind: "loop_start".to_string(),
        ..OpIR::default()
    });

    // 3. Header block argument loads (phi values from entry/back-edge).
    if header != func.entry_block
        && let Some(param_vars) = block_param_vars.get(&header)
    {
        for (i, var_name) in param_vars.iter().enumerate() {
            if i < block.args.len() {
                out.push(OpIR {
                    kind: "load_var".to_string(),
                    var: Some(var_name.clone()),
                    out: Some(value_var(block.args[i].id)),
                    ..OpIR::default()
                });
            }
        }
    }

    // 4. Emit all header-region blocks' ops sequentially in CFG order.
    //    The header region = [header, chain blocks (guards + non-guards), cond_block].
    //    Each block's ops are emitted inline.  Branch terminators get
    //    store_var/load_var for phi values.  Guard CondBranch terminators
    //    get br_if + raise-path emission inline.
    {
        // Build the ordered list in actual CFG order: header, then all
        // chain blocks (non-guard and guard interleaved), then cond block.
        let mut header_region: Vec<BlockId> = vec![header];
        // Reconstruct the full chain in CFG order by re-walking the
        // header's successor chain (guards + non-guards together).
        {
            let mut cur = header;
            let mut walk_visited: HashSet<BlockId> = HashSet::new();
            walk_visited.insert(header);
            while let Some(blk) = func.blocks.get(&cur) {
                let next = match &blk.terminator {
                    Terminator::Branch { target, .. } => *target,
                    Terminator::CondBranch {
                        then_block,
                        else_block,
                        ..
                    } => {
                        // Guard CondBranch: follow non-raise path.
                        if region.guard_raise_blocks.contains(then_block) {
                            *else_block
                        } else if region.guard_raise_blocks.contains(else_block) {
                            *then_block
                        } else {
                            // This is the cond block — stop.
                            break;
                        }
                    }
                    _ => break,
                };
                if next == region.cond_block {
                    break;
                }
                if !walk_visited.insert(next) {
                    break;
                }
                header_region.push(next);
                cur = next;
            }
        }
        // Add the cond block if it's not the header itself.
        if region.cond_block != header {
            header_region.push(region.cond_block);
        }
        for bid in header_region.iter().copied().filter(|bid| *bid != header) {
            loop_inline_blocks.insert(bid);
        }

        let guard_set: HashSet<BlockId> = region.guard_chain.iter().copied().collect();

        for (region_idx, region_bid) in header_region.iter().enumerate() {
            let Some(region_block) = func.blocks.get(region_bid) else {
                continue;
            };
            // Emit ops for this block.
            emit_block_ops_inner(
                region_block,
                original_to_new_label,
                label_to_block,
                block_param_vars,
                out,
            );

            // Handle terminator based on type.
            if region_idx + 1 < header_region.len() {
                let next_bid = header_region[region_idx + 1];
                match &region_block.terminator {
                    Terminator::Branch { target, args } => {
                        // Store args for the Branch target, load args for
                        // the next block in the emission sequence.
                        emit_block_arg_stores(*target, args, block_param_vars, out);
                        // If target != next_bid (e.g., Branch → guard, but
                        // next is a non-guard), also load for next_bid.
                        if *target != next_bid {
                            if let Some(next_block) = func.blocks.get(&next_bid)
                                && let Some(param_vars) = block_param_vars.get(&next_bid)
                            {
                                for (i, var_name) in param_vars.iter().enumerate() {
                                    if i < next_block.args.len() {
                                        out.push(OpIR {
                                            kind: "load_var".to_string(),
                                            var: Some(var_name.clone()),
                                            out: Some(value_var(next_block.args[i].id)),
                                            ..OpIR::default()
                                        });
                                    }
                                }
                            }
                        } else if let Some(next_block) = func.blocks.get(&next_bid)
                            && let Some(param_vars) = block_param_vars.get(&next_bid)
                        {
                            for (i, var_name) in param_vars.iter().enumerate() {
                                if i < next_block.args.len() {
                                    out.push(OpIR {
                                        kind: "load_var".to_string(),
                                        var: Some(var_name.clone()),
                                        out: Some(value_var(next_block.args[i].id)),
                                        ..OpIR::default()
                                    });
                                }
                            }
                        }
                    }
                    Terminator::CondBranch {
                        cond: guard_cond,
                        then_block: guard_then,
                        then_args: guard_then_args,
                        else_block: guard_else,
                        else_args: guard_else_args,
                    } if guard_set.contains(region_bid) => {
                        // Guard CondBranch: emit br_if to raise path,
                        // fallthrough to non-raise continuation.
                        let then_is_raise = region.guard_raise_blocks.contains(guard_then);

                        if then_is_raise {
                            // then = raise, else = continue.
                            // br_if cond → raise_label (deferred).
                            // Fallthrough → non-raise continuation.
                            let raise_label = block_label_id(guard_then);
                            out.push(OpIR {
                                kind: "br_if".to_string(),
                                args: Some(vec![value_var(*guard_cond)]),
                                value: Some(raise_label),
                                ..OpIR::default()
                            });
                            // Defer the raise-path block emission to
                            // just before loop_end (dead-end blocks).
                            deferred_raise_paths.push((
                                *guard_then,
                                guard_then_args.clone(),
                                collect_guard_raise_path_blocks(func, *guard_then)
                                    .into_iter()
                                    .collect(),
                            ));
                            // Store args for the non-raise continuation.
                            emit_block_arg_stores(
                                *guard_else,
                                guard_else_args,
                                block_param_vars,
                                out,
                            );
                            // Load args for next block in emission sequence.
                            if let Some(next_block) = func.blocks.get(&next_bid)
                                && let Some(param_vars) = block_param_vars.get(&next_bid)
                            {
                                for (i, var_name) in param_vars.iter().enumerate() {
                                    if i < next_block.args.len() {
                                        out.push(OpIR {
                                            kind: "load_var".to_string(),
                                            var: Some(var_name.clone()),
                                            out: Some(value_var(next_block.args[i].id)),
                                            ..OpIR::default()
                                        });
                                    }
                                }
                            }
                        } else {
                            // else = raise, then = continue.
                            // br_if cond → continue (skip raise on true).
                            // Fallthrough → raise path.
                            let continue_label = block_label_id(&next_bid);
                            out.push(OpIR {
                                kind: "br_if".to_string(),
                                args: Some(vec![value_var(*guard_cond)]),
                                value: Some(continue_label),
                                ..OpIR::default()
                            });
                            // Fallthrough is raise path — emit it inline.
                            // emit_guard_raise_path handles its own
                            // entry store_var for guard_else args.
                            emit_guard_raise_path(
                                *guard_else,
                                guard_else_args,
                                &collect_guard_raise_path_blocks(func, *guard_else)
                                    .into_iter()
                                    .collect(),
                                func,
                                block_param_vars,
                                block_label_id,
                                if_inlined_blocks,
                                original_to_new_label,
                                label_to_block,
                                out,
                            );
                            // Emit label for the continuation (br_if target).
                            out.push(OpIR {
                                kind: "label".to_string(),
                                value: Some(continue_label),
                                ..OpIR::default()
                            });
                            emit_block_arg_stores(
                                *guard_then,
                                guard_then_args,
                                block_param_vars,
                                out,
                            );
                            if let Some(next_block) = func.blocks.get(&next_bid)
                                && let Some(param_vars) = block_param_vars.get(&next_bid)
                            {
                                for (i, var_name) in param_vars.iter().enumerate() {
                                    if i < next_block.args.len() {
                                        out.push(OpIR {
                                            kind: "load_var".to_string(),
                                            var: Some(var_name.clone()),
                                            out: Some(value_var(next_block.args[i].id)),
                                            ..OpIR::default()
                                        });
                                    }
                                }
                            }
                        }
                    }
                    _ => {
                        // Other terminators for non-last blocks (unexpected
                        // but handle gracefully).
                    }
                }
            }
        }
    }

    // 5. Store args for body entry and exit blocks (CondBranch phi values).
    emit_block_arg_stores(region.body_entry, &region.body_args, block_param_vars, out);
    emit_block_arg_stores(region.exit_block, &region.exit_args, block_param_vars, out);

    // 6. loop_break_if_X(cond) — the native backend branches to body_block
    //    (continue) or cleanup→after_block (break) based on the condition.
    let break_kind_str = match region.break_kind {
        LoopBreakKind::BreakIfFalse => "loop_break_if_false",
        LoopBreakKind::BreakIfTrue => "loop_break_if_true",
    };
    let break_op = OpIR {
        kind: break_kind_str.to_string(),
        args: Some(vec![value_var(region.cond)]),
        ..OpIR::default()
    };
    out.push(break_op);

    // 7. Body blocks in RPO order.  The first body block (body_entry) is
    //    emitted without a label — the backend is already in body_block
    //    after the break op.  Subsequent blocks get labels for internal
    //    control flow (if/else, nested loops, etc.).
    let body_rpo: Vec<BlockId> = rpo
        .iter()
        .filter(|b| region.body_set.contains(b))
        .copied()
        .collect();
    let deferred_terminal_body_blocks: Vec<BlockId> = body_rpo
        .iter()
        .copied()
        .filter(|bid| {
            func.blocks.get(bid).is_some_and(|blk| {
                matches!(
                    blk.terminator,
                    Terminator::Return { .. } | Terminator::Unreachable
                )
            })
        })
        .collect();

    let mut is_first_body = true;
    let mut inner_consumed: HashSet<BlockId> = HashSet::new();

    for body_bid in &body_rpo {
        if inner_consumed.contains(body_bid) {
            continue;
        }
        if deferred_terminal_body_blocks.contains(body_bid) {
            continue;
        }

        let Some(body_block) = func.blocks.get(body_bid) else {
            continue;
        };
        let body_role = func
            .loop_roles
            .get(body_bid)
            .cloned()
            .unwrap_or(LoopRole::None);

        // Nested LoopHeader: recursively emit its structured loop.
        // The recursive call handles label, loop_start, ops, break,
        // body, continue, loop_end — so we just call it and skip.
        if body_role == LoopRole::LoopHeader && loop_regions.contains_key(body_bid) {
            emit_structured_loop_region(
                *body_bid,
                func,
                loop_regions,
                rpo,
                block_param_vars,
                block_label_id,
                if_inlined_blocks,
                original_to_new_label,
                label_to_block,
                out,
                _loop_consumed,
            );
            if let Some(inner_region) = loop_regions.get(body_bid) {
                // Consume ALL blocks owned by the inner loop: body,
                // guard chain, guard raise paths, cond block, AND
                // all intermediate blocks between header and cond.
                inner_consumed.extend(inner_region.body_set.iter().copied());
                inner_consumed.extend(inner_region.guard_chain.iter().copied());
                inner_consumed.extend(inner_region.guard_raise_blocks.iter().copied());
                inner_consumed.insert(inner_region.cond_block);
                // Follow the chain from the inner header to the cond
                // block, consuming all intermediate blocks.
                let mut cur = *body_bid;
                let mut visited = HashSet::new();
                visited.insert(cur);
                while let Some(blk) = func.blocks.get(&cur) {
                    match &blk.terminator {
                        Terminator::Branch { target, .. } => {
                            if !visited.insert(*target) || *target == inner_region.cond_block {
                                inner_consumed.insert(*target);
                                break;
                            }
                            inner_consumed.insert(*target);
                            cur = *target;
                        }
                        Terminator::CondBranch {
                            then_block,
                            else_block,
                            ..
                        } => {
                            // Guard: follow non-raise path, consume both
                            let then_raises = func
                                .blocks
                                .get(then_block)
                                .map(|b| b.ops.iter().any(|op| op.opcode == OpCode::Raise))
                                .unwrap_or(false);
                            let raise_bid = if then_raises {
                                *then_block
                            } else {
                                *else_block
                            };
                            let cont_bid = if then_raises {
                                *else_block
                            } else {
                                *then_block
                            };
                            inner_consumed.insert(raise_bid);
                            // Follow raise path successors
                            if let Some(rblk) = func.blocks.get(&raise_bid) {
                                for succ in successors_of(rblk) {
                                    inner_consumed.insert(succ);
                                }
                            }
                            if !visited.insert(cont_bid) || cont_bid == inner_region.cond_block {
                                inner_consumed.insert(cont_bid);
                                break;
                            }
                            inner_consumed.insert(cont_bid);
                            cur = cont_bid;
                        }
                        _ => break,
                    }
                }
            }
            is_first_body = false;
            continue;
        }

        // Emit label for non-first body blocks (internal control flow).
        if !is_first_body {
            out.push(OpIR {
                kind: "label".to_string(),
                value: Some(block_label_id(body_bid)),
                ..OpIR::default()
            });
        } else {
            loop_inline_blocks.insert(*body_bid);
        }
        is_first_body = false;

        // Load block args.
        if let Some(param_vars) = block_param_vars.get(body_bid) {
            for (i, var_name) in param_vars.iter().enumerate() {
                if i < body_block.args.len() {
                    out.push(OpIR {
                        kind: "load_var".to_string(),
                        var: Some(var_name.clone()),
                        out: Some(value_var(body_block.args[i].id)),
                        ..OpIR::default()
                    });
                }
            }
        }

        // Emit ops.
        emit_block_ops_inner(
            body_block,
            original_to_new_label,
            label_to_block,
            block_param_vars,
            out,
        );

        // Emit terminator — replace back-edges with loop_continue,
        // exit jumps with loop_break, and other terminators normally.
        match &body_block.terminator {
            Terminator::Branch { target, args } if *target == header => {
                // Back-edge → store updated phi values, then loop_continue.
                emit_block_arg_stores(*target, args, block_param_vars, out);
                out.push(OpIR {
                    kind: "loop_continue".to_string(),
                    ..OpIR::default()
                });
            }
            Terminator::Branch { target, args } if *target == region.exit_block => {
                // Explicit break (jump to exit) → store exit args, loop_break.
                emit_block_arg_stores(*target, args, block_param_vars, out);
                out.push(OpIR {
                    kind: "loop_break".to_string(),
                    ..OpIR::default()
                });
            }
            _ => {
                // Internal control flow — emit normally.
                emit_terminator(
                    body_block,
                    block_param_vars,
                    block_label_id,
                    &loop_inline_blocks,
                    out,
                    original_has_ret,
                    &func.loop_break_kinds,
                );
            }
        }
    }

    // 8a. Emit deferred raise-path blocks.  These are dead-end blocks
    //     (end with `raise`) targeted by br_if from guard CondBranches
    //     in the header region.  They must exist within the loop as
    //     labeled blocks so the br_if targets resolve.
    for (raise_bid, raise_args, raise_path_blocks) in &deferred_raise_paths {
        emit_guard_raise_path(
            *raise_bid,
            raise_args,
            raise_path_blocks,
            func,
            block_param_vars,
            block_label_id,
            &loop_inline_blocks,
            original_to_new_label,
            label_to_block,
            out,
        );
    }

    // 8b. loop_end — seals loop_block, switches to after_block.
    out.push(OpIR {
        kind: "loop_end".to_string(),
        ..OpIR::default()
    });
    // 8c. Materialize the post-loop control-flow edge explicitly.
    //
    // The native backend resumes execution after LOOP_END in the loop's
    // after_block. When the surrounding linearization does not place the
    // loop's exit block immediately next, fallthrough can run unrelated
    // sibling code before the real post-loop continuation. Emit the edge
    // explicitly so nested loop regions inside larger branch regions retain
    // their outer merge target.
    let exit_role = func
        .loop_roles
        .get(&region.exit_block)
        .cloned()
        .unwrap_or(LoopRole::None);
    let exit_needs_fallthrough = if_inlined_blocks.contains(&region.exit_block)
        || region.exit_block == func.entry_block
        || exit_role == LoopRole::LoopHeader;
    if !exit_needs_fallthrough {
        out.push(OpIR {
            kind: "jump".to_string(),
            value: Some(block_label_id(&region.exit_block)),
            ..OpIR::default()
        });
    }

    // 8d. Emit terminal dead-end body blocks after the loop boundary. These are
    // internal branch targets that end in `ret`/`raise`/`unreachable`; keeping
    // them inside the linear loop body breaks the structured `loop_continue`
    // → `loop_end` region shape for native lowering.
    for body_bid in deferred_terminal_body_blocks {
        let Some(body_block) = func.blocks.get(&body_bid) else {
            continue;
        };
        out.push(OpIR {
            kind: "label".to_string(),
            value: Some(block_label_id(&body_bid)),
            ..OpIR::default()
        });
        if let Some(param_vars) = block_param_vars.get(&body_bid) {
            for (i, var_name) in param_vars.iter().enumerate() {
                if i < body_block.args.len() {
                    out.push(OpIR {
                        kind: "load_var".to_string(),
                        var: Some(var_name.clone()),
                        out: Some(value_var(body_block.args[i].id)),
                        ..OpIR::default()
                    });
                }
            }
        }
        emit_block_ops_inner(
            body_block,
            original_to_new_label,
            label_to_block,
            block_param_vars,
            out,
        );
        emit_terminator(
            body_block,
            block_param_vars,
            block_label_id,
            if_inlined_blocks,
            out,
            original_has_ret,
            &func.loop_break_kinds,
        );
    }
}

/// Emit the raise-path blocks for a guard CondBranch.
///
/// Follows Branch chains from `start_bid`, emitting each block with a
/// label, block arg loads, ops, and terminators.  This handles patterns
/// like guard → join → raise where the raise is 1-2 hops away.
pub(super) fn emit_guard_raise_path(
    start_bid: BlockId,
    start_args: &[ValueId],
    raise_path_blocks: &HashSet<BlockId>,
    func: &TirFunction,
    block_param_vars: &HashMap<BlockId, Vec<String>>,
    block_label_id: &dyn Fn(&BlockId) -> i64,
    if_inlined_blocks: &HashSet<BlockId>,
    original_to_new_label: &HashMap<i64, i64>,
    label_to_block: &HashMap<i64, BlockId>,
    out: &mut Vec<OpIR>,
) {
    let original_has_ret = func
        .attrs
        .get("_original_has_ret")
        .map(|v| matches!(v, AttrValue::Bool(true)))
        .unwrap_or(false);

    // Emit store_var for entry args before the first block label.
    emit_block_arg_stores(start_bid, start_args, block_param_vars, out);

    let mut cur = start_bid;
    let mut visited: HashSet<BlockId> = HashSet::new();
    while visited.insert(cur) {
        let Some(blk) = func.blocks.get(&cur) else {
            break;
        };

        // Emit label.
        out.push(OpIR {
            kind: "label".to_string(),
            value: Some(block_label_id(&cur)),
            ..OpIR::default()
        });

        // Load block args.
        if let Some(param_vars) = block_param_vars.get(&cur) {
            for (i, var_name) in param_vars.iter().enumerate() {
                if i < blk.args.len() {
                    out.push(OpIR {
                        kind: "load_var".to_string(),
                        var: Some(var_name.clone()),
                        out: Some(value_var(blk.args[i].id)),
                        ..OpIR::default()
                    });
                }
            }
        }

        // Emit ops.
        emit_block_ops_inner(
            blk,
            original_to_new_label,
            label_to_block,
            block_param_vars,
            out,
        );

        // Emit terminator and follow chain. A raise block can still branch
        // into a cleanup block that belongs to the same deferred raise path.
        // Keep materializing that chain so any handler labels it owns survive.
        let has_raise = blk.ops.iter().any(|op| op.opcode == OpCode::Raise);
        match &blk.terminator {
            Terminator::Branch { target, .. }
                if has_raise && raise_path_blocks.contains(target) =>
            {
                emit_terminator(
                    blk,
                    block_param_vars,
                    block_label_id,
                    if_inlined_blocks,
                    out,
                    original_has_ret,
                    &func.loop_break_kinds,
                );
                cur = *target;
            }
            _ if has_raise => {
                emit_terminator(
                    blk,
                    block_param_vars,
                    block_label_id,
                    if_inlined_blocks,
                    out,
                    original_has_ret,
                    &func.loop_break_kinds,
                );
                break;
            }
            Terminator::Branch { target, args } => {
                emit_block_arg_stores(*target, args, block_param_vars, out);
                cur = *target;
            }
            _ => {
                // Terminal block (raise, return, unreachable).
                emit_terminator(
                    blk,
                    block_param_vars,
                    block_label_id,
                    if_inlined_blocks,
                    out,
                    original_has_ret,
                    &func.loop_break_kinds,
                );
                break;
            }
        }
    }
}

/// Emit a block's ops with type annotations and label remapping.
/// Shared by both the main emission loop and structured loop emission.
pub(super) fn emit_block_ops_inner(
    block: &TirBlock,
    original_to_new_label: &HashMap<i64, i64>,
    original_label_to_block: &HashMap<i64, BlockId>,
    block_param_vars: &HashMap<BlockId, Vec<String>>,
    out: &mut Vec<OpIR>,
) {
    for op in &block.ops {
        if dominators::is_exception_transfer_edge(op.opcode)
            && let Some(orig_id) = attr_int(&op.attrs, "value")
            && let Some(&handler_block) = original_label_to_block.get(&orig_id)
        {
            emit_block_arg_stores(handler_block, &op.operands, block_param_vars, out);
        }
        for mut opir in lower_op_many(op) {
            annotate_lowered_op(&mut opir, op, original_to_new_label);
            out.push(opir);
        }
    }
}

// ---------------------------------------------------------------------------
// Terminator emission
/// Emit return ops for inlined if/else blocks.
pub(super) fn emit_return_ops(values: &[ValueId], original_has_ret: bool, out: &mut Vec<OpIR>) {
    if values.is_empty() {
        if original_has_ret {
            let ret_name = format!("_ret_none_{}", out.len());
            out.push(OpIR {
                kind: "const_none".to_string(),
                out: Some(ret_name.clone()),
                ..OpIR::default()
            });
            out.push(OpIR {
                kind: "ret".to_string(),
                var: Some(ret_name.clone()),
                args: Some(vec![ret_name]),
                ..OpIR::default()
            });
        } else {
            out.push(OpIR {
                kind: "ret_void".to_string(),
                ..OpIR::default()
            });
        }
    } else {
        out.push(OpIR {
            kind: "ret".to_string(),
            var: Some(value_var(values[0])),
            args: Some(values.iter().map(|v| value_var(*v)).collect()),
            ..OpIR::default()
        });
    }
}

// ---------------------------------------------------------------------------

pub(super) fn emit_terminator(
    block: &TirBlock,
    block_param_vars: &HashMap<BlockId, Vec<String>>,
    block_label_id: &dyn Fn(&BlockId) -> i64,
    if_inlined_blocks: &HashSet<BlockId>,
    out: &mut Vec<OpIR>,
    original_has_ret: bool,
    _loop_break_kinds: &HashMap<BlockId, LoopBreakKind>,
) {
    match &block.terminator {
        Terminator::Return { values } => {
            if values.is_empty() {
                if original_has_ret {
                    let ret_name = format!("_ret_none_{}", out.len());
                    out.push(OpIR {
                        kind: "const_none".to_string(),
                        out: Some(ret_name.clone()),
                        ..OpIR::default()
                    });
                    out.push(OpIR {
                        kind: "ret".to_string(),
                        var: Some(ret_name.clone()),
                        args: Some(vec![ret_name]),
                        ..OpIR::default()
                    });
                } else {
                    out.push(OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    });
                }
            } else {
                // The native backend reads the return value from `op.var`,
                // not from `op.args`.  Set both for compatibility.
                out.push(OpIR {
                    kind: "ret".to_string(),
                    var: Some(value_var(values[0])),
                    args: Some(values.iter().map(|v| value_var(*v)).collect()),
                    ..OpIR::default()
                });
            }
        }

        Terminator::Branch { target, args } => {
            emit_block_arg_stores(*target, args, block_param_vars, out);

            if if_inlined_blocks.contains(target) {
                // The target block is emitted inline without its own label, so
                // the normal edge is a real fallthrough. Emitting a jump here
                // would reference an unlabeled block and break TIR roundtrip
                // validation. This applies regardless of whether the current
                // block ends in a check_exception or a plain branch.
            } else {
                out.push(OpIR {
                    kind: "jump".to_string(),
                    value: Some(block_label_id(target)),
                    ..OpIR::default()
                });
            }
        }

        Terminator::CondBranch {
            cond,
            then_block,
            then_args,
            else_block,
            else_args,
        } => {
            // ── Structured loop break ──
            // If this block is a LoopHeader, emit loop_break_if_false/true
            // instead of br_if. The break kind determines which polarity:
            //   BreakIfFalse: `while cond:` → break when cond is false
            //   BreakIfTrue:  `for x in iter:` → break when done is true
            // Then-block = body (continue), else-block = exit (break).
            let needs_trampoline = !then_args.is_empty();
            if needs_trampoline {
                // Allocate a fresh label for the then-path trampoline.
                let trampoline_label = {
                    let max_label = out.iter().filter_map(|op| op.value).max().unwrap_or(0);
                    max_label + 1000
                };
                out.push(OpIR {
                    kind: "br_if".to_string(),
                    args: Some(vec![value_var(*cond)]),
                    value: Some(trampoline_label),
                    ..OpIR::default()
                });
                emit_block_arg_stores(*else_block, else_args, block_param_vars, out);
                out.push(OpIR {
                    kind: "jump".to_string(),
                    value: Some(block_label_id(else_block)),
                    ..OpIR::default()
                });
                out.push(OpIR {
                    kind: "label".to_string(),
                    value: Some(trampoline_label),
                    ..OpIR::default()
                });
                emit_block_arg_stores(*then_block, then_args, block_param_vars, out);
                out.push(OpIR {
                    kind: "jump".to_string(),
                    value: Some(block_label_id(then_block)),
                    ..OpIR::default()
                });
            } else {
                // No then-args: original pattern is safe.
                out.push(OpIR {
                    kind: "br_if".to_string(),
                    args: Some(vec![value_var(*cond)]),
                    value: Some(block_label_id(then_block)),
                    ..OpIR::default()
                });
                emit_block_arg_stores(*else_block, else_args, block_param_vars, out);
                out.push(OpIR {
                    kind: "jump".to_string(),
                    value: Some(block_label_id(else_block)),
                    ..OpIR::default()
                });
            }
        }

        Terminator::Switch {
            value,
            cases,
            default,
            default_args,
        } => {
            // Emit a chain of br_if checks for each case, then jump to default.
            for (case_val, target, case_args) in cases {
                out.push(OpIR {
                    kind: "switch_case".to_string(),
                    args: Some(vec![value_var(*value)]),
                    value: Some(*case_val),
                    ..OpIR::default()
                });
                emit_block_arg_stores(*target, case_args, block_param_vars, out);
                out.push(OpIR {
                    kind: "jump".to_string(),
                    value: Some(block_label_id(target)),
                    ..OpIR::default()
                });
            }
            emit_block_arg_stores(*default, default_args, block_param_vars, out);
            out.push(OpIR {
                kind: "jump".to_string(),
                value: Some(block_label_id(default)),
                ..OpIR::default()
            });
        }

        Terminator::StateDispatch {
            cases,
            default,
            default_args,
        } => {
            // The `_poll` state-machine dispatch.  Round-trip back to the
            // `state_switch` SimpleIR op the native/WASM backends re-derive their
            // dispatch from (they read `molt_obj_get_state(self)` and switch on
            // the saved state to the resume continuation that established it,
            // scanning the linear stream for `state_yield`/`state_label` ids).
            //
            // The per-edge block-argument incomings are emitted as `store_var`
            // into each target block's join-slot param vars BEFORE the dispatch:
            // - resume (case) edges first, so the values threaded across a
            //   suspend (e.g. exception-stack bookkeeping) populate the resume
            //   block's join slots, which the native backend loads on re-entry
            //   via its global `label_join_slots` mechanism;
            // - then the state-0 default (initial-entry) edge, which falls
            //   through to the default block.
            //
            // The LLVM backend does NOT consume this SimpleIR (it lowers the
            // `StateDispatch` terminator directly to a real `switch` to the real
            // resume blocks, supplying their phis); this path is exclusively for
            // the SimpleIR-consuming native/WASM state-machine lowering.
            for (_state_id, target, case_args) in cases {
                emit_block_arg_stores(*target, case_args, block_param_vars, out);
            }
            emit_block_arg_stores(*default, default_args, block_param_vars, out);
            out.push(OpIR {
                kind: "state_switch".to_string(),
                ..OpIR::default()
            });
            // State 0 (initial entry) falls through to the default block.
            out.push(OpIR {
                kind: "jump".to_string(),
                value: Some(block_label_id(default)),
                ..OpIR::default()
            });
        }

        Terminator::Unreachable => {
            if block.ops.iter().any(|op| op.opcode == OpCode::StateYield) {
                return;
            }
            out.push(OpIR {
                kind: "unreachable".to_string(),
                ..OpIR::default()
            });
        }
    }
}

/// Emit `store_var` ops to pass values to the target block's argument variables.
pub(super) fn emit_block_arg_stores(
    target: BlockId,
    args: &[ValueId],
    block_param_vars: &HashMap<BlockId, Vec<String>>,
    out: &mut Vec<OpIR>,
) {
    if let Some(param_vars) = block_param_vars.get(&target) {
        for (i, arg_val) in args.iter().enumerate() {
            if let Some(var_name) = param_vars.get(i) {
                out.push(OpIR {
                    kind: "store_var".to_string(),
                    var: Some(var_name.clone()),
                    args: Some(vec![value_var(*arg_val)]),
                    ..OpIR::default()
                });
            }
        }
    }
}
