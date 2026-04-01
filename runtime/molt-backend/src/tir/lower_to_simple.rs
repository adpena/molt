//! TIR → SimpleIR back-conversion scaffold.
//!
//! This module provides the bridge that allows TIR optimization passes to
//! benefit Cranelift and WASM backends without rewriting them.
//!
//! # Phase 1 (current)
//! Basic linearization: visits blocks in reverse-postorder, converts block
//! arguments at join points to `store_var` ops, and maps TIR terminators back
//! to SimpleIR control-flow markers.
//!
//! # Phase 2
//! Full round-trip with type annotations, phi elimination, and all OpCode
//! mappings.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use crate::ir::OpIR;

use super::blocks::{BlockId, LoopBreakKind, Terminator, TirBlock};
use super::function::TirFunction;
use super::ops::{AttrValue, OpCode, TirOp};
use super::types::TirType;
use super::values::ValueId;

thread_local! {
    static VALUE_NAME_OVERRIDES: RefCell<HashMap<ValueId, String>> = RefCell::new(HashMap::new());
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Convert a [`TirFunction`] back to a linear sequence of [`OpIR`] entries
/// suitable for the existing Cranelift/WASM/Luau backends.
///
/// The conversion linearises blocks in reverse-postorder (entry first, then
/// successors), emitting:
/// - A `label` op at the start of each non-entry block.
/// - `store_var` ops for block arguments at join points.
/// - One [`OpIR`] per [`TirOp`] in the block.
/// - Control-flow [`OpIR`] ops derived from the block's [`Terminator`].
///
/// When a `types` map is provided, the back-conversion propagates TIR type
/// refinement results into SimpleIR fast-path flags (`fast_int`, `fast_float`,
/// `type_hint`, `stack_eligible`), closing the optimisation gap where type
/// information was previously lost.
pub fn lower_to_simple_ir(func: &TirFunction, types: &HashMap<ValueId, TirType>) -> Vec<OpIR> {
    VALUE_NAME_OVERRIDES.with(|overrides| {
        let mut overrides = overrides.borrow_mut();
        overrides.clear();
        if let Some(entry_block) = func.blocks.get(&func.entry_block) {
            for (idx, arg) in entry_block.args.iter().enumerate() {
                if let Some(name) = func.param_names.get(idx) {
                    overrides.insert(arg.id, name.clone());
                }
            }
        }
    });

    let mut out = Vec::new();

    // Compute block visit order (reverse-postorder from entry).
    let rpo = reverse_postorder(func);

    // Build a BlockId → label_id mapping.  Blocks that have an original
    // SimpleIR label value (stored in label_id_map during lifting) reuse that
    // value so that check_exception / jump / br_if targets still match.
    // Blocks without a mapped label (e.g. blocks created by TIR optimisation
    // passes, or CFG blocks whose original label value coincides with another
    // block's fallback) are assigned fresh IDs guaranteed not to collide.
    let label_id_for_block: HashMap<BlockId, i64> = {
        let used_ids: HashSet<i64> = func.label_id_map.values().copied().collect();
        let max_used = used_ids.iter().copied().max().unwrap_or(0);
        let max_bid = func.blocks.keys().map(|b| b.0 as i64).max().unwrap_or(0);
        let mut next_fresh = max_used.max(max_bid) + 1;
        let mut mapping = HashMap::new();
        for bid in func.blocks.keys() {
            if let Some(&label_val) = func.label_id_map.get(&bid.0) {
                mapping.insert(*bid, label_val);
            } else {
                while used_ids.contains(&next_fresh) {
                    next_fresh += 1;
                }
                mapping.insert(*bid, next_fresh);
                next_fresh += 1;
            }
        }
        mapping
    };
    let block_label_id =
        |bid: &BlockId| -> i64 { label_id_for_block.get(bid).copied().unwrap_or(bid.0 as i64) };

    // Collect block argument info for all blocks so we can generate
    // `store_var` assignments at branch sites.
    // Map: (source_block, target_block) → Vec<(arg_value, param_var_name)>
    // We synthesise variable names for block arguments as "_bb<id>_arg<n>".

    // Build param-variable names for every block that has args.
    let block_param_vars: HashMap<BlockId, Vec<String>> = func
        .blocks
        .iter()
        .map(|(bid, block)| {
            let vars: Vec<String> = block
                .args
                .iter()
                .enumerate()
                .map(|(i, _)| format!("_bb{}_arg{}", bid.0, i))
                .collect();
            (*bid, vars)
        })
        .collect();

    // ── Exception handler target blocks ──
    // Collect blocks that are targets of check_exception ops.  These blocks
    // MUST have their label emitted regardless of loop/if membership, because
    // check_exception references them from a separate control-flow path that
    // is invisible to the structured loop/if detection.  Without this, an
    // exception handler block that happens to also be classified as a loop
    // body block or if-inlined block would lose its label, causing
    // validate_labels to fail or (worse) the backend to silently skip the
    // exception dispatch.
    let exception_handler_blocks: HashSet<BlockId> = {
        let mut handler_label_ids: HashSet<i64> = HashSet::new();
        for block in func.blocks.values() {
            for op in &block.ops {
                if op.opcode == OpCode::CheckException {
                    if let Some(AttrValue::Int(target_id)) = op.attrs.get("value") {
                        handler_label_ids.insert(*target_id);
                    }
                }
            }
        }
        // Map label IDs back to block IDs via the label_id_for_block inverse.
        let label_to_block: HashMap<i64, BlockId> = label_id_for_block
            .iter()
            .map(|(bid, lid)| (*lid, *bid))
            .collect();
        handler_label_ids
            .iter()
            .filter_map(|lid| label_to_block.get(lid).copied())
            .collect()
    };

    // ── Loop region detection (must run before if-pattern detection) ──
    // Compute loop_region_blocks first so that the if-pattern detector can
    // refuse to inline blocks that belong to a loop body.  Inlining loop
    // body blocks as if/else/end_if corrupts the loop back-edge because the
    // inlined blocks lose their labels and the loop_continue/loop_end
    // markers can no longer reach them.
    let mut loop_region_blocks: HashSet<BlockId> = HashSet::new();
    let mut header_body_chain: HashMap<BlockId, Vec<BlockId>> = HashMap::new();
    for bid in &rpo {
        let role = func
            .loop_roles
            .get(bid)
            .cloned()
            .unwrap_or(super::blocks::LoopRole::None);
        if role != super::blocks::LoopRole::LoopHeader {
            continue;
        }
        let Some(block) = func.blocks.get(bid) else {
            continue;
        };
        // Collect loop body blocks via DFS from ALL successors of the header,
        // stopping at the header itself (back-edges), LoopEnd blocks, and
        // blocks past LoopEnd in RPO.  This approach does NOT assume the
        // header's first CondBranch is the loop break — it may be an
        // if-statement guard (e.g. sentinel check) inside the loop body.
        // The real break CondBranch is identified after the region is built
        // by checking which CondBranch has an arm exiting the region.
        let end_block_bid = func.loop_pairs.get(bid).copied();
        let end_rpo_pos = end_block_bid.and_then(|eb| rpo.iter().position(|b| *b == eb));
        let mut region: HashSet<BlockId> = HashSet::new();

        // DFS from all successors of the header block.
        let mut stack: Vec<BlockId> = block_successors(block);
        while let Some(region_bid) = stack.pop() {
            // Stop at back-edges to the header.
            if region_bid == *bid { continue; }
            // Stop at LoopEnd blocks.
            let succ_role = func.loop_roles.get(&region_bid).cloned()
                .unwrap_or(super::blocks::LoopRole::None);
            if succ_role == super::blocks::LoopRole::LoopEnd { continue; }
            // Don't traverse past the end block in RPO.
            let succ_rpo = rpo.iter().position(|b| *b == region_bid);
            if let (Some(s_pos), Some(e_pos)) = (succ_rpo, end_rpo_pos) {
                if s_pos > e_pos { continue; }
            }
            if !region.insert(region_bid) { continue; }
            let Some(rb) = func.blocks.get(&region_bid) else { continue; };
            for succ in block_successors(rb) {
                stack.push(succ);
            }
        }

        let chain: Vec<BlockId> = rpo
            .iter()
            .filter(|candidate| region.contains(candidate))
            .copied()
            .collect();
        loop_region_blocks.extend(chain.iter().copied());
        header_body_chain.insert(*bid, chain);
    }

    // ── Identify the actual break CondBranch for each loop ──
    // Prefer the structured loop-break block recorded during SimpleIR → TIR
    // lowering.  That metadata is authoritative: it points to the original
    // top-level `loop_break_if_*` for the loop and avoids mistaking early
    // returns or exception guards inside the body for the loop's exit test.
    //
    // For synthetic TIR that lacks this metadata, fall back to a CFG scan.
    let mut loop_break_blocks: HashMap<BlockId, BlockId> = func.loop_break_blocks.clone();
    for bid in &rpo {
        let role = func.loop_roles.get(bid).cloned()
            .unwrap_or(super::blocks::LoopRole::None);
        if role != super::blocks::LoopRole::LoopHeader { continue; }
        if loop_break_blocks.contains_key(bid) {
            continue;
        }
        let region_chain = header_body_chain.get(bid).cloned().unwrap_or_default();
        let region_set: HashSet<BlockId> = region_chain.iter().copied().collect();

        // Check header itself first.
        if let Some(block) = func.blocks.get(bid) {
            if let Terminator::CondBranch { then_block, else_block, .. } = &block.terminator {
                let then_in_region = region_set.contains(then_block) || *then_block == *bid;
                let else_in_region = region_set.contains(else_block) || *else_block == *bid;
                if then_in_region != else_in_region {
                    loop_break_blocks.insert(*bid, *bid);
                    continue;
                }
            }
        }

        // Check header's direct Branch target.
        if let Some(block) = func.blocks.get(bid) {
            if let Terminator::Branch { target, .. } = &block.terminator {
                if let Some(cond_blk) = func.blocks.get(target) {
                    if let Terminator::CondBranch { then_block, else_block, .. } = &cond_blk.terminator {
                        let then_in_region = region_set.contains(then_block) || *then_block == *bid;
                        let else_in_region = region_set.contains(else_block) || *else_block == *bid;
                        if then_in_region != else_in_region {
                            loop_break_blocks.insert(*bid, *target);
                            continue;
                        }
                    }
                }
            }
        }

        // Walk deeper: scan region blocks for the CondBranch where one arm
        // exits the region.  This handles loops where if-statements appear
        // before the actual break condition (e.g. sentinel checks).
        for region_bid in &region_chain {
            let Some(region_block) = func.blocks.get(region_bid) else { continue };
            if let Terminator::CondBranch { then_block, else_block, .. } = &region_block.terminator {
                let then_in_region = region_set.contains(then_block) || *then_block == *bid;
                let else_in_region = region_set.contains(else_block) || *else_block == *bid;
                if then_in_region != else_in_region {
                    loop_break_blocks.insert(*bid, *region_bid);
                    break;
                }
            }
        }
    }

    // ── Structured if/else/end_if detection ──
    // Detect simple CondBranch patterns where both successors:
    //   (a) have no check_exception ops (which require label blocks for implicit edges)
    //   (b) have simple terminators (Branch to same join block, or Return/Unreachable)
    //   (c) are not claimed by another pattern or loop region
    //   (d) neither successor is part of a loop region (loop body blocks need
    //       their own labels for back-edge resolution)
    //
    // These patterns are emitted as if/else/end_if + phi ops, producing
    // cleaner CLIF without extra unsealed label blocks.
    struct IfPattern {
        then_bid: BlockId,
        else_bid: BlockId,
        join_bid: Option<BlockId>,
    }
    let mut if_patterns: HashMap<BlockId, IfPattern> = HashMap::new();
    let mut if_inlined_blocks: HashSet<BlockId> = HashSet::new();

    for bid in &rpo {
        let role = func.loop_roles.get(bid).cloned()
            .unwrap_or(super::blocks::LoopRole::None);
        if role != super::blocks::LoopRole::None { continue; }
        // Skip blocks that are inside a loop region — their CondBranch
        // successors are part of the loop body and must keep their labels.
        if loop_region_blocks.contains(bid) { continue; }
        let Some(block) = func.blocks.get(bid) else { continue };
        let Terminator::CondBranch { then_block, else_block, .. } = &block.terminator else { continue };
        let (then_bid, else_bid) = (*then_block, *else_block);
        if then_bid == else_bid { continue; }
        // Successor blocks that are part of a loop region must not be
        // inlined — they need their own labels for loop back-edges.
        let then_role = func.loop_roles.get(&then_bid).cloned()
            .unwrap_or(super::blocks::LoopRole::None);
        let else_role = func.loop_roles.get(&else_bid).cloned()
            .unwrap_or(super::blocks::LoopRole::None);
        if then_role != super::blocks::LoopRole::None
            || else_role != super::blocks::LoopRole::None
            || loop_region_blocks.contains(&then_bid)
            || loop_region_blocks.contains(&else_bid)
        {
            continue;
        }
        let Some(then_blk) = func.blocks.get(&then_bid) else { continue };
        let Some(else_blk) = func.blocks.get(&else_bid) else { continue };
        if if_inlined_blocks.contains(&then_bid) || if_inlined_blocks.contains(&else_bid) { continue; }
        // No check_exception in successors — those need labels for implicit edges.
        if then_blk.ops.iter().any(|op| op.opcode == OpCode::CheckException) { continue; }
        if else_blk.ops.iter().any(|op| op.opcode == OpCode::CheckException) { continue; }
        // Simple terminators only.
        let then_target = match &then_blk.terminator {
            Terminator::Branch { target, .. } => Some(*target),
            Terminator::Return { .. } | Terminator::Unreachable => None,
            _ => { continue; }
        };
        let else_target = match &else_blk.terminator {
            Terminator::Branch { target, .. } => Some(*target),
            Terminator::Return { .. } | Terminator::Unreachable => None,
            _ => { continue; }
        };
        let join_bid = match (then_target, else_target) {
            (Some(t), Some(e)) if t == e => Some(t),
            (Some(t), None) => Some(t),
            (None, Some(e)) => Some(e),
            (None, None) => None,
            _ => { continue; }
        };
        if_patterns.insert(*bid, IfPattern { then_bid, else_bid, join_bid });
        if_inlined_blocks.insert(then_bid);
        if_inlined_blocks.insert(else_bid);
    }

    // Build the emission order: RPO but with loop exit blocks deferred
    // until after their loop region.  Without this, RPO can place the
    // exit block before the loop body, causing the native backend to
    // execute after-loop code before the loop body.
    //
    // We do NOT modify RPO.  Instead, during emission we skip exit blocks
    // when encountered in RPO and emit them immediately after loop_end.
    // This is tracked in `deferred_exits`: header_bid → exit_bid.
    let mut deferred_exits: HashMap<BlockId, BlockId> = HashMap::new();
    // Use the validated loop_break_blocks map to determine exit blocks.
    // This correctly handles loops where if-statements precede the real
    // break condition (e.g. sentinel checks before `while j <= n`).
    for bid in &rpo {
        let role = func.loop_roles.get(bid).cloned()
            .unwrap_or(super::blocks::LoopRole::None);
        if role != super::blocks::LoopRole::LoopHeader { continue; }
        if let Some(break_bid) = loop_break_blocks.get(bid) {
            if let Some(break_block) = func.blocks.get(break_bid) {
                if let Terminator::CondBranch { then_block, else_block, .. } = &break_block.terminator {
                    let region_chain = header_body_chain.get(bid).cloned().unwrap_or_default();
                    let region_set: HashSet<BlockId> = region_chain.iter().copied().collect();
                    let exit = if !region_set.contains(then_block) && *then_block != *bid {
                        *then_block
                    } else {
                        *else_block
                    };
                    deferred_exits.insert(*bid, exit);
                }
            }
        }
    }
    // Set of exit blocks that should be deferred.
    let deferred_exit_set: HashSet<BlockId> = deferred_exits.values().copied().collect();

    // Blocks that are emitted inline as part of a nested loop within an
    // outer loop's body chain.  These must be skipped in the RPO main loop
    // to avoid double-emission at an incorrect position (after the function
    // return, causing infinite loops).
    let mut emitted_inline: HashSet<BlockId> = HashSet::new();

    for bid in &rpo {
        // Exception handler target blocks are NEVER skipped — their labels
        // must be emitted so that check_exception dispatch can reach them.
        let is_exc_handler = exception_handler_blocks.contains(bid);

        // Skip deferred exit blocks — they'll be emitted after loop_end.
        if !is_exc_handler && deferred_exit_set.contains(bid) && !loop_region_blocks.contains(bid) {
            continue;
        }

        // Skip blocks in loop regions — they're emitted with proper
        // labels, ops, and terminators inside the header's
        // loop_start/loop_end region.
        let loop_role = func
            .loop_roles
            .get(bid)
            .cloned()
            .unwrap_or(super::blocks::LoopRole::None);
        if !is_exc_handler && loop_region_blocks.contains(bid) && loop_role != super::blocks::LoopRole::LoopHeader {
            continue;
        }
        // Skip LoopEnd blocks — structural markers from the original
        // SimpleIR.  The TIR roundtrip emits loop_continue + loop_end
        // via back-edge detection in the loop body handler.
        if !is_exc_handler && loop_role == super::blocks::LoopRole::LoopEnd {
            continue;
        }

        // Skip blocks inlined inside structured if/else/end_if regions.
        // Exception handler blocks override this even if they contain no
        // check_exception ops — they may be the TARGET of one.
        if !is_exc_handler && if_inlined_blocks.contains(bid) {
            continue;
        }

        // Skip blocks that were emitted inline as part of a nested loop
        // within an outer loop's body.  Without this, nested loop headers
        // and their body/exit blocks would be emitted twice: once inline
        // at the correct position within the outer loop, and again here
        // at the RPO position (which may be after the function return).
        if !is_exc_handler && emitted_inline.contains(bid) {
            continue;
        }

        let block = match func.blocks.get(bid) {
            Some(b) => b,
            None => continue,
        };

        // Emit loop_start before loop header blocks.
        if loop_role == super::blocks::LoopRole::LoopHeader {
            // Emit a label for the loop header so that check_exception
            // targets referencing this block's label ID remain valid.
            if *bid != func.entry_block {
                out.push(OpIR {
                    kind: "label".to_string(),
                    value: Some(block_label_id(bid)),
                    ..OpIR::default()
                });
            }
            out.push(OpIR {
                kind: "loop_start".to_string(),
                ..OpIR::default()
            });
        }

        // Loop headers fall through from the preheader into the structured
        // region; re-entering through a label would execute loop_index_start
        // on every iteration.
        if *bid != func.entry_block && loop_role != super::blocks::LoopRole::LoopHeader {
            out.push(OpIR {
                kind: "label".to_string(),
                value: Some(block_label_id(bid)),
                ..OpIR::default()
            });

            // Load block argument variables into SSA-named vars.
            if let Some(param_vars) = block_param_vars.get(bid) {
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
        }

        // Helper: emit a block's ops with type annotation.
        // Phi Copy ops from the original SimpleIR are handled specially:
        // - In linearised blocks (no block args), the CondBranch was folded
        //   and the phi becomes copy_var from the last operand.
        // - In if-pattern join blocks (has block args), the phi is redundant
        //   (the if-pattern emission + load_var handle it) — skip.
        let emit_block_ops = |block: &TirBlock, out: &mut Vec<OpIR>| {
            let has_block_args = !block.args.is_empty();
            for op in &block.ops {
                // Detect phi Copy ops (original_kind="phi" with 2+ operands).
                if op.opcode == OpCode::Copy
                    && op.attrs.get("_original_kind")
                        .map(|v| matches!(v, super::ops::AttrValue::Str(s) if s == "phi"))
                        .unwrap_or(false)
                    && op.operands.len() >= 2
                {
                    if has_block_args {
                        // Join block with block args: phi is handled by
                        // if-pattern emission + load_var.  Skip it.
                        continue;
                    }
                    // Linearised block: emit copy_var from last operand.
                    if let Some(dst) = op.results.first() {
                        out.push(OpIR {
                            kind: "copy_var".to_string(),
                            var: Some(value_var(*op.operands.last().unwrap())),
                            out: Some(value_var(*dst)),
                            ..OpIR::default()
                        });
                    }
                    continue;
                }
                if let Some(mut opir) = lower_op(op) {
                    annotate_type_flags(&mut opir, op, types);
                    out.push(opir);
                }
            }
        };

        // For loop headers: emit the header's ops and loop break condition,
        // then emit each body block with proper labels, ops, and terminators
        // (including br_if/jump for CondBranch) inside the loop_start/loop_end
        // region.  Blocks that branch back to the header emit loop_continue +
        // loop_end instead of a jump.
        //
        // Previous approach tried to inline all body blocks linearly, which
        // broke when a loop body contained CondBranch terminators (e.g.
        // if-statements inside while loops) — the conditional was silently
        // dropped, causing infinite loops or wrong results.
        if loop_role == super::blocks::LoopRole::LoopHeader {
            let break_kind = func
                .loop_break_kinds
                .get(bid)
                .copied()
                .unwrap_or(LoopBreakKind::BreakIfTrue);
            let region_chain = header_body_chain.get(bid).cloned().unwrap_or_default();
            let mut loop_backedge_target = *bid;
            let original_has_ret = func.attrs.get("_original_has_ret")
                .map(|v| matches!(v, super::ops::AttrValue::Bool(true)))
                .unwrap_or(false);

            // Helper: emit a block's label and load its block arguments
            // from store_var slots.
            let emit_block_header = |rbid: &BlockId, rblock: &TirBlock, out: &mut Vec<OpIR>| {
                out.push(OpIR {
                    kind: "label".to_string(),
                    value: Some(block_label_id(rbid)),
                    ..OpIR::default()
                });
                if let Some(param_vars) = block_param_vars.get(rbid) {
                    for (i, var_name) in param_vars.iter().enumerate() {
                        if i < rblock.args.len() {
                            out.push(OpIR {
                                kind: "load_var".to_string(),
                                var: Some(var_name.clone()),
                                out: Some(value_var(rblock.args[i].id)),
                                ..OpIR::default()
                            });
                        }
                    }
                }
            };

            // Helper: emit a block's terminator, but replace branches back
            // to the loop header with loop_continue + loop_end.
            let emit_body_terminator =
                |rblock: &TirBlock, backedge_target: BlockId, out: &mut Vec<OpIR>| {
                match &rblock.terminator {
                    Terminator::Branch { target, args } => {
                        let target_role = func
                            .loop_roles
                            .get(target)
                            .cloned()
                            .unwrap_or(super::blocks::LoopRole::None);
                        if target_role == super::blocks::LoopRole::LoopEnd {
                            if let Some(loop_end_block) = func.blocks.get(target)
                                && let Terminator::Branch {
                                    target: continue_target,
                                    args: continue_args,
                                } = &loop_end_block.terminator
                            {
                                emit_block_arg_stores(
                                    *continue_target,
                                    continue_args,
                                    &block_param_vars,
                                    out,
                                );
                            }
                            out.push(OpIR {
                                kind: "loop_continue".to_string(),
                                ..OpIR::default()
                            });
                            out.push(OpIR {
                                kind: "loop_end".to_string(),
                                ..OpIR::default()
                            });
                        } else if *target == backedge_target {
                            emit_block_arg_stores(*target, args, &block_param_vars, out);
                            // Back-edge to loop header: loop_continue + loop_end.
                            out.push(OpIR {
                                kind: "loop_continue".to_string(),
                                ..OpIR::default()
                            });
                            out.push(OpIR {
                                kind: "loop_end".to_string(),
                                ..OpIR::default()
                            });
                        } else {
                            emit_block_arg_stores(*target, args, &block_param_vars, out);
                            let last_op_is_check_exception = rblock.ops.last()
                                .map(|op| op.opcode == OpCode::CheckException)
                                .unwrap_or(false);
                            if !last_op_is_check_exception {
                                out.push(OpIR {
                                    kind: "jump".to_string(),
                                    value: Some(block_label_id(target)),
                                    ..OpIR::default()
                                });
                            }
                        }
                    }
                    Terminator::CondBranch {
                        cond,
                        then_block,
                        then_args,
                        else_block,
                        else_args,
                    } => {
                        // Check if this CondBranch is itself a nested loop header.
                        let block_loop_role = func.loop_roles.get(&rblock.id).cloned()
                            .unwrap_or(super::blocks::LoopRole::None);
                        if block_loop_role == super::blocks::LoopRole::LoopHeader {
                            // Nested loop header: handled by recursive loop emission.
                            // This shouldn't happen since nested loop headers are
                            // separate entries in the RPO, but handle defensively.
                            emit_block_arg_stores(*then_block, then_args, &block_param_vars, out);
                            let mut br_op = OpIR {
                                kind: "br_if".to_string(),
                                args: Some(vec![value_var(*cond)]),
                                value: Some(block_label_id(then_block)),
                                ..OpIR::default()
                            };
                            annotate_cond_type_hint(&mut br_op, *cond, types);
                            out.push(br_op);
                            emit_block_arg_stores(*else_block, else_args, &block_param_vars, out);
                            out.push(OpIR {
                                kind: "jump".to_string(),
                                value: Some(block_label_id(else_block)),
                                ..OpIR::default()
                            });
                        } else {
                            // Generic conditional branch inside loop body.
                            emit_block_arg_stores(*then_block, then_args, &block_param_vars, out);
                            let mut br_op = OpIR {
                                kind: "br_if".to_string(),
                                args: Some(vec![value_var(*cond)]),
                                value: Some(block_label_id(then_block)),
                                ..OpIR::default()
                            };
                            annotate_cond_type_hint(&mut br_op, *cond, types);
                            out.push(br_op);
                            emit_block_arg_stores(*else_block, else_args, &block_param_vars, out);
                            out.push(OpIR {
                                kind: "jump".to_string(),
                                value: Some(block_label_id(else_block)),
                                ..OpIR::default()
                            });
                        }
                    }
                    Terminator::Return { values } => {
                        emit_return_ops(values, original_has_ret, out);
                    }
                    Terminator::Unreachable => {
                        out.push(OpIR {
                            kind: "unreachable".to_string(),
                            ..OpIR::default()
                        });
                    }
                    _ => {}
                }
            };

            // Determine loop condition data from the header's terminator.
            // The header may have a direct CondBranch, or it may Branch to
            // the first region block which then has the CondBranch.
            //
            // IMPORTANT: Only use a CondBranch as the loop break if it was
            // validated by loop_break_blocks (one arm exits the region).
            // If the header's CondBranch is an if-statement guard (e.g.
            // sentinel check), it must NOT be treated as the loop break.
            let mut cond_data: Option<(ValueId, BlockId, Vec<ValueId>, BlockId, Vec<ValueId>)> = None;
            let mut inlined_cond_block: Option<BlockId> = None;

            // Check if the validated break block is the header or its direct successor.
            let break_block_bid = loop_break_blocks.get(bid).copied();
            let break_is_header = break_block_bid == Some(*bid);
            let break_is_header_successor = break_block_bid.map(|bbid| {
                if let Terminator::Branch { target, .. } = &block.terminator {
                    bbid == *target
                } else {
                    false
                }
            }).unwrap_or(false);

            match &block.terminator {
                Terminator::CondBranch {
                    cond,
                    then_block,
                    then_args,
                    else_block,
                    else_args,
                } if break_is_header => {
                    emit_block_ops(block, &mut out);
                    cond_data = Some((
                        *cond,
                        *then_block,
                        then_args.clone(),
                        *else_block,
                        else_args.clone(),
                    ));
                }
                Terminator::Branch { target, args } if break_is_header_successor => {
                    emit_block_ops(block, &mut out);
                    // The header branches to the break condition block.
                    // Inline it as the loop condition.
                    if let Some(cond_block) = func.blocks.get(target)
                        && let Terminator::CondBranch {
                            cond,
                            then_block,
                            then_args,
                            else_block,
                            else_args,
                        } = &cond_block.terminator
                    {
                        loop_backedge_target = *target;
                        // Emit the cond block's ops inline with arg mapping.
                        let resolved: Vec<(ValueId, String)> = cond_block
                            .args
                            .iter()
                            .enumerate()
                            .filter_map(|(i, arg)| {
                                args.get(i).map(|&pred| (arg.id, value_var(pred)))
                            })
                            .collect();
                        VALUE_NAME_OVERRIDES.with(|overrides| {
                            let mut map = overrides.borrow_mut();
                            for (id, name) in &resolved {
                                map.insert(*id, name.clone());
                            }
                        });
                        for op in &cond_block.ops {
                            if let Some(mut opir) = lower_op(op) {
                                annotate_type_flags(&mut opir, op, types);
                                out.push(opir);
                            }
                        }
                        inlined_cond_block = Some(*target);
                        cond_data = Some((
                            *cond,
                            *then_block,
                            then_args.clone(),
                            *else_block,
                            else_args.clone(),
                        ));
                    }
                }
                _ => {
                    emit_block_ops(block, &mut out);
                }
            }

            if let Some((cond, then_block, then_args, else_block, else_args)) = cond_data {
                let (after_block, after_args, body_block, body_args) = match break_kind {
                    LoopBreakKind::BreakIfTrue => {
                        (then_block, then_args, else_block, else_args)
                    }
                    LoopBreakKind::BreakIfFalse => {
                        (else_block, else_args, then_block, then_args)
                    }
                };

                // Emit loop break condition.
                emit_block_arg_stores(after_block, &after_args, &block_param_vars, &mut out);
                let mut break_op = OpIR {
                    kind: match break_kind {
                        LoopBreakKind::BreakIfTrue => "loop_break_if_true".to_string(),
                        LoopBreakKind::BreakIfFalse => "loop_break_if_false".to_string(),
                    },
                    args: Some(vec![value_var(cond)]),
                    ..OpIR::default()
                };
                annotate_cond_type_hint(&mut break_op, cond, types);
                out.push(break_op);

                // Store args for the first body block entry.
                emit_block_arg_stores(body_block, &body_args, &block_param_vars, &mut out);

                // Emit each body block in the region chain with proper
                // labels, ops, and terminators.  This preserves control
                // flow for CondBranch terminators (if-statements) inside
                // the loop body.
                let body_blocks: Vec<BlockId> = region_chain
                    .iter()
                    .copied()
                    .filter(|candidate| {
                        Some(*candidate) != inlined_cond_block
                            && func
                                .loop_roles
                                .get(candidate)
                                .cloned()
                                .unwrap_or(super::blocks::LoopRole::None)
                                != super::blocks::LoopRole::LoopEnd
                    })
                    .collect();

                // Track whether we've emitted loop_end yet.
                let mut emitted_loop_end = false;

                for region_bid in &body_blocks {
                    if emitted_inline.contains(region_bid) {
                        continue;
                    }
                    let nested_role = func.loop_roles.get(region_bid).cloned()
                        .unwrap_or(super::blocks::LoopRole::None);
                    if nested_role == super::blocks::LoopRole::LoopHeader {
                        // Nested loop header: emit the FULL loop structure
                        // inline within the outer loop's body.  The RPO main
                        // loop would emit this at an incorrect position
                        // (potentially after the function return), so we
                        // handle it here and mark all involved blocks as
                        // emitted_inline to prevent double-emission.
                        emit_nested_loop(
                            func,
                            region_bid,
                            bid,
                            &header_body_chain,
                            &block_param_vars,
                            &block_label_id,
                            &deferred_exits,
                            &loop_break_blocks,
                            types,
                            original_has_ret,
                            &mut emitted_inline,
                            &mut out,
                        );
                        continue;
                    }
                    // Skip blocks that are in a nested loop's body chain —
                    // they're emitted by the nested loop header above.
                    let mut in_nested_body = false;
                    for (hdr, chain) in &header_body_chain {
                        if *hdr != *bid && chain.contains(region_bid) {
                            in_nested_body = true;
                            break;
                        }
                    }
                    if in_nested_body { continue; }

                    if let Some(region_block) = func.blocks.get(region_bid) {
                        emit_block_header(region_bid, region_block, &mut out);
                        emit_block_ops(region_block, &mut out);
                        emit_body_terminator(region_block, loop_backedge_target, &mut out);
                        // Check if this block's terminator emitted loop_end
                        // (i.e. it branches back to the header).
                        if let Terminator::Branch { target, .. } = &region_block.terminator {
                            let target_role = func
                                .loop_roles
                                .get(target)
                                .cloned()
                                .unwrap_or(super::blocks::LoopRole::None);
                            if *target == loop_backedge_target
                                || target_role == super::blocks::LoopRole::LoopEnd
                            {
                                emitted_loop_end = true;
                            }
                        }
                    }
                }

                // If no body block branched back to the header (e.g. the
                // body_block itself is the only block and was handled above,
                // or the region chain is empty), emit the body block directly
                // and close the loop.
                if !emitted_loop_end {
                    if body_blocks.is_empty() {
                        // The body is a single block not in the region chain.
                        if let Some(body_block_ir) = func.blocks.get(&body_block) {
                            emit_block_header(&body_block, body_block_ir, &mut out);
                            emit_block_ops(body_block_ir, &mut out);
                            emit_body_terminator(body_block_ir, loop_backedge_target, &mut out);
                            // If it still didn't emit loop_end, force it.
                            let closes_loop = match &body_block_ir.terminator {
                                Terminator::Branch { target, .. } => {
                                    let target_role = func
                                        .loop_roles
                                        .get(target)
                                        .cloned()
                                        .unwrap_or(super::blocks::LoopRole::None);
                                    *target == loop_backedge_target
                                        || target_role == super::blocks::LoopRole::LoopEnd
                                }
                                _ => false,
                            };
                            if !closes_loop {
                                out.push(OpIR {
                                    kind: "loop_continue".to_string(),
                                    ..OpIR::default()
                                });
                                out.push(OpIR {
                                    kind: "loop_end".to_string(),
                                    ..OpIR::default()
                                });
                            }
                        } else {
                            out.push(OpIR {
                                kind: "loop_continue".to_string(),
                                ..OpIR::default()
                            });
                            out.push(OpIR {
                                kind: "loop_end".to_string(),
                                ..OpIR::default()
                            });
                        }
                    } else {
                        // Region chain blocks didn't branch back to header.
                        // This can happen when the back-edge goes through a
                        // block not in the region chain.  Emit fallback
                        // loop_continue + loop_end.
                        out.push(OpIR {
                            kind: "loop_continue".to_string(),
                            ..OpIR::default()
                        });
                        out.push(OpIR {
                            kind: "loop_end".to_string(),
                            ..OpIR::default()
                        });
                    }
                }

                // Emit the deferred exit block immediately after loop_end
                // so the after-loop code follows the loop body, not precedes it.
                if let Some(exit_bid) = deferred_exits.get(bid) {
                    if let Some(exit_block) = func.blocks.get(exit_bid) {
                        out.push(OpIR {
                            kind: "label".to_string(),
                            value: Some(block_label_id(exit_bid)),
                            ..OpIR::default()
                        });
                        if let Some(param_vars) = block_param_vars.get(exit_bid) {
                            for (i, var_name) in param_vars.iter().enumerate() {
                                if i < exit_block.args.len() {
                                    out.push(OpIR {
                                        kind: "load_var".to_string(),
                                        var: Some(var_name.clone()),
                                        out: Some(value_var(exit_block.args[i].id)),
                                        ..OpIR::default()
                                    });
                                }
                            }
                        }
                        emit_block_ops(exit_block, &mut out);
                        let exit_role = func.loop_roles.get(exit_bid).cloned()
                            .unwrap_or(super::blocks::LoopRole::None);
                        emit_terminator(
                            exit_block,
                            &block_param_vars,
                            &block_label_id,
                            &func.loop_roles,
                            &mut out,
                            original_has_ret,
                            exit_role,
                            types,
                        );
                    }
                }
            } else {
                // The break CondBranch is deeper in the body (not the header
                // or its direct successor).  Emit the header's terminator
                // normally, then emit all body blocks.  The break block's
                // CondBranch will be converted to loop_break_if_* in
                // emit_body_terminator_with_break.
                let original_has_ret = func.attrs.get("_original_has_ret")
                    .map(|v| matches!(v, super::ops::AttrValue::Bool(true)))
                    .unwrap_or(false);

                // Emit the header's own terminator as br_if/jump.
                emit_terminator(
                    block,
                    &block_param_vars,
                    &block_label_id,
                    &func.loop_roles,
                    &mut out,
                    original_has_ret,
                    loop_role,
                    types,
                );

                // Emit body blocks with the break block handled specially.
                let body_blocks: Vec<BlockId> = region_chain
                    .iter()
                    .copied()
                    .filter(|candidate| {
                        Some(*candidate) != inlined_cond_block
                            && func
                                .loop_roles
                                .get(candidate)
                                .cloned()
                                .unwrap_or(super::blocks::LoopRole::None)
                                != super::blocks::LoopRole::LoopEnd
                    })
                    .collect();

                let mut emitted_loop_end = false;

                for region_bid in &body_blocks {
                    if emitted_inline.contains(region_bid) { continue; }
                    let nested_role = func.loop_roles.get(region_bid).cloned()
                        .unwrap_or(super::blocks::LoopRole::None);
                    if nested_role == super::blocks::LoopRole::LoopHeader {
                        emit_nested_loop(
                            func,
                            region_bid,
                            bid,
                            &header_body_chain,
                            &block_param_vars,
                            &block_label_id,
                            &deferred_exits,
                            &loop_break_blocks,
                            types,
                            original_has_ret,
                            &mut emitted_inline,
                            &mut out,
                        );
                        continue;
                    }
                    let mut in_nested_body = false;
                    for (hdr, chain) in &header_body_chain {
                        if *hdr != *bid && chain.contains(region_bid) {
                            in_nested_body = true;
                            break;
                        }
                    }
                    if in_nested_body { continue; }

                    if let Some(region_block) = func.blocks.get(region_bid) {
                        emit_block_header(region_bid, region_block, &mut out);
                        emit_block_ops(region_block, &mut out);

                        // Check if this is the break block for this loop.
                        if Some(*region_bid) == break_block_bid {
                            if let Terminator::CondBranch { cond, then_block, then_args, else_block, else_args } = &region_block.terminator {
                                let region_set: HashSet<BlockId> = body_blocks.iter().copied().collect();
                                let then_in_region = region_set.contains(then_block) || *then_block == *bid;
                                let (after_block, after_args, body_target, body_target_args) = if !then_in_region {
                                    // then exits, else continues
                                    (*then_block, then_args.clone(), *else_block, else_args.clone())
                                } else {
                                    // else exits, then continues
                                    (*else_block, else_args.clone(), *then_block, then_args.clone())
                                };
                                emit_block_arg_stores(after_block, &after_args, &block_param_vars, &mut out);
                                let break_kind_str = if !then_in_region {
                                    "loop_break_if_true"
                                } else {
                                    "loop_break_if_false"
                                };
                                let mut break_op = OpIR {
                                    kind: break_kind_str.to_string(),
                                    args: Some(vec![value_var(*cond)]),
                                    ..OpIR::default()
                                };
                                annotate_cond_type_hint(&mut break_op, *cond, types);
                                out.push(break_op);
                                emit_block_arg_stores(body_target, &body_target_args, &block_param_vars, &mut out);
                            }
                        } else {
                            emit_body_terminator(region_block, loop_backedge_target, &mut out);
                            if let Terminator::Branch { target, .. } = &region_block.terminator {
                                let target_role = func.loop_roles.get(target).cloned()
                                    .unwrap_or(super::blocks::LoopRole::None);
                                if *target == loop_backedge_target
                                    || target_role == super::blocks::LoopRole::LoopEnd
                                {
                                    emitted_loop_end = true;
                                }
                            }
                        }
                    }
                }

                if !emitted_loop_end {
                    out.push(OpIR { kind: "loop_continue".to_string(), ..OpIR::default() });
                    out.push(OpIR { kind: "loop_end".to_string(), ..OpIR::default() });
                }

                // Emit deferred exit block.
                if let Some(exit_bid) = deferred_exits.get(bid) {
                    if let Some(exit_block) = func.blocks.get(exit_bid) {
                        out.push(OpIR {
                            kind: "label".to_string(),
                            value: Some(block_label_id(exit_bid)),
                            ..OpIR::default()
                        });
                        if let Some(param_vars) = block_param_vars.get(exit_bid) {
                            for (i, var_name) in param_vars.iter().enumerate() {
                                if i < exit_block.args.len() {
                                    out.push(OpIR {
                                        kind: "load_var".to_string(),
                                        var: Some(var_name.clone()),
                                        out: Some(value_var(exit_block.args[i].id)),
                                        ..OpIR::default()
                                    });
                                }
                            }
                        }
                        emit_block_ops(exit_block, &mut out);
                        let exit_role = func.loop_roles.get(exit_bid).cloned()
                            .unwrap_or(super::blocks::LoopRole::None);
                        emit_terminator(
                            exit_block,
                            &block_param_vars,
                            &block_label_id,
                            &func.loop_roles,
                            &mut out,
                            original_has_ret,
                            exit_role,
                            types,
                        );
                    }
                }
            }
        } else if let Some(pattern) = if_patterns.get(bid) {
            // ── Structured if/else/end_if emission ──
            // Emit the current block's ops, then inline the then/else
            // blocks between if/else/end_if markers with phi ops.
            emit_block_ops(block, &mut out);

            let Terminator::CondBranch { cond, .. } = &block.terminator else {
                unreachable!();
            };

            let then_blk = func.blocks.get(&pattern.then_bid).expect("then block missing");
            let else_blk = func.blocks.get(&pattern.else_bid).expect("else block missing");
            let original_has_ret = func.attrs.get("_original_has_ret")
                .map(|v| matches!(v, super::ops::AttrValue::Bool(true)))
                .unwrap_or(false);

            // Resolve the join block's block-arg slots and the values each
            // branch contributes. The join block itself will load these via
            // load_var when its label is emitted later.
            let join_arg_stores: Vec<(String, String, String)> = if let Some(join_bid) = pattern.join_bid {
                let join_blk = func.blocks.get(&join_bid);
                let join_param_count = join_blk.map(|b| b.args.len()).unwrap_or(0);
                let join_param_vars = block_param_vars.get(&join_bid);
                let then_branch_args = match &then_blk.terminator {
                    Terminator::Branch { args, .. } => args.as_slice(),
                    _ => &[],
                };
                let else_branch_args = match &else_blk.terminator {
                    Terminator::Branch { args, .. } => args.as_slice(),
                    _ => &[],
                };
                (0..join_param_count)
                    .filter_map(|i| {
                        let join_arg_name = join_blk
                            .and_then(|b| b.args.get(i))
                            .map(|a| value_var(a.id))?;
                        let join_slot_name = join_param_vars
                            .and_then(|vars| vars.get(i))
                            .cloned()
                            .unwrap_or_else(|| format!("_bb{}_arg{}", join_bid.0, i));
                        let then_val = then_branch_args.get(i).map(|v| value_var(*v))
                            .unwrap_or_else(|| join_arg_name.clone());
                        let else_val = else_branch_args.get(i).map(|v| value_var(*v))
                            .unwrap_or_else(|| join_arg_name.clone());
                        Some((join_slot_name, then_val, else_val))
                    })
                    .collect()
            } else {
                vec![]
            };

            // Emit: if cond
            let mut if_op = OpIR {
                kind: "if".to_string(),
                args: Some(vec![value_var(*cond)]),
                ..OpIR::default()
            };
            annotate_cond_type_hint(&mut if_op, *cond, types);
            out.push(if_op);

            // Emit then-block ops inline, preserving the phi-copy filtering
            // that normal block emission applies.
            emit_block_ops(then_blk, &mut out);
            if !matches!(then_blk.terminator, Terminator::Return { .. } | Terminator::Unreachable) {
                for (join_slot_name, then_val, _) in &join_arg_stores {
                    out.push(OpIR {
                        kind: "store_var".to_string(),
                        var: Some(join_slot_name.clone()),
                        args: Some(vec![then_val.clone()]),
                        ..OpIR::default()
                    });
                }
            }
            // Emit then-block terminator if terminal (Return).
            if let Terminator::Return { values } = &then_blk.terminator {
                emit_return_ops(values, original_has_ret, &mut out);
            }

            // Emit: else
            out.push(OpIR { kind: "else".to_string(), ..OpIR::default() });

            // Emit else-block ops inline, preserving the phi-copy filtering
            // that normal block emission applies.
            emit_block_ops(else_blk, &mut out);
            if !matches!(else_blk.terminator, Terminator::Return { .. } | Terminator::Unreachable) {
                for (join_slot_name, _, else_val) in &join_arg_stores {
                    out.push(OpIR {
                        kind: "store_var".to_string(),
                        var: Some(join_slot_name.clone()),
                        args: Some(vec![else_val.clone()]),
                        ..OpIR::default()
                    });
                }
            }
            // Emit else-block terminator if terminal (Return).
            if let Terminator::Return { values } = &else_blk.terminator {
                emit_return_ops(values, original_has_ret, &mut out);
            }

            // Emit: end_if
            out.push(OpIR { kind: "end_if".to_string(), ..OpIR::default() });
        } else {
            // Non-loop, non-if-pattern block: emit ops and terminator normally.
            emit_block_ops(block, &mut out);
            let original_has_ret = func.attrs.get("_original_has_ret")
                .map(|v| matches!(v, super::ops::AttrValue::Bool(true)))
                .unwrap_or(false);
            emit_terminator(
                block,
                &block_param_vars,
                &block_label_id,
                &func.loop_roles,
                &mut out,
                original_has_ret,
                loop_role,
                types,
            );
        }
    }

    VALUE_NAME_OVERRIDES.with(|overrides| overrides.borrow_mut().clear());

    out
}

/// Validate that every label referenced by jump/br_if/check_exception exists
/// as a label op in the output.  Returns false if any reference is dangling.
pub fn validate_labels(ops: &[crate::ir::OpIR]) -> bool {
    let mut defined_labels: HashSet<i64> = HashSet::new();
    let mut referenced_labels: HashSet<i64> = HashSet::new();
    for op in ops {
        match op.kind.as_str() {
            "label" | "state_label" => {
                if let Some(id) = op.value {
                    defined_labels.insert(id);
                }
            }
            "jump" | "br_if" | "check_exception" => {
                if let Some(id) = op.value {
                    referenced_labels.insert(id);
                }
            }
            _ => {}
        }
    }
    referenced_labels.is_subset(&defined_labels)
}

// ---------------------------------------------------------------------------
// Op lowering
// ---------------------------------------------------------------------------

/// Convert a single TirOp to an OpIR. Returns None for ops that are
/// dialect-internal and have no SimpleIR equivalent (yet).
fn lower_op(op: &TirOp) -> Option<OpIR> {
    // Map result (if any) to output variable.
    let out_var = op.results.first().map(|v| value_var(*v));

    match op.opcode {
        // Constants.
        OpCode::ConstInt => Some(OpIR {
            kind: "const".to_string(),
            value: attr_int(&op.attrs, "value"),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::ConstFloat => Some(OpIR {
            kind: "const_float".to_string(),
            f_value: attr_float(&op.attrs, "f_value").or_else(|| attr_float(&op.attrs, "value")),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::ConstStr => Some(OpIR {
            kind: "const_str".to_string(),
            s_value: attr_str(&op.attrs, "s_value").or_else(|| attr_str(&op.attrs, "value")),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::ConstBool => Some(OpIR {
            kind: "const_bool".to_string(),
            // The SSA lift stores the value as AttrValue::Int (from OpIR.value),
            // while SCCP-generated const_bool ops store it as AttrValue::Bool.
            // Handle both representations to avoid silently converting true→false.
            value: Some(match op.attrs.get("value") {
                Some(AttrValue::Bool(b)) => {
                    if *b {
                        1
                    } else {
                        0
                    }
                }
                Some(AttrValue::Int(i)) => {
                    if *i != 0 {
                        1
                    } else {
                        0
                    }
                }
                _ => 0,
            }),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::ConstNone => Some(OpIR {
            kind: "const_none".to_string(),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::ConstBytes => Some(OpIR {
            kind: "const_bytes".to_string(),
            bytes: attr_bytes(&op.attrs, "bytes").or_else(|| attr_bytes(&op.attrs, "value")),
            out: out_var,
            ..OpIR::default()
        }),

        // Arithmetic.
        OpCode::Add => Some(binary_op("add", op, out_var)),
        OpCode::Sub => Some(binary_op("sub", op, out_var)),
        OpCode::Mul => Some(binary_op("mul", op, out_var)),
        OpCode::InplaceAdd => Some(binary_op("inplace_add", op, out_var)),
        OpCode::InplaceSub => Some(binary_op("inplace_sub", op, out_var)),
        OpCode::InplaceMul => Some(binary_op("inplace_mul", op, out_var)),
        OpCode::Div => Some(binary_op("div", op, out_var)),
        OpCode::FloorDiv => Some(binary_op("floor_div", op, out_var)),
        OpCode::Mod => Some(binary_op("mod", op, out_var)),
        OpCode::Pow => Some(binary_op("pow", op, out_var)),
        OpCode::Neg => Some(unary_op("neg", op, out_var)),
        OpCode::Pos => Some(unary_op("pos", op, out_var)),

        // Comparison.
        OpCode::Eq => Some(binary_op("eq", op, out_var)),
        OpCode::Ne => Some(binary_op("ne", op, out_var)),
        OpCode::Lt => Some(binary_op("lt", op, out_var)),
        OpCode::Le => Some(binary_op("le", op, out_var)),
        OpCode::Gt => Some(binary_op("gt", op, out_var)),
        OpCode::Ge => Some(binary_op("ge", op, out_var)),
        OpCode::Is => Some(binary_op("is", op, out_var)),
        OpCode::IsNot => Some(binary_op("is_not", op, out_var)),
        OpCode::In => Some(binary_op("in", op, out_var)),
        OpCode::NotIn => Some(binary_op("not_in", op, out_var)),

        // Bitwise.
        OpCode::BitAnd => Some(binary_op("bit_and", op, out_var)),
        OpCode::BitOr => Some(binary_op("bit_or", op, out_var)),
        OpCode::BitXor => Some(binary_op("bit_xor", op, out_var)),
        OpCode::BitNot => Some(unary_op("bit_not", op, out_var)),
        OpCode::Shl => Some(binary_op("shl", op, out_var)),
        OpCode::Shr => Some(binary_op("shr", op, out_var)),

        // Boolean.
        OpCode::And => Some(binary_op("and", op, out_var)),
        OpCode::Or => Some(binary_op("or", op, out_var)),
        OpCode::Not => Some(unary_op("not", op, out_var)),

        // Memory.
        OpCode::LoadAttr => {
            let kind =
                attr_str(&op.attrs, "_original_kind").unwrap_or_else(|| "get_attr".to_string());
            Some(OpIR {
                kind,
                args: Some(operand_args(op)),
                s_value: attr_str(&op.attrs, "name").or_else(|| attr_str(&op.attrs, "s_value")),
                value: attr_int(&op.attrs, "value"),
                out: out_var,
                ic_index: attr_int(&op.attrs, "ic_index"),
                ..OpIR::default()
            })
        }
        OpCode::StoreAttr => {
            let kind =
                attr_str(&op.attrs, "_original_kind").unwrap_or_else(|| "set_attr".to_string());
            Some(OpIR {
                kind,
                args: Some(operand_args(op)),
                s_value: attr_str(&op.attrs, "name").or_else(|| attr_str(&op.attrs, "s_value")),
                value: attr_int(&op.attrs, "value"),
                out: out_var,
                ..OpIR::default()
            })
        }
        OpCode::Index => {
            let kind = attr_str(&op.attrs, "_original_kind").unwrap_or_else(|| "index".to_string());
            Some(binary_op(&kind, op, out_var))
        }
        OpCode::StoreIndex => {
            let kind =
                attr_str(&op.attrs, "_original_kind").unwrap_or_else(|| "store_index".to_string());
            Some(OpIR {
                kind,
                args: Some(operand_args(op)),
                out: out_var,
                ..OpIR::default()
            })
        }

        // Call — s_value holds the target function name, value holds the code_id.
        // Recover the original SimpleIR kind (call_func, call_indirect, etc.)
        // if it was preserved during the SSA lift.
        OpCode::Call => {
            let kind = attr_str(&op.attrs, "_original_kind").unwrap_or_else(|| "call".to_string());
            Some(OpIR {
                kind,
                s_value: attr_str(&op.attrs, "s_value"),
                args: Some(operand_args(op)),
                out: out_var,
                value: attr_int(&op.attrs, "value"),
                ..OpIR::default()
            })
        }
        OpCode::CallMethod => Some(OpIR {
            kind: "call_method".to_string(),
            args: Some(operand_args(op)),
            s_value: attr_str(&op.attrs, "method"),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::CallBuiltin => {
            let kind =
                attr_str(&op.attrs, "_original_kind").unwrap_or_else(|| "call_builtin".to_string());
            Some(OpIR {
                kind,
                args: Some(operand_args(op)),
                s_value: attr_str(&op.attrs, "name"),
                out: out_var,
                ..OpIR::default()
            })
        }

        // Box/unbox — no-ops at SimpleIR level (type info discarded).
        OpCode::BoxVal | OpCode::UnboxVal | OpCode::TypeGuard => {
            if let (Some(src), Some(dst)) = (op.operands.first(), op.results.first()) {
                Some(OpIR {
                    kind: "copy_var".to_string(),
                    var: Some(value_var(*src)),
                    out: Some(value_var(*dst)),
                    ..OpIR::default()
                })
            } else {
                None
            }
        }

        // Copy: either a genuine copy_var or a passthrough for an unknown op
        // whose original kind was preserved in attrs.
        OpCode::Copy => {
            if let Some(original_kind) = attr_str(&op.attrs, "_original_kind") {
                if original_kind == "unpack_sequence" {
                    let mut args = operand_args(op);
                    args.extend(op.results.iter().map(|v| value_var(*v)));
                    return Some(OpIR {
                        kind: original_kind,
                        args: Some(args),
                        value: attr_int(&op.attrs, "value"),
                        f_value: attr_float(&op.attrs, "f_value"),
                        s_value: attr_str(&op.attrs, "s_value"),
                        bytes: attr_bytes(&op.attrs, "bytes"),
                        var: attr_str(&op.attrs, "_var"),
                        fast_int: attr_bool(&op.attrs, "_fast_int"),
                        fast_float: attr_bool(&op.attrs, "_fast_float"),
                        type_hint: attr_str(&op.attrs, "_type_hint"),
                        task_kind: attr_str(&op.attrs, "task_kind"),
                        container_type: attr_str(&op.attrs, "container_type"),
                        ic_index: attr_int(&op.attrs, "ic_index"),
                        raw_int: attr_bool(&op.attrs, "raw_int"),
                        ..OpIR::default()
                    });
                }
                // Passthrough: reconstruct the original SimpleIR op with all fields.
                Some(OpIR {
                    kind: original_kind,
                    args: if op.operands.is_empty() {
                        None
                    } else {
                        Some(operand_args(op))
                    },
                    out: out_var,
                    value: attr_int(&op.attrs, "value"),
                    f_value: attr_float(&op.attrs, "f_value"),
                    s_value: attr_str(&op.attrs, "s_value"),
                    bytes: attr_bytes(&op.attrs, "bytes"),
                    var: attr_str(&op.attrs, "_var"),
                    fast_int: attr_bool(&op.attrs, "_fast_int"),
                    fast_float: attr_bool(&op.attrs, "_fast_float"),
                    type_hint: attr_str(&op.attrs, "_type_hint"),
                    task_kind: attr_str(&op.attrs, "task_kind"),
                    container_type: attr_str(&op.attrs, "container_type"),
                    ic_index: attr_int(&op.attrs, "ic_index"),
                    raw_int: attr_bool(&op.attrs, "raw_int"),
                    ..OpIR::default()
                })
            } else if let (Some(src), Some(dst)) = (op.operands.first(), op.results.first()) {
                Some(OpIR {
                    kind: "copy_var".to_string(),
                    var: Some(value_var(*src)),
                    out: Some(value_var(*dst)),
                    ..OpIR::default()
                })
            } else {
                None
            }
        }

        // Build containers.
        OpCode::BuildList => Some(OpIR {
            kind: "build_list".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::BuildDict => Some(OpIR {
            kind: "build_dict".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::BuildTuple => Some(OpIR {
            kind: "build_tuple".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::BuildSet => Some(OpIR {
            kind: "build_set".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::BuildSlice => Some(OpIR {
            kind: "build_slice".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),

        // Iteration.
        OpCode::GetIter => Some(unary_op("get_iter", op, out_var)),
        OpCode::IterNext => Some(unary_op("iter_next", op, out_var)),
        OpCode::IterNextUnboxed => {
            // Emit as iter_next_unboxed with two output vars:
            // results[0] = value, results[1] = done_flag.
            let val_var = op.results.first().map(|v| value_var(*v));
            let done_var = op.results.get(1).map(|v| value_var(*v));
            Some(OpIR {
                kind: "iter_next_unboxed".to_string(),
                args: Some(operand_args(op)),
                out: done_var,
                var: val_var,
                ..OpIR::default()
            })
        }
        OpCode::ForIter => Some(OpIR {
            kind: "for_iter".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),

        // Generator.
        OpCode::Yield => Some(OpIR {
            kind: "yield".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::YieldFrom => Some(OpIR {
            kind: "yield_from".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),

        // Exception.
        OpCode::Raise => Some(OpIR {
            kind: "raise".to_string(),
            args: Some(operand_args(op)),
            ..OpIR::default()
        }),
        OpCode::CheckException => Some(OpIR {
            kind: "check_exception".to_string(),
            // Emit with None args (matching the original structured IR format).
            // The Cranelift backend manages live-value state implicitly from
            // the structured control flow context. Emitting the TIR operands
            // (which are all block-argument values captured at exception
            // boundaries) causes the backend to generate incorrect exception
            // handling state with inflated argument lists.
            args: None,
            out: out_var,
            value: attr_int(&op.attrs, "value"),
            ..OpIR::default()
        }),
        OpCode::TryStart => Some(OpIR {
            kind: "try_start".to_string(),
            value: attr_int(&op.attrs, "value"),
            ..OpIR::default()
        }),
        OpCode::TryEnd => Some(OpIR {
            kind: "try_end".to_string(),
            value: attr_int(&op.attrs, "value"),
            ..OpIR::default()
        }),
        OpCode::StateBlockStart => Some(OpIR {
            kind: "state_block_start".to_string(),
            value: attr_int(&op.attrs, "value"),
            ..OpIR::default()
        }),
        OpCode::StateBlockEnd => Some(OpIR {
            kind: "state_block_end".to_string(),
            value: attr_int(&op.attrs, "value"),
            ..OpIR::default()
        }),

        // Import.
        OpCode::Import => Some(OpIR {
            kind: "import".to_string(),
            s_value: attr_str(&op.attrs, "module"),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::ImportFrom => Some(OpIR {
            kind: "import_from".to_string(),
            s_value: attr_str(&op.attrs, "name"),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),

        // Refcount and allocation — preserve for native backend.
        OpCode::IncRef => Some(OpIR {
            kind: "inc_ref".to_string(),
            args: Some(operand_args(op)),
            ..OpIR::default()
        }),
        OpCode::DecRef => Some(OpIR {
            kind: "dec_ref".to_string(),
            args: Some(operand_args(op)),
            ..OpIR::default()
        }),
        OpCode::Alloc => Some(OpIR {
            kind: "alloc".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            value: attr_int(&op.attrs, "value"),
            s_value: attr_str(&op.attrs, "s_value"),
            ..OpIR::default()
        }),
        OpCode::StackAlloc => Some(OpIR {
            kind: "stack_alloc".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            value: attr_int(&op.attrs, "value"),
            ..OpIR::default()
        }),
        OpCode::Free => Some(OpIR {
            kind: "free".to_string(),
            args: Some(operand_args(op)),
            ..OpIR::default()
        }),

        // SCF ops — handled separately via terminators in Phase 2.
        OpCode::ScfIf | OpCode::ScfFor | OpCode::ScfWhile | OpCode::ScfYield => None,

        // Deopt — emit a hint but not critical.
        OpCode::Deopt => Some(OpIR {
            kind: "deopt".to_string(),
            ..OpIR::default()
        }),

        // Remaining attribute ops.
        OpCode::DelAttr => {
            let kind =
                attr_str(&op.attrs, "_original_kind").unwrap_or_else(|| "del_attr".to_string());
            Some(OpIR {
                kind,
                args: Some(operand_args(op)),
                s_value: attr_str(&op.attrs, "name").or_else(|| attr_str(&op.attrs, "s_value")),
                ..OpIR::default()
            })
        }
        OpCode::DelIndex => {
            let kind =
                attr_str(&op.attrs, "_original_kind").unwrap_or_else(|| "del_index".to_string());
            Some(OpIR {
                kind,
                args: Some(operand_args(op)),
                ..OpIR::default()
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Terminator emission
/// Emit return ops for inlined if/else blocks.
fn emit_return_ops(values: &[ValueId], original_has_ret: bool, out: &mut Vec<OpIR>) {
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

fn emit_terminator(
    block: &TirBlock,
    block_param_vars: &HashMap<BlockId, Vec<String>>,
    block_label_id: &dyn Fn(&BlockId) -> i64,
    loop_roles: &HashMap<BlockId, super::blocks::LoopRole>,
    out: &mut Vec<OpIR>,
    original_has_ret: bool,
    loop_role: super::blocks::LoopRole,
    types: &HashMap<ValueId, TirType>,
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
            if loop_role == super::blocks::LoopRole::LoopEnd {
                // Loop back-edge: emit loop_continue + loop_end instead
                // of a plain jump.  The native backend uses these markers
                // to construct the Cranelift loop back-edge.
                emit_block_arg_stores(*target, args, block_param_vars, out);
                out.push(OpIR {
                    kind: "loop_continue".to_string(),
                    ..OpIR::default()
                });
                out.push(OpIR {
                    kind: "loop_end".to_string(),
                    ..OpIR::default()
                });
            } else if loop_roles.get(target).cloned()
                == Some(super::blocks::LoopRole::LoopHeader)
            {
                // Structured loop entry falls through from the preheader into
                // loop_start. The header label exists only for external
                // references such as check_exception targets; the preheader
                // itself must not jump to it or re-enter above loop setup.
                emit_block_arg_stores(*target, args, block_param_vars, out);
            } else {
                // If the block ends with a check_exception op, the native
                // backend handles the fallthrough implicitly — suppress the
                // jump so the next block's ops follow sequentially.  This
                // prevents TIR block boundaries from fragmenting loop bodies
                // with spurious jump/label pairs.
                let last_op_is_check_exception = block.ops.last()
                    .map(|op| op.opcode == OpCode::CheckException)
                    .unwrap_or(false);
                emit_block_arg_stores(*target, args, block_param_vars, out);
                if !last_op_is_check_exception {
                    out.push(OpIR {
                        kind: "jump".to_string(),
                        value: Some(block_label_id(target)),
                        ..OpIR::default()
                    });
                }
            }
        }

        Terminator::CondBranch {
            cond,
            then_block,
            then_args,
            else_block,
            else_args,
        } => {
            if loop_role == super::blocks::LoopRole::LoopHeader {
                // Loop header conditional: the "then" branch exits the
                // loop (break), the "else" branch continues into the body.
                // Store then-args so the after-loop block gets correct values
                // when the break is taken.
                emit_block_arg_stores(*then_block, then_args, block_param_vars, out);
                // Emit loop_break_if_true which the native backend uses
                // to construct the loop exit branch.
                let mut break_op = OpIR {
                    kind: "loop_break_if_true".to_string(),
                    args: Some(vec![value_var(*cond)]),
                    ..OpIR::default()
                };
                annotate_cond_type_hint(&mut break_op, *cond, types);
                out.push(break_op);
                // Fall through to body — store else-args for the body block.
                emit_block_arg_stores(*else_block, else_args, block_param_vars, out);
            } else {
                // Generic conditional branch.
                emit_block_arg_stores(*then_block, then_args, block_param_vars, out);
                let mut br_op = OpIR {
                    kind: "br_if".to_string(),
                    args: Some(vec![value_var(*cond)]),
                    value: Some(block_label_id(then_block)),
                    ..OpIR::default()
                };
                annotate_cond_type_hint(&mut br_op, *cond, types);
                out.push(br_op);
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

        Terminator::Unreachable => {
            out.push(OpIR {
                kind: "unreachable".to_string(),
                ..OpIR::default()
            });
        }
    }
}

/// Emit `store_var` ops to pass `values` to the target block's arg variables.
/// Emit a nested loop's full structure (label, loop_start, condition,
/// body blocks, loop_continue, loop_end, deferred exit) inline within
/// an outer loop's body.  All emitted blocks are recorded in
/// `emitted_inline` so the RPO main loop skips them.
///
/// This is the key fix for the nested-loop-inside-if-inside-while
/// pattern: without inline emission, the RPO main loop places the
/// inner loop header after the function return, creating unreachable
/// loop_start/loop_end pairs and causing infinite loops.
fn emit_nested_loop(
    func: &TirFunction,
    inner_header_bid: &BlockId,
    outer_header_bid: &BlockId,
    header_body_chain: &HashMap<BlockId, Vec<BlockId>>,
    block_param_vars: &HashMap<BlockId, Vec<String>>,
    block_label_id: &impl Fn(&BlockId) -> i64,
    deferred_exits: &HashMap<BlockId, BlockId>,
    loop_break_blocks: &HashMap<BlockId, BlockId>,
    types: &HashMap<ValueId, TirType>,
    original_has_ret: bool,
    emitted_inline: &mut HashSet<BlockId>,
    out: &mut Vec<OpIR>,
) {
    let Some(inner_block) = func.blocks.get(inner_header_bid) else { return };
    emitted_inline.insert(*inner_header_bid);

    let inner_region_chain = header_body_chain
        .get(inner_header_bid)
        .cloned()
        .unwrap_or_default();
    let inner_region_set: HashSet<BlockId> = inner_region_chain.iter().copied().collect();

    // Helper: emit ops for a block
    let emit_ops = |block: &TirBlock, out: &mut Vec<OpIR>| {
        for op in &block.ops {
            if let Some(mut opir) = lower_op(op) {
                annotate_type_flags(&mut opir, op, types);
                out.push(opir);
            }
        }
    };

    // Emit label for the inner loop header.
    out.push(OpIR {
        kind: "label".to_string(),
        value: Some(block_label_id(inner_header_bid)),
        ..OpIR::default()
    });
    // Load block arguments for the inner header.
    if let Some(param_vars) = block_param_vars.get(inner_header_bid) {
        for (i, var_name) in param_vars.iter().enumerate() {
            if i < inner_block.args.len() {
                out.push(OpIR {
                    kind: "load_var".to_string(),
                    var: Some(var_name.clone()),
                    out: Some(value_var(inner_block.args[i].id)),
                    ..OpIR::default()
                });
            }
        }
    }

    out.push(OpIR {
        kind: "loop_start".to_string(),
        ..OpIR::default()
    });

    // Use loop_break_blocks to determine if the break is at the header,
    // its direct successor, or deeper in the body.
    let break_block_bid = loop_break_blocks.get(inner_header_bid).copied();
    let break_is_header = break_block_bid == Some(*inner_header_bid);
    let break_is_header_successor = break_block_bid.map(|bbid| {
        if let Terminator::Branch { target, .. } = &inner_block.terminator {
            bbid == *target
        } else {
            false
        }
    }).unwrap_or(false);

    let mut cond_data: Option<(ValueId, BlockId, Vec<ValueId>, BlockId, Vec<ValueId>)> = None;
    let mut inlined_cond_block: Option<BlockId> = None;
    let mut inner_loop_backedge_target = *inner_header_bid;

    if break_is_header {
        if let Terminator::CondBranch {
            cond, then_block, then_args, else_block, else_args,
        } = &inner_block.terminator {
            emit_ops(inner_block, out);
            cond_data = Some((*cond, *then_block, then_args.clone(), *else_block, else_args.clone()));
        }
    } else if break_is_header_successor {
        if let Terminator::Branch { target, args } = &inner_block.terminator {
            emit_ops(inner_block, out);
            if let Some(cond_block) = func.blocks.get(target)
                && let Terminator::CondBranch {
                    cond, then_block, then_args, else_block, else_args,
                } = &cond_block.terminator
            {
                inner_loop_backedge_target = *target;
                let resolved: Vec<(ValueId, String)> = cond_block
                    .args
                    .iter()
                    .enumerate()
                    .filter_map(|(i, arg)| {
                        args.get(i).map(|&pred| (arg.id, value_var(pred)))
                    })
                    .collect();
                VALUE_NAME_OVERRIDES.with(|overrides| {
                    let mut map = overrides.borrow_mut();
                    for (id, name) in &resolved {
                        map.insert(*id, name.clone());
                    }
                });
                for op in &cond_block.ops {
                    if let Some(mut opir) = lower_op(op) {
                        annotate_type_flags(&mut opir, op, types);
                        out.push(opir);
                    }
                }
                emitted_inline.insert(*target);
                inlined_cond_block = Some(*target);
                cond_data = Some((*cond, *then_block, then_args.clone(), *else_block, else_args.clone()));
            }
        }
    } else {
        // Break is deeper in the body. Emit header ops and terminator normally.
        emit_ops(inner_block, out);
    }

    if let Some((cond, then_block, then_args, else_block, else_args)) = cond_data {
        // Determine body/exit using the validated region set.
        let then_in_region = inner_region_set.contains(&then_block) || then_block == *inner_header_bid;
        let (after_block, after_args, body_block, body_args) = if !then_in_region {
            // then exits, else is body
            (then_block, then_args, else_block, else_args)
        } else {
            // else exits, then is body
            (else_block, else_args, then_block, then_args)
        };

        // Emit loop break condition.
        emit_block_arg_stores(after_block, &after_args, block_param_vars, out);
        let break_kind_str = if !then_in_region {
            "loop_break_if_true"
        } else {
            "loop_break_if_false"
        };
        let mut break_op = OpIR {
            kind: break_kind_str.to_string(),
            args: Some(vec![value_var(cond)]),
            ..OpIR::default()
        };
        annotate_cond_type_hint(&mut break_op, cond, types);
        out.push(break_op);

        // Store args for the first body block entry.
        emit_block_arg_stores(body_block, &body_args, block_param_vars, out);

        // Emit inner loop body blocks.
        let inner_body_blocks: Vec<BlockId> = inner_region_chain
            .iter()
            .copied()
            .filter(|candidate| {
                Some(*candidate) != inlined_cond_block
                    && deferred_exits.get(inner_header_bid).copied() != Some(*candidate)
                    && func
                        .loop_roles
                        .get(candidate)
                        .cloned()
                        .unwrap_or(super::blocks::LoopRole::None)
                        != super::blocks::LoopRole::LoopEnd
            })
            .collect();

        let mut inner_emitted_loop_end = false;

        for inner_region_bid in &inner_body_blocks {
            emitted_inline.insert(*inner_region_bid);
            let nested_nested_role = func.loop_roles.get(inner_region_bid).cloned()
                .unwrap_or(super::blocks::LoopRole::None);
            if nested_nested_role == super::blocks::LoopRole::LoopHeader {
                emit_nested_loop(
                    func,
                    inner_region_bid,
                    inner_header_bid,
                    header_body_chain,
                    block_param_vars,
                    block_label_id,
                    deferred_exits,
                    loop_break_blocks,
                    types,
                    original_has_ret,
                    emitted_inline,
                    out,
                );
                continue;
            }
            let mut in_deeper_nested = false;
            for (hdr, chain) in header_body_chain {
                if *hdr != *inner_header_bid && chain.contains(inner_region_bid) {
                    in_deeper_nested = true;
                    break;
                }
            }
            if in_deeper_nested { continue; }

            if let Some(region_block) = func.blocks.get(inner_region_bid) {
                out.push(OpIR {
                    kind: "label".to_string(),
                    value: Some(block_label_id(inner_region_bid)),
                    ..OpIR::default()
                });
                if let Some(param_vars) = block_param_vars.get(inner_region_bid) {
                    for (i, var_name) in param_vars.iter().enumerate() {
                        if i < region_block.args.len() {
                            out.push(OpIR {
                                kind: "load_var".to_string(),
                                var: Some(var_name.clone()),
                                out: Some(value_var(region_block.args[i].id)),
                                ..OpIR::default()
                            });
                        }
                    }
                }
                emit_ops(region_block, out);
                match &region_block.terminator {
                    Terminator::Branch { target, args } => {
                        let target_role = func
                            .loop_roles
                            .get(target)
                            .cloned()
                            .unwrap_or(super::blocks::LoopRole::None);
                        if target_role == super::blocks::LoopRole::LoopEnd {
                            if let Some(loop_end_block) = func.blocks.get(target)
                                && let Terminator::Branch {
                                    target: continue_target,
                                    args: continue_args,
                                } = &loop_end_block.terminator
                            {
                                emit_block_arg_stores(
                                    *continue_target,
                                    continue_args,
                                    block_param_vars,
                                    out,
                                );
                            }
                            out.push(OpIR { kind: "loop_continue".to_string(), ..OpIR::default() });
                            out.push(OpIR { kind: "loop_end".to_string(), ..OpIR::default() });
                            inner_emitted_loop_end = true;
                        } else if *target == inner_loop_backedge_target {
                            emit_block_arg_stores(*target, args, block_param_vars, out);
                            out.push(OpIR { kind: "loop_continue".to_string(), ..OpIR::default() });
                            out.push(OpIR { kind: "loop_end".to_string(), ..OpIR::default() });
                            inner_emitted_loop_end = true;
                        } else {
                            emit_block_arg_stores(*target, args, block_param_vars, out);
                            let last_op_is_check_exception = region_block.ops.last()
                                .map(|op| op.opcode == OpCode::CheckException)
                                .unwrap_or(false);
                            if !last_op_is_check_exception {
                                out.push(OpIR {
                                    kind: "jump".to_string(),
                                    value: Some(block_label_id(target)),
                                    ..OpIR::default()
                                });
                            }
                        }
                    }
                    Terminator::CondBranch {
                        cond, then_block, then_args, else_block, else_args,
                    } => {
                        emit_block_arg_stores(*then_block, then_args, block_param_vars, out);
                        let mut br_op = OpIR {
                            kind: "br_if".to_string(),
                            args: Some(vec![value_var(*cond)]),
                            value: Some(block_label_id(then_block)),
                            ..OpIR::default()
                        };
                        annotate_cond_type_hint(&mut br_op, *cond, types);
                        out.push(br_op);
                        emit_block_arg_stores(*else_block, else_args, block_param_vars, out);
                        out.push(OpIR {
                            kind: "jump".to_string(),
                            value: Some(block_label_id(else_block)),
                            ..OpIR::default()
                        });
                    }
                    Terminator::Return { values } => {
                        emit_return_ops(values, original_has_ret, out);
                    }
                    Terminator::Unreachable => {
                        out.push(OpIR { kind: "unreachable".to_string(), ..OpIR::default() });
                    }
                    _ => {}
                }
            }
        }

        if !inner_emitted_loop_end {
            if inner_body_blocks.is_empty() {
                if let Some(body_block_ir) = func.blocks.get(&body_block) {
                    emitted_inline.insert(body_block);
                    out.push(OpIR {
                        kind: "label".to_string(),
                        value: Some(block_label_id(&body_block)),
                        ..OpIR::default()
                    });
                    emit_ops(body_block_ir, out);
                    emit_block_arg_stores_for_terminator(
                        func,
                        body_block_ir,
                        inner_loop_backedge_target,
                        block_param_vars,
                        block_label_id,
                        out,
                    );
                } else {
                    out.push(OpIR { kind: "loop_continue".to_string(), ..OpIR::default() });
                    out.push(OpIR { kind: "loop_end".to_string(), ..OpIR::default() });
                }
            } else {
                out.push(OpIR { kind: "loop_continue".to_string(), ..OpIR::default() });
                out.push(OpIR { kind: "loop_end".to_string(), ..OpIR::default() });
            }
        }

        // Emit the inner loop's deferred exit block.
        if let Some(exit_bid) = deferred_exits.get(inner_header_bid) {
            emitted_inline.insert(*exit_bid);
            if let Some(exit_block) = func.blocks.get(exit_bid) {
                out.push(OpIR {
                    kind: "label".to_string(),
                    value: Some(block_label_id(exit_bid)),
                    ..OpIR::default()
                });
                if let Some(param_vars) = block_param_vars.get(exit_bid) {
                    for (i, var_name) in param_vars.iter().enumerate() {
                        if i < exit_block.args.len() {
                            out.push(OpIR {
                                kind: "load_var".to_string(),
                                var: Some(var_name.clone()),
                                out: Some(value_var(exit_block.args[i].id)),
                                ..OpIR::default()
                            });
                        }
                    }
                }
                emit_ops(exit_block, out);
                match &exit_block.terminator {
                    Terminator::Branch { .. } => emit_block_arg_stores_for_terminator(
                        func,
                        exit_block,
                        *outer_header_bid,
                        block_param_vars,
                        block_label_id,
                        out,
                    ),
                    _ => {
                        let exit_role = func.loop_roles.get(exit_bid).cloned()
                            .unwrap_or(super::blocks::LoopRole::None);
                        emit_terminator(
                            exit_block,
                            block_param_vars,
                            block_label_id,
                            &func.loop_roles,
                            out,
                            original_has_ret,
                            exit_role,
                            types,
                        );
                    }
                }
            }
        }
    } else {
        // Break is deeper in the body, or no condition found.
        // Emit header terminator normally, then all body blocks.
        // The break block's CondBranch will be emitted as loop_break_if_*.
        emit_terminator(
            inner_block,
            block_param_vars,
            block_label_id,
            &func.loop_roles,
            out,
            original_has_ret,
            super::blocks::LoopRole::LoopHeader,
            types,
        );

        let inner_body_blocks: Vec<BlockId> = inner_region_chain
            .iter()
            .copied()
            .filter(|candidate| {
                deferred_exits.get(inner_header_bid).copied() != Some(*candidate)
                    && func.loop_roles.get(candidate).cloned()
                        .unwrap_or(super::blocks::LoopRole::None)
                        != super::blocks::LoopRole::LoopEnd
            })
            .collect();

        let mut inner_emitted_loop_end = false;

        for inner_region_bid in &inner_body_blocks {
            emitted_inline.insert(*inner_region_bid);
            let nested_nested_role = func.loop_roles.get(inner_region_bid).cloned()
                .unwrap_or(super::blocks::LoopRole::None);
            if nested_nested_role == super::blocks::LoopRole::LoopHeader {
                emit_nested_loop(
                    func, inner_region_bid, inner_header_bid,
                    header_body_chain, block_param_vars, block_label_id,
                    deferred_exits, loop_break_blocks, types, original_has_ret,
                    emitted_inline, out,
                );
                continue;
            }
            let mut in_deeper_nested = false;
            for (hdr, chain) in header_body_chain {
                if *hdr != *inner_header_bid && chain.contains(inner_region_bid) {
                    in_deeper_nested = true;
                    break;
                }
            }
            if in_deeper_nested { continue; }

            if let Some(region_block) = func.blocks.get(inner_region_bid) {
                out.push(OpIR {
                    kind: "label".to_string(),
                    value: Some(block_label_id(inner_region_bid)),
                    ..OpIR::default()
                });
                if let Some(param_vars) = block_param_vars.get(inner_region_bid) {
                    for (i, var_name) in param_vars.iter().enumerate() {
                        if i < region_block.args.len() {
                            out.push(OpIR {
                                kind: "load_var".to_string(),
                                var: Some(var_name.clone()),
                                out: Some(value_var(region_block.args[i].id)),
                                ..OpIR::default()
                            });
                        }
                    }
                }
                emit_ops(region_block, out);

                // Check if this is the break block.
                if Some(*inner_region_bid) == break_block_bid {
                    if let Terminator::CondBranch { cond, then_block, then_args, else_block, else_args } = &region_block.terminator {
                        let then_in_region = inner_region_set.contains(then_block) || *then_block == *inner_header_bid;
                        let (after_block, after_args, body_target, body_target_args) = if !then_in_region {
                            (*then_block, then_args.clone(), *else_block, else_args.clone())
                        } else {
                            (*else_block, else_args.clone(), *then_block, then_args.clone())
                        };
                        emit_block_arg_stores(after_block, &after_args, block_param_vars, out);
                        let bk_str = if !then_in_region { "loop_break_if_true" } else { "loop_break_if_false" };
                        let mut break_op = OpIR {
                            kind: bk_str.to_string(),
                            args: Some(vec![value_var(*cond)]),
                            ..OpIR::default()
                        };
                        annotate_cond_type_hint(&mut break_op, *cond, types);
                        out.push(break_op);
                        emit_block_arg_stores(body_target, &body_target_args, block_param_vars, out);
                    }
                } else {
                    // Normal body block terminator.
                    match &region_block.terminator {
                        Terminator::Branch { target, args } => {
                            let target_role = func.loop_roles.get(target).cloned()
                                .unwrap_or(super::blocks::LoopRole::None);
                            if target_role == super::blocks::LoopRole::LoopEnd {
                                if let Some(loop_end_block) = func.blocks.get(target)
                                    && let Terminator::Branch {
                                        target: continue_target,
                                        args: continue_args,
                                    } = &loop_end_block.terminator
                                {
                                    emit_block_arg_stores(*continue_target, continue_args, block_param_vars, out);
                                }
                                out.push(OpIR { kind: "loop_continue".to_string(), ..OpIR::default() });
                                out.push(OpIR { kind: "loop_end".to_string(), ..OpIR::default() });
                                inner_emitted_loop_end = true;
                            } else if *target == inner_loop_backedge_target {
                                emit_block_arg_stores(*target, args, block_param_vars, out);
                                out.push(OpIR { kind: "loop_continue".to_string(), ..OpIR::default() });
                                out.push(OpIR { kind: "loop_end".to_string(), ..OpIR::default() });
                                inner_emitted_loop_end = true;
                            } else {
                                emit_block_arg_stores(*target, args, block_param_vars, out);
                                let last_op_is_check_exception = region_block.ops.last()
                                    .map(|op| op.opcode == OpCode::CheckException)
                                    .unwrap_or(false);
                                if !last_op_is_check_exception {
                                    out.push(OpIR {
                                        kind: "jump".to_string(),
                                        value: Some(block_label_id(target)),
                                        ..OpIR::default()
                                    });
                                }
                            }
                        }
                        Terminator::CondBranch { cond, then_block, then_args, else_block, else_args } => {
                            emit_block_arg_stores(*then_block, then_args, block_param_vars, out);
                            let mut br_op = OpIR {
                                kind: "br_if".to_string(),
                                args: Some(vec![value_var(*cond)]),
                                value: Some(block_label_id(then_block)),
                                ..OpIR::default()
                            };
                            annotate_cond_type_hint(&mut br_op, *cond, types);
                            out.push(br_op);
                            emit_block_arg_stores(*else_block, else_args, block_param_vars, out);
                            out.push(OpIR {
                                kind: "jump".to_string(),
                                value: Some(block_label_id(else_block)),
                                ..OpIR::default()
                            });
                        }
                        Terminator::Return { values } => {
                            emit_return_ops(values, original_has_ret, out);
                        }
                        Terminator::Unreachable => {
                            out.push(OpIR { kind: "unreachable".to_string(), ..OpIR::default() });
                        }
                        _ => {}
                    }
                }
            }
        }

        if !inner_emitted_loop_end {
            out.push(OpIR { kind: "loop_continue".to_string(), ..OpIR::default() });
            out.push(OpIR { kind: "loop_end".to_string(), ..OpIR::default() });
        }

        // Emit the inner loop's deferred exit block.
        if let Some(exit_bid) = deferred_exits.get(inner_header_bid) {
            emitted_inline.insert(*exit_bid);
            if let Some(exit_block) = func.blocks.get(exit_bid) {
                out.push(OpIR {
                    kind: "label".to_string(),
                    value: Some(block_label_id(exit_bid)),
                    ..OpIR::default()
                });
                if let Some(param_vars) = block_param_vars.get(exit_bid) {
                    for (i, var_name) in param_vars.iter().enumerate() {
                        if i < exit_block.args.len() {
                            out.push(OpIR {
                                kind: "load_var".to_string(),
                                var: Some(var_name.clone()),
                                out: Some(value_var(exit_block.args[i].id)),
                                ..OpIR::default()
                            });
                        }
                    }
                }
                emit_ops(exit_block, out);
                match &exit_block.terminator {
                    Terminator::Branch { .. } => emit_block_arg_stores_for_terminator(
                        func,
                        exit_block,
                        *outer_header_bid,
                        block_param_vars,
                        block_label_id,
                        out,
                    ),
                    _ => {
                        let exit_role = func.loop_roles.get(exit_bid).cloned()
                            .unwrap_or(super::blocks::LoopRole::None);
                        emit_terminator(
                            exit_block,
                            block_param_vars,
                            block_label_id,
                            &func.loop_roles,
                            out,
                            original_has_ret,
                            exit_role,
                            types,
                        );
                    }
                }
            }
        }
    }
}

/// Helper for emit_nested_loop: emit a block's Branch terminator,
/// converting back-edges to loop_continue + loop_end.
fn emit_block_arg_stores_for_terminator(
    func: &TirFunction,
    block: &TirBlock,
    backedge_target: BlockId,
    block_param_vars: &HashMap<BlockId, Vec<String>>,
    block_label_id: &impl Fn(&BlockId) -> i64,
    out: &mut Vec<OpIR>,
) {
    match &block.terminator {
        Terminator::Branch { target, args } => {
            let target_role = func
                .loop_roles
                .get(target)
                .cloned()
                .unwrap_or(super::blocks::LoopRole::None);
            if target_role == super::blocks::LoopRole::LoopEnd {
                if let Some(loop_end_block) = func.blocks.get(target)
                    && let Terminator::Branch {
                        target: continue_target,
                        args: continue_args,
                    } = &loop_end_block.terminator
                {
                    emit_block_arg_stores(*continue_target, continue_args, block_param_vars, out);
                }
                out.push(OpIR { kind: "loop_continue".to_string(), ..OpIR::default() });
                out.push(OpIR { kind: "loop_end".to_string(), ..OpIR::default() });
            } else if *target == backedge_target {
                emit_block_arg_stores(*target, args, block_param_vars, out);
                out.push(OpIR { kind: "loop_continue".to_string(), ..OpIR::default() });
                out.push(OpIR { kind: "loop_end".to_string(), ..OpIR::default() });
            } else {
                emit_block_arg_stores(*target, args, block_param_vars, out);
                out.push(OpIR {
                    kind: "jump".to_string(),
                    value: Some(block_label_id(target)),
                    ..OpIR::default()
                });
            }
        }
        _ => {
            out.push(OpIR { kind: "loop_continue".to_string(), ..OpIR::default() });
            out.push(OpIR { kind: "loop_end".to_string(), ..OpIR::default() });
        }
    }
}

fn emit_block_arg_stores(
    target: BlockId,
    values: &[ValueId],
    block_param_vars: &HashMap<BlockId, Vec<String>>,
    out: &mut Vec<OpIR>,
) {
    if values.is_empty() {
        return;
    }
    if let Some(param_vars) = block_param_vars.get(&target) {
        for (i, val) in values.iter().enumerate() {
            if let Some(var_name) = param_vars.get(i) {
                out.push(OpIR {
                    kind: "store_var".to_string(),
                    var: Some(var_name.clone()),
                    args: Some(vec![value_var(*val)]),
                    ..OpIR::default()
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// RPO traversal
// ---------------------------------------------------------------------------

fn reverse_postorder(func: &TirFunction) -> Vec<BlockId> {
    let mut visited: HashSet<BlockId> = HashSet::new();
    let mut postorder: Vec<BlockId> = Vec::new();
    let mut stack: Vec<(BlockId, bool)> = vec![(func.entry_block, false)];

    while let Some((bid, processed)) = stack.pop() {
        if processed {
            postorder.push(bid);
            continue;
        }
        if visited.contains(&bid) {
            continue;
        }
        visited.insert(bid);
        stack.push((bid, true));

        if let Some(block) = func.blocks.get(&bid) {
            // Push successors in reverse order for correct DFS.
            let succs = successors_of(block);
            for succ in succs.into_iter().rev() {
                if !visited.contains(&succ) {
                    stack.push((succ, false));
                }
            }
        }
    }

    postorder.reverse();

    // Append any blocks not reachable via normal control flow (e.g. exception
    // handler blocks only reachable via check_exception implicit edges).
    // These must still appear in the output so the native backend can create
    // state_blocks for their labels.
    if visited.len() < func.blocks.len() {
        let mut unreachable: Vec<BlockId> = func
            .blocks
            .keys()
            .filter(|bid| !visited.contains(bid))
            .copied()
            .collect();
        // Sort for deterministic output.
        unreachable.sort_by_key(|bid| bid.0);
        postorder.extend(unreachable);
    }

    postorder
}

fn successors_of(block: &TirBlock) -> Vec<BlockId> {
    match &block.terminator {
        Terminator::Branch { target, .. } => vec![*target],
        Terminator::CondBranch {
            then_block,
            else_block,
            ..
        } => vec![*then_block, *else_block],
        Terminator::Switch { cases, default, .. } => {
            let mut succs = vec![*default];
            for (_, target, _) in cases {
                succs.push(*target);
            }
            succs
        }
        Terminator::Return { .. } | Terminator::Unreachable => vec![],
    }
}

// ---------------------------------------------------------------------------
// Type annotation propagation
// ---------------------------------------------------------------------------

/// Annotate an [`OpIR`] condition-bearing op (`if`, `loop_break_if_*`,
/// `br_if`) with the TIR type of its condition value.  This enables
/// fast-path truthy dispatch (`molt_is_truthy_int`, `molt_is_truthy_bool`)
/// in downstream backends.
fn annotate_cond_type_hint(opir: &mut OpIR, cond: ValueId, types: &HashMap<ValueId, TirType>) {
    match types.get(&cond) {
        Some(TirType::I64) => { opir.type_hint = Some("int".to_string()); }
        Some(TirType::Bool) => { opir.type_hint = Some("bool".to_string()); }
        Some(TirType::F64) => { opir.type_hint = Some("float".to_string()); }
        Some(TirType::Str) => { opir.type_hint = Some("str".to_string()); }
        _ => {}
    }
}

/// Annotate a SimpleIR [`OpIR`] with fast-path flags derived from TIR type
/// refinement results.  This is the critical bridge that makes TIR type
/// analysis visible to downstream backends (Cranelift, WASM, Luau).
fn annotate_type_flags(opir: &mut OpIR, tir_op: &TirOp, types: &HashMap<ValueId, TirType>) {
    // If the op already has type metadata from the original IR (preserved
    // through the passthrough path), respect it — the original frontend
    // annotation is authoritative for ops the type refiner doesn't understand
    // (iter, list_new, etc.).  Only apply type refinement when there's no
    // existing annotation or when the op is a known arithmetic/comparison op.
    let has_original_hint = opir.type_hint.is_some()
        || opir.fast_int.is_some()
        || opir.fast_float.is_some();

    // Only apply TIR type refinement to ops where the refiner's inference is
    // trustworthy.  For passthrough ops (iter, iter_next, list_new, etc.) the
    // refiner may incorrectly infer I64 for results that are actually tuples
    // or lists.  Restrict refinement to known arithmetic/comparison/const ops.
    let is_refinable = matches!(
        tir_op.opcode,
        OpCode::Add | OpCode::Sub | OpCode::Mul | OpCode::Div
        | OpCode::FloorDiv | OpCode::Mod | OpCode::Pow | OpCode::Neg | OpCode::Pos
        | OpCode::InplaceAdd | OpCode::InplaceSub | OpCode::InplaceMul
        | OpCode::Eq | OpCode::Ne | OpCode::Lt | OpCode::Le | OpCode::Gt | OpCode::Ge
        | OpCode::ConstInt | OpCode::ConstFloat | OpCode::ConstBool
        | OpCode::BoxVal | OpCode::UnboxVal
        | OpCode::Not | OpCode::And | OpCode::Or
        | OpCode::BitAnd | OpCode::BitOr | OpCode::BitXor
        | OpCode::Shl | OpCode::Shr
    );

    // Look up the type of the first result value (most ops have 0 or 1 result).
    if !has_original_hint && is_refinable {
        if let Some(&result_id) = tir_op.results.first() {
            match types.get(&result_id) {
                Some(TirType::I64) => {
                    // TIR I64 means the value is known to be an integer, but
                    // at the SimpleIR level values are still NaN-boxed. Use
                    // fast_int (unbox → native op → rebox) not raw_int (which
                    // assumes values are already raw i64 registers).
                    // raw_int is only safe for loop_index_start/next counters
                    // that the backend explicitly manages as raw i64.
                    opir.fast_int = Some(true);
                }
                Some(TirType::F64) => {
                    opir.fast_float = Some(true);
                }
                Some(ty @ TirType::Bool)
                | Some(ty @ TirType::Str)
                | Some(ty @ TirType::Bytes)
                | Some(ty @ TirType::List(_))
                | Some(ty @ TirType::Dict(_, _))
                | Some(ty @ TirType::Set(_))
                | Some(ty @ TirType::Tuple(_))
                | Some(ty @ TirType::BigInt) => {
                    opir.type_hint = Some(type_to_hint_string(ty));
                }
                // DynBox, Never, Union, Box, Func, Ptr — no hint to propagate.
                _ => {}
            }
        }
    }

    // Propagate StackAlloc: if the TIR op is StackAlloc, mark the SimpleIR op
    // so the native backend can emit stack allocation instead of heap allocation.
    if tir_op.opcode == OpCode::StackAlloc {
        opir.stack_eligible = Some(true);
    }

    // Propagate container_type for Index / StoreIndex ops.
    // operands[0] is the container; look up its TIR type.
    if matches!(tir_op.opcode, OpCode::Index | OpCode::StoreIndex) {
        if opir.container_type.is_none() {
            if let Some(container_id) = tir_op.operands.first() {
                match types.get(container_id) {
                    Some(TirType::List(_)) => { opir.container_type = Some("list".to_string()); }
                    Some(TirType::Str) => { opir.container_type = Some("str".to_string()); }
                    Some(TirType::Dict(_, _)) => { opir.container_type = Some("dict".to_string()); }
                    Some(TirType::Tuple(_)) => { opir.container_type = Some("tuple".to_string()); }
                    Some(TirType::Set(_)) => { opir.container_type = Some("set".to_string()); }
                    _ => {}
                }
            }
        }
    }

    // Propagate container_type for len ops (CallBuiltin with _original_kind="len").
    // operands[0] is the container argument.
    if tir_op.opcode == OpCode::CallBuiltin
        && opir.container_type.is_none()
        && opir.kind == "len"
    {
        if let Some(arg_id) = tir_op.operands.first() {
            match types.get(arg_id) {
                Some(TirType::List(_)) => { opir.container_type = Some("list".to_string()); }
                Some(TirType::Str) => { opir.container_type = Some("str".to_string()); }
                Some(TirType::Dict(_, _)) => { opir.container_type = Some("dict".to_string()); }
                Some(TirType::Tuple(_)) => { opir.container_type = Some("tuple".to_string()); }
                Some(TirType::Set(_)) => { opir.container_type = Some("set".to_string()); }
                _ => {}
            }
        }
    }

    // Preserve original fast_int / fast_float / type_hint from the input IR
    // when the type refiner did not produce a more specific type.  This ensures
    // the round-trip is lossless even when type refinement yields DynBox.
    if opir.fast_int.is_none() && attr_bool(&tir_op.attrs, "_fast_int") == Some(true) {
        opir.fast_int = Some(true);
    }
    if opir.fast_float.is_none() && attr_bool(&tir_op.attrs, "_fast_float") == Some(true) {
        opir.fast_float = Some(true);
    }
    if opir.type_hint.is_none()
        && let Some(th) = attr_str(&tir_op.attrs, "_type_hint")
    {
        opir.type_hint = Some(th);
    }
}

/// Convert a TIR type to a human-readable hint string for the backend.
/// Collect all successor BlockIds from a block's terminator.
fn block_successors(block: &TirBlock) -> Vec<BlockId> {
    match &block.terminator {
        Terminator::Branch { target, .. } => vec![*target],
        Terminator::CondBranch { then_block, else_block, .. } => vec![*then_block, *else_block],
        Terminator::Switch { cases, default, .. } => {
            let mut succs: Vec<BlockId> = cases.iter().map(|c| c.1).collect();
            succs.push(*default);
            succs
        }
        _ => vec![],
    }
}

fn type_to_hint_string(ty: &TirType) -> String {
    match ty {
        TirType::I64 => "int".to_string(),
        TirType::F64 => "float".to_string(),
        TirType::Bool => "bool".to_string(),
        TirType::Str => "str".to_string(),
        TirType::Bytes => "bytes".to_string(),
        TirType::None => "none".to_string(),
        TirType::List(_) => "list".to_string(),
        TirType::Dict(_, _) => "dict".to_string(),
        TirType::Set(_) => "set".to_string(),
        TirType::Tuple(_) => "tuple".to_string(),
        TirType::BigInt => "bigint".to_string(),
        TirType::Func(_) => "func".to_string(),
        TirType::Ptr(_) => "ptr".to_string(),
        TirType::Box(inner) => format!("box<{}>", type_to_hint_string(inner)),
        TirType::Union(members) => {
            let parts: Vec<String> = members.iter().map(type_to_hint_string).collect();
            format!("union<{}>", parts.join(","))
        }
        TirType::DynBox => "any".to_string(),
        TirType::Never => "never".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Helper utilities
// ---------------------------------------------------------------------------

/// Synthesise a SimpleIR variable name from a ValueId.
fn value_var(id: ValueId) -> String {
    VALUE_NAME_OVERRIDES.with(|overrides| {
        overrides
            .borrow()
            .get(&id)
            .cloned()
            .unwrap_or_else(|| format!("_v{}", id.0))
    })
}

fn binary_op(kind: &str, op: &TirOp, out: Option<String>) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        args: Some(operand_args(op)),
        out,
        ..OpIR::default()
    }
}

fn unary_op(kind: &str, op: &TirOp, out: Option<String>) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        args: Some(operand_args(op)),
        out,
        ..OpIR::default()
    }
}

fn operand_args(op: &TirOp) -> Vec<String> {
    op.operands.iter().map(|v| value_var(*v)).collect()
}

fn attr_int(attrs: &super::ops::AttrDict, key: &str) -> Option<i64> {
    match attrs.get(key) {
        Some(AttrValue::Int(i)) => Some(*i),
        _ => None,
    }
}

fn attr_float(attrs: &super::ops::AttrDict, key: &str) -> Option<f64> {
    match attrs.get(key) {
        Some(AttrValue::Float(f)) => Some(*f),
        _ => None,
    }
}

fn attr_str(attrs: &super::ops::AttrDict, key: &str) -> Option<String> {
    match attrs.get(key) {
        Some(AttrValue::Str(s)) => Some(s.clone()),
        _ => None,
    }
}

fn attr_bool(attrs: &super::ops::AttrDict, key: &str) -> Option<bool> {
    match attrs.get(key) {
        Some(AttrValue::Bool(b)) => Some(*b),
        _ => None,
    }
}

fn attr_bytes(attrs: &super::ops::AttrDict, key: &str) -> Option<Vec<u8>> {
    match attrs.get(key) {
        Some(AttrValue::Bytes(b)) => Some(b.clone()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::{Terminator, TirBlock};
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;

    fn add_function() -> TirFunction {
        let mut func =
            TirFunction::new("add".into(), vec![TirType::I64, TirType::I64], TirType::I64);

        let result = ValueId(func.next_value);
        func.next_value += 1;

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Add,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![result],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        func
    }

    #[test]
    fn linearize_simple_function_compiles() {
        let func = add_function();
        let ops = lower_to_simple_ir(&func, &HashMap::new());
        // Must produce at least one op.
        assert!(!ops.is_empty(), "expected non-empty ops for add function");
    }

    #[test]
    fn linearize_emits_return() {
        let func = add_function();
        let ops = lower_to_simple_ir(&func, &HashMap::new());
        let has_ret = ops.iter().any(|o| o.kind == "ret" || o.kind == "ret_void");
        assert!(has_ret, "expected a return op, got: {:?}", ops);
    }

    #[test]
    fn empty_tir_return_preserves_original_ret_signature() {
        let mut func = TirFunction::new("ret_none".into(), vec![], TirType::DynBox);
        func.attrs
            .insert("_original_has_ret".into(), AttrValue::Bool(true));
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.terminator = Terminator::Return { values: vec![] };

        let ops = lower_to_simple_ir(&func, &HashMap::new());

        assert!(
            !ops.iter().any(|op| op.kind == "ret_void"),
            "roundtrip must not downgrade original `ret` to `ret_void`: {ops:?}"
        );
        let ret_op = ops
            .iter()
            .find(|op| op.kind == "ret")
            .expect("roundtrip must synthesize `ret None`");
        let none_op = ops
            .iter()
            .find(|op| op.kind == "const_none")
            .expect("roundtrip must synthesize a const_none return value");
        let none_name = none_op
            .out
            .as_deref()
            .expect("const_none must define an output var");
        assert_eq!(
            ret_op.var.as_deref(),
            Some(none_name),
            "ret must use the synthesized None value"
        );
        assert_eq!(
            ret_op.args.as_ref().and_then(|args| args.first()).map(String::as_str),
            Some(none_name),
            "ret args must also reference the synthesized None value"
        );
    }

    #[test]
    fn ret_op_has_var_set() {
        let func = add_function();
        let ops = lower_to_simple_ir(&func, &HashMap::new());
        let ret_op = ops
            .iter()
            .find(|o| o.kind == "ret")
            .expect("expected a ret op");
        assert!(
            ret_op.var.is_some(),
            "ret op must have `var` set for the native backend; got: {:?}",
            ret_op
        );
    }

    /// Integration test: full TIR round-trip preserves `ret` var field.
    /// This simulates the frontend's `def add(a,b): return a+b` IR.
    #[test]
    fn tir_round_trip_preserves_ret_var() {
        use crate::ir::{FunctionIR, OpIR};
        use crate::tir::lower_from_simple::lower_to_tir;
        use crate::tir::type_refine;

        let func_ir = FunctionIR {
            name: "add".into(),
            params: vec!["a".into(), "b".into()],
            ops: vec![
                OpIR {
                    kind: "add".into(),
                    args: Some(vec!["a".into(), "b".into()]),
                    out: Some("v0".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".into(),
                    var: Some("v0".into()),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
        };

        let mut tir_func = lower_to_tir(&func_ir);
        type_refine::refine_types(&mut tir_func);
        let type_map = type_refine::extract_type_map(&tir_func);
        let round_tripped = lower_to_simple_ir(&tir_func, &type_map);

        let ret_op = round_tripped
            .iter()
            .find(|o| o.kind == "ret")
            .expect("TIR round-trip must preserve the ret op");
        assert!(
            ret_op.var.is_some(),
            "TIR round-trip must set `var` on ret op for native backend; got: {:?}",
            ret_op,
        );
    }

    #[test]
    fn linearize_emits_add_op() {
        let func = add_function();
        let ops = lower_to_simple_ir(&func, &HashMap::new());
        let has_add = ops.iter().any(|o| o.kind == "add");
        assert!(has_add, "expected an 'add' op, got: {:?}", ops);
    }

    #[test]
    fn linearize_multi_block_emits_labels() {
        // Build: func @branch(bool) -> i64 with two successor blocks.
        let mut func = TirFunction::new("branch".into(), vec![TirType::Bool], TirType::I64);

        let bb1 = func.fresh_block();
        let bb2 = func.fresh_block();
        let v1 = func.fresh_value();
        let v2 = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.terminator = Terminator::CondBranch {
            cond: ValueId(0),
            then_block: bb1,
            then_args: vec![],
            else_block: bb2,
            else_args: vec![],
        };

        let mut attrs1 = AttrDict::new();
        attrs1.insert("value".into(), AttrValue::Int(1));
        func.blocks.insert(
            bb1,
            TirBlock {
                id: bb1,
                args: vec![],
                ops: vec![TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstInt,
                    operands: vec![],
                    results: vec![v1],
                    attrs: attrs1,
                    source_span: None,
                }],
                terminator: Terminator::Return { values: vec![v1] },
            },
        );

        let mut attrs2 = AttrDict::new();
        attrs2.insert("value".into(), AttrValue::Int(0));
        func.blocks.insert(
            bb2,
            TirBlock {
                id: bb2,
                args: vec![],
                ops: vec![TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstInt,
                    operands: vec![],
                    results: vec![v2],
                    attrs: attrs2,
                    source_span: None,
                }],
                terminator: Terminator::Return { values: vec![v2] },
            },
        );

        let ops = lower_to_simple_ir(&func, &HashMap::new());
        let kinds: Vec<&str> = ops.iter().map(|o| o.kind.as_str()).collect();
        // Simple CondBranch with both successors returning is now emitted
        // as structured if/else/end_if instead of labels + jumps.
        assert!(
            kinds.contains(&"if"),
            "expected structured 'if' op for simple CondBranch, got: {:?}",
            kinds
        );
        assert!(
            kinds.contains(&"else"),
            "expected structured 'else' op for simple CondBranch, got: {:?}",
            kinds
        );
        assert!(
            kinds.contains(&"end_if"),
            "expected structured 'end_if' op for simple CondBranch, got: {:?}",
            kinds
        );
        // Both branches should have const + ret.
        let ret_count = kinds.iter().filter(|k| **k == "ret").count();
        assert!(
            ret_count >= 2,
            "expected >=2 ret ops (one per branch), got {}: {:?}",
            ret_count,
            kinds
        );
    }

    #[test]
    fn value_var_naming() {
        assert_eq!(value_var(ValueId(0)), "_v0");
        assert_eq!(value_var(ValueId(42)), "_v42");
    }

    /// Verify that TIR type refinement results are propagated back to SimpleIR
    /// fast-path flags.  This is the critical test for the type-propagation fix:
    /// ops that TIR proves are I64 must have `fast_int = Some(true)` in the
    /// output SimpleIR.
    #[test]
    fn type_propagation_sets_fast_int_on_arithmetic() {
        use crate::tir::type_refine::{extract_type_map, refine_types};

        // Build: func @add_ints() -> I64
        //   %0 = const_int 10
        //   %1 = const_int 20
        //   %2 = add %0, %1
        //   return %2
        let mut func = TirFunction::new("add_ints".into(), vec![], TirType::I64);

        let v0 = ValueId(func.next_value);
        func.next_value += 1;
        let v1 = ValueId(func.next_value);
        func.next_value += 1;
        let v2 = ValueId(func.next_value);
        func.next_value += 1;

        let mut attrs0 = AttrDict::new();
        attrs0.insert("value".into(), AttrValue::Int(10));
        let mut attrs1 = AttrDict::new();
        attrs1.insert("value".into(), AttrValue::Int(20));

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![v0],
            attrs: attrs0,
            source_span: None,
        });
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![v1],
            attrs: attrs1,
            source_span: None,
        });
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Add,
            operands: vec![v0, v1],
            results: vec![v2],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return { values: vec![v2] };

        // Run type refinement.
        refine_types(&mut func);
        let type_map = extract_type_map(&func);

        // Verify the type map has I64 for all three values.
        assert_eq!(type_map.get(&v0), Some(&TirType::I64), "v0 should be I64");
        assert_eq!(type_map.get(&v1), Some(&TirType::I64), "v1 should be I64");
        assert_eq!(
            type_map.get(&v2),
            Some(&TirType::I64),
            "v2 should be I64 (add of two I64s)"
        );

        // Lower to SimpleIR with the type map.
        let ops = lower_to_simple_ir(&func, &type_map);

        // The 'add' op in the output must have fast_int = Some(true).
        let add_ops: Vec<&OpIR> = ops.iter().filter(|o| o.kind == "add").collect();
        assert!(!add_ops.is_empty(), "expected an 'add' op in output");
        for add_op in &add_ops {
            assert_eq!(
                add_op.fast_int,
                Some(true),
                "add op should have fast_int=true after type propagation, got: {:?}",
                add_op
            );
        }

        // The const ops must also have fast_int = Some(true).
        let const_ops: Vec<&OpIR> = ops.iter().filter(|o| o.kind == "const").collect();
        assert!(const_ops.len() >= 2, "expected at least 2 const ops");
        for const_op in &const_ops {
            assert_eq!(
                const_op.fast_int,
                Some(true),
                "const int op should have fast_int=true, got: {:?}",
                const_op
            );
        }
    }

    /// Verify that F64 types produce fast_float flags.
    #[test]
    fn type_propagation_sets_fast_float_on_float_arithmetic() {
        use crate::tir::type_refine::{extract_type_map, refine_types};

        let mut func = TirFunction::new("add_floats".into(), vec![], TirType::F64);

        let v0 = ValueId(func.next_value);
        func.next_value += 1;
        let v1 = ValueId(func.next_value);
        func.next_value += 1;
        let v2 = ValueId(func.next_value);
        func.next_value += 1;

        let mut attrs0 = AttrDict::new();
        attrs0.insert("f_value".into(), AttrValue::Float(1.5));
        let mut attrs1 = AttrDict::new();
        attrs1.insert("f_value".into(), AttrValue::Float(2.5));

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstFloat,
            operands: vec![],
            results: vec![v0],
            attrs: attrs0,
            source_span: None,
        });
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstFloat,
            operands: vec![],
            results: vec![v1],
            attrs: attrs1,
            source_span: None,
        });
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Add,
            operands: vec![v0, v1],
            results: vec![v2],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return { values: vec![v2] };

        refine_types(&mut func);
        let type_map = extract_type_map(&func);
        let ops = lower_to_simple_ir(&func, &type_map);

        let add_ops: Vec<&OpIR> = ops.iter().filter(|o| o.kind == "add").collect();
        assert!(!add_ops.is_empty());
        for add_op in &add_ops {
            assert_eq!(
                add_op.fast_float,
                Some(true),
                "float add should have fast_float=true, got: {:?}",
                add_op
            );
        }
    }

    /// Verify that comparison ops get type_hint="bool" (not fast_int/fast_float).
    #[test]
    fn type_propagation_sets_type_hint_for_bool() {
        use crate::tir::type_refine::{extract_type_map, refine_types};

        let mut func = TirFunction::new("cmp".into(), vec![], TirType::Bool);

        let v0 = ValueId(func.next_value);
        func.next_value += 1;
        let v1 = ValueId(func.next_value);
        func.next_value += 1;
        let v2 = ValueId(func.next_value);
        func.next_value += 1;

        let mut attrs0 = AttrDict::new();
        attrs0.insert("value".into(), AttrValue::Int(1));
        let mut attrs1 = AttrDict::new();
        attrs1.insert("value".into(), AttrValue::Int(2));

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![v0],
            attrs: attrs0,
            source_span: None,
        });
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![v1],
            attrs: attrs1,
            source_span: None,
        });
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Eq,
            operands: vec![v0, v1],
            results: vec![v2],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return { values: vec![v2] };

        refine_types(&mut func);
        let type_map = extract_type_map(&func);
        let ops = lower_to_simple_ir(&func, &type_map);

        let eq_ops: Vec<&OpIR> = ops.iter().filter(|o| o.kind == "eq").collect();
        assert!(!eq_ops.is_empty());
        for eq_op in &eq_ops {
            assert_eq!(
                eq_op.type_hint.as_deref(),
                Some("bool"),
                "eq op should have type_hint='bool', got: {:?}",
                eq_op
            );
            // Bool should NOT set fast_int or fast_float.
            assert!(eq_op.fast_int.is_none(), "bool op should not have fast_int");
            assert!(
                eq_op.fast_float.is_none(),
                "bool op should not have fast_float"
            );
        }
    }

    /// Verify that no type map (empty) means no flags are set.
    #[test]
    fn empty_type_map_sets_no_flags() {
        let func = add_function();
        let ops = lower_to_simple_ir(&func, &HashMap::new());
        let add_ops: Vec<&OpIR> = ops.iter().filter(|o| o.kind == "add").collect();
        assert!(!add_ops.is_empty());
        for add_op in &add_ops {
            assert!(
                add_op.fast_int.is_none(),
                "empty type map should not set fast_int"
            );
            assert!(
                add_op.fast_float.is_none(),
                "empty type map should not set fast_float"
            );
            assert!(
                add_op.type_hint.is_none(),
                "empty type map should not set type_hint"
            );
        }
    }

    #[test]
    fn tir_round_trip_preserves_guarded_field_set_offset() {
        use crate::ir::{FunctionIR, OpIR};
        use crate::tir::lower_from_simple::lower_to_tir;

        let func_ir = FunctionIR {
            name: "guarded_store".into(),
            params: vec![
                "obj".into(),
                "class_bits".into(),
                "expected".into(),
                "value".into(),
            ],
            ops: vec![OpIR {
                kind: "guarded_field_set".into(),
                args: Some(vec![
                    "obj".into(),
                    "class_bits".into(),
                    "expected".into(),
                    "value".into(),
                ]),
                s_value: Some("x".into()),
                value: Some(24),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
        };

        let tir_func = lower_to_tir(&func_ir);
        let round_tripped = lower_to_simple_ir(&tir_func, &HashMap::new());
        let store_op = round_tripped
            .iter()
            .find(|op| op.kind == "guarded_field_set")
            .expect("expected guarded_field_set after TIR round-trip");

        assert_eq!(store_op.s_value.as_deref(), Some("x"));
        assert_eq!(store_op.value, Some(24));
    }

    #[test]
    fn tir_round_trip_preserves_guarded_field_get_offset() {
        use crate::ir::{FunctionIR, OpIR};
        use crate::tir::lower_from_simple::lower_to_tir;

        let func_ir = FunctionIR {
            name: "guarded_load".into(),
            params: vec!["obj".into(), "class_bits".into(), "expected".into()],
            ops: vec![OpIR {
                kind: "guarded_field_get".into(),
                args: Some(vec!["obj".into(), "class_bits".into(), "expected".into()]),
                s_value: Some("x".into()),
                value: Some(24),
                out: Some("loaded".into()),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
        };

        let tir_func = lower_to_tir(&func_ir);
        let round_tripped = lower_to_simple_ir(&tir_func, &HashMap::new());
        let load_op = round_tripped
            .iter()
            .find(|op| op.kind == "guarded_field_get")
            .expect("expected guarded_field_get after TIR round-trip");

        assert_eq!(load_op.s_value.as_deref(), Some("x"));
        assert_eq!(load_op.value, Some(24));
        assert!(
            load_op.out.is_some(),
            "guarded_field_get must preserve an output"
        );
    }

    /// Integration test: TIR round-trip preserves loop markers (loop_start, loop_end).
    /// Simulates a `while i < 3: total += i; i += 1` pattern.
    #[test]
    fn tir_round_trip_preserves_loop_markers() {
        use crate::ir::{FunctionIR, OpIR};
        use crate::tir::lower_from_simple::lower_to_tir;

        let func_ir = FunctionIR {
            name: "while_loop".into(),
            params: vec!["n".into()],
            ops: vec![
                OpIR {
                    kind: "const".into(),
                    value: Some(0),
                    out: Some("total".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".into(),
                    value: Some(0),
                    out: Some("i".into()),
                    ..OpIR::default()
                },
                // loop_start: header
                OpIR {
                    kind: "loop_start".into(),
                    ..OpIR::default()
                },
                // condition: i < n
                OpIR {
                    kind: "lt".into(),
                    args: Some(vec!["i".into(), "n".into()]),
                    out: Some("cond".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_break_if_false".into(),
                    args: Some(vec!["cond".into()]),
                    ..OpIR::default()
                },
                // body: total += i
                OpIR {
                    kind: "add".into(),
                    args: Some(vec!["total".into(), "i".into()]),
                    out: Some("total".into()),
                    ..OpIR::default()
                },
                // body: i += 1
                OpIR {
                    kind: "const".into(),
                    value: Some(1),
                    out: Some("one".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "add".into(),
                    args: Some(vec!["i".into(), "one".into()]),
                    out: Some("i".into()),
                    ..OpIR::default()
                },
                // loop_end: back-edge
                OpIR {
                    kind: "loop_end".into(),
                    ..OpIR::default()
                },
                // after loop
                OpIR {
                    kind: "ret".into(),
                    var: Some("total".into()),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
        };

        let tir_func = lower_to_tir(&func_ir);

        // Verify loop roles were detected.
        let has_header = tir_func
            .loop_roles
            .values()
            .any(|r| *r == super::super::blocks::LoopRole::LoopHeader);
        let has_end = tir_func
            .loop_roles
            .values()
            .any(|r| *r == super::super::blocks::LoopRole::LoopEnd);
        assert!(
            has_header,
            "expected a LoopHeader role; loop_roles = {:?}",
            tir_func.loop_roles
        );
        assert!(
            has_end,
            "expected a LoopEnd role; loop_roles = {:?}",
            tir_func.loop_roles
        );

        let type_map = HashMap::new();
        let round_tripped = lower_to_simple_ir(&tir_func, &type_map);

        // Must contain a structured loop exit op, not a state-machine branch.
        let has_loop_break = round_tripped
            .iter()
            .any(|o| o.kind == "loop_break_if_false");
        assert!(
            has_loop_break,
            "round-tripped while loop must contain loop_break_if_false for the loop condition; ops: {:?}",
            round_tripped
                .iter()
                .map(|o| o.kind.as_str())
                .collect::<Vec<_>>()
        );

        // Structured loop round-trips must use loop_continue/loop_end for the
        // back-edge rather than a state-machine jump to the header label.
        let has_loop_continue = round_tripped.iter().any(|o| o.kind == "loop_continue");
        assert!(
            has_loop_continue,
            "round-tripped while loop must contain loop_continue"
        );
        let header_label = round_tripped
            .iter()
            .find(|o| o.kind == "label")
            .and_then(|o| o.value);
        let has_back_edge_jump = header_label.is_some_and(|label| {
            round_tripped
                .iter()
                .any(|o| o.kind == "jump" && o.value == Some(label))
        });
        assert!(
            !has_back_edge_jump,
            "round-tripped while loop must not lower the back-edge as jump-to-header"
        );

        // Must still have a ret op.
        let has_ret = round_tripped.iter().any(|o| o.kind == "ret");
        assert!(
            has_ret,
            "round-tripped ops must contain ret; ops: {:?}",
            round_tripped
                .iter()
                .map(|o| o.kind.as_str())
                .collect::<Vec<_>>()
        );

        // Label validation must pass.
        assert!(
            validate_labels(&round_tripped),
            "label validation failed on round-tripped while loop: {round_tripped:#?}"
        );
    }

    /// Regression test: a pre-break guard inside the loop body must not be
    /// mistaken for the loop's structured exit condition.
    #[test]
    fn tir_round_trip_keeps_guard_branch_out_of_loop_break_selection() {
        use crate::ir::{FunctionIR, OpIR};
        use crate::tir::lower_from_simple::lower_to_tir;

        let func_ir = FunctionIR {
            name: "guard_before_break".into(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const_bool".into(),
                    value: Some(0),
                    out: Some("retv".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_bool".into(),
                    value: Some(1),
                    out: Some("guard".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_bool".into(),
                    value: Some(0),
                    out: Some("break_cond".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_start".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "if".into(),
                    args: Some(vec!["guard".into()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".into(),
                    var: Some("retv".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "end_if".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_break_if_true".into(),
                    args: Some(vec!["break_cond".into()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_continue".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_end".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".into(),
                    var: Some("retv".into()),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
        };

        let tir_func = lower_to_tir(&func_ir);
        let round_tripped = lower_to_simple_ir(&tir_func, &HashMap::new());

        let has_real_break = round_tripped.iter().any(|op| {
            matches!(op.kind.as_str(), "loop_break_if_true" | "loop_break_if_false")
                && op.args
                    .as_ref()
                    .is_some_and(|args| args == &vec!["_v3".to_string()])
        });
        assert!(
            has_real_break,
            "round-tripped loop must preserve the real loop_break_if_true operand; ops: {round_tripped:#?}"
        );

        let rewrote_guard_into_break = round_tripped.iter().any(|op| {
            matches!(op.kind.as_str(), "loop_break_if_true" | "loop_break_if_false")
                && op.args
                    .as_ref()
                    .is_some_and(|args| args == &vec!["_v2".to_string()])
        });
        assert!(
            !rewrote_guard_into_break,
            "pre-break guard must not be rewritten as the loop exit test; ops: {round_tripped:#?}"
        );

        assert!(
            validate_labels(&round_tripped),
            "label validation failed on guard-before-break loop: {round_tripped:#?}"
        );
    }

    /// Regression test: counted loops must not re-enter above loop_index_start.
    /// Otherwise the induction variable resets every iteration and the loop hangs.
    #[test]
    fn tir_round_trip_keeps_loop_index_start_out_of_backedge_path() {
        use crate::ir::{FunctionIR, OpIR};
        use crate::tir::lower_from_simple::lower_to_tir;

        let func_ir = FunctionIR {
            name: "counted_loop".into(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const".into(),
                    value: Some(3),
                    out: Some("limit".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".into(),
                    value: Some(0),
                    out: Some("zero".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".into(),
                    value: Some(1),
                    out: Some("one".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_start".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_index_start".into(),
                    args: Some(vec!["zero".into()]),
                    out: Some("i".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "lt".into(),
                    args: Some(vec!["i".into(), "limit".into()]),
                    out: Some("cond".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_break_if_false".into(),
                    args: Some(vec!["cond".into()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "add".into(),
                    args: Some(vec!["i".into(), "one".into()]),
                    out: Some("next_i".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_index_next".into(),
                    args: Some(vec!["next_i".into()]),
                    out: Some("i".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_continue".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_end".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".into(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
        };

        let tir_func = lower_to_tir(&func_ir);
        let round_tripped = lower_to_simple_ir(&tir_func, &HashMap::new());

        let loop_start_idx = round_tripped
            .iter()
            .position(|op| op.kind == "loop_start")
            .expect("expected loop_start after round-trip");
        let loop_index_start_idx = round_tripped
            .iter()
            .position(|op| op.kind == "loop_index_start")
            .expect("expected loop_index_start after round-trip");
        assert!(
            round_tripped[loop_start_idx + 1..loop_index_start_idx]
                .iter()
                .all(|op| op.kind != "label" && op.kind != "jump" && op.kind != "br_if"),
            "counted loop must not place control-flow re-entry before loop_index_start; ops: {:?}",
            round_tripped
                .iter()
                .map(|op| op.kind.as_str())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn tir_if_pattern_join_args_emit_store_var_for_join_block() {
        use crate::ir::{FunctionIR, OpIR};
        use crate::tir::lower_from_simple::lower_to_tir;

        let func_ir = FunctionIR {
            name: "if_join_args".into(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const_bool".into(),
                    value: Some(0),
                    out: Some("cond".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "if".into(),
                    args: Some(vec!["cond".into()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".into(),
                    value: Some(11),
                    out: Some("then_val".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "else".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".into(),
                    value: Some(22),
                    out: Some("else_val".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "end_if".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "phi".into(),
                    out: Some("merged".into()),
                    args: Some(vec!["then_val".into(), "else_val".into()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".into(),
                    var: Some("merged".into()),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
        };

        let mut rewritten_ops = func_ir.ops.clone();
        crate::rewrite_phi_to_store_load(&mut rewritten_ops);
        let rewritten = FunctionIR {
            name: func_ir.name,
            params: func_ir.params,
            ops: rewritten_ops,
            param_types: func_ir.param_types,
            source_file: func_ir.source_file,
        };

        let tir_func = lower_to_tir(&rewritten);
        let round_tripped = lower_to_simple_ir(&tir_func, &HashMap::new());

        assert!(
            round_tripped.iter().any(|op| op.kind == "store_var"),
            "if-pattern join should store block-arg values; ops: {:?}",
            round_tripped
                .iter()
                .map(|op| op.kind.as_str())
                .collect::<Vec<_>>()
        );
        assert!(
            round_tripped.iter().any(|op| op.kind == "load_var"),
            "join block should load merged value; ops: {:?}",
            round_tripped
                .iter()
                .map(|op| op.kind.as_str())
                .collect::<Vec<_>>()
        );
        assert!(
            round_tripped.iter().all(|op| op.kind != "phi"),
            "round-tripped if-pattern should not reintroduce phi: {:?}",
            round_tripped
                .iter()
                .map(|op| (op.kind.as_str(), op.out.as_deref(), op.var.as_deref(), op.args.clone()))
                .collect::<Vec<_>>()
        );
    }
}
