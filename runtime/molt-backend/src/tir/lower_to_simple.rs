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
//! Full round-trip with phi elimination and all OpCode mappings.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use crate::ir::OpIR;

use super::blocks::{BlockId, LoopBreakKind, Terminator, TirBlock};
use super::function::TirFunction;
use super::ops::{AttrValue, OpCode, TirOp};
use super::types::TirType;
use super::values::ValueId;

/// A detected natural-loop region, keyed by the loop header block.
/// Used by structured loop emission to re-wrap linearised TIR control
/// flow into loop_start/loop_break_if_X/loop_continue/loop_end sequences.
struct LoopRegion {
    /// Guard blocks with CondBranch terminators (type checks, bounds
    /// checks) in the header chain.  These are emitted inline in the
    /// header region (before break) with br_if + raise-path handling.
    guard_chain: Vec<BlockId>,
    /// Raise-path blocks reachable from guard CondBranches.
    /// Consumed so they are not double-emitted in the main loop.
    guard_raise_blocks: Vec<BlockId>,
    /// The block whose CondBranch controls the loop (body vs exit).
    cond_block: BlockId,
    body_entry: BlockId,
    exit_block: BlockId,
    body_set: HashSet<BlockId>,
    break_kind: LoopBreakKind,
    cond: ValueId,
    body_args: Vec<ValueId>,
    exit_args: Vec<ValueId>,
}

thread_local! {
    static VALUE_NAME_OVERRIDES: RefCell<HashMap<ValueId, String>> = RefCell::new(HashMap::new());
}

fn collect_guard_raise_path_blocks(func: &TirFunction, start: BlockId) -> Vec<BlockId> {
    let mut raise_blocks = Vec::new();
    let mut cur = start;
    let mut visited: HashSet<BlockId> = HashSet::new();
    for _ in 0..3 {
        if !visited.insert(cur) {
            break;
        }
        raise_blocks.push(cur);
        let Some(blk) = func.blocks.get(&cur) else {
            break;
        };
        if let Terminator::Branch { target, .. } = &blk.terminator {
            cur = *target;
        } else {
            break;
        }
    }
    raise_blocks
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
/// When a `types` map is provided, the back-conversion may use it for
/// structural emission decisions, but it does not mint backend-authoritative
/// optimization hints on the SimpleIR transport.
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
    let debug_lower_func = std::env::var("MOLT_DEBUG_LOWER_FUNC").ok();
    let debug_loop_if_return = func.name == "loop_if_return_continue_roundtrip"
        || debug_lower_func.as_deref() == Some(func.name.as_str());
    if debug_loop_if_return {
        eprintln!("LOWER_DEBUG_RPO: {:?}", rpo);
    }

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
    if debug_loop_if_return {
        eprintln!("LOWER_DEBUG_LABEL_MAP: {:?}", label_id_for_block);
    }

    // Build a mapping from ORIGINAL label IDs to NEW label IDs.
    // check_exception, try_start, and try_end ops carry original label IDs
    // in their value attrs.  After TIR roundtrip, blocks may have different
    // label IDs.  This map translates original → new so the ops reference
    // the correct post-roundtrip labels.
    let original_to_new_label: HashMap<i64, i64> = {
        let mut map = HashMap::new();
        for (&bid_u32, &original_id) in &func.label_id_map {
            let block_id = BlockId(bid_u32);
            if let Some(&new_id) = label_id_for_block.get(&block_id)
                && original_id != new_id
            {
                map.insert(original_id, new_id);
            }
        }
        map
    };

    let original_label_to_block: HashMap<i64, BlockId> = func
        .label_id_map
        .iter()
        .map(|(&bid_u32, &label_id)| (label_id, BlockId(bid_u32)))
        .collect();

    let exception_handler_blocks: HashSet<BlockId> = func
        .blocks
        .values()
        .flat_map(|block| block.ops.iter())
        .filter_map(|op| match op.opcode {
            OpCode::CheckException | OpCode::TryStart | OpCode::TryEnd => {
                attr_int(&op.attrs, "value")
                    .and_then(|label_id| original_label_to_block.get(&label_id).copied())
            }
            _ => None,
        })
        .collect();

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

    // ── Structured loop region detection (MLIR ControlFlowToSCF) ──
    // For each LoopHeader with a CondBranch terminator, identify the loop
    // body blocks by DFS from the body-entry successor, stopping at the
    // header (back-edge) and exit block.  These regions are emitted as
    // contiguous loop_start / loop_break_if_X / body / loop_continue /
    // loop_end sequences, enabling the native backend's structured loop
    // optimisations (raw_int_shadow, inline list access, NoGIL fast paths).
    let mut loop_regions: HashMap<BlockId, LoopRegion> = HashMap::new();
    let mut loop_consumed: HashSet<BlockId> = HashSet::new();
    for bid in &rpo {
        if debug_loop_if_return {
            eprintln!(
                "LOWER_DEBUG_PRE_IFPATTERN bid={:?} role={:?} loop_consumed={}",
                bid,
                func.loop_roles
                    .get(bid)
                    .cloned()
                    .unwrap_or(super::blocks::LoopRole::None),
                loop_consumed.contains(bid)
            );
        }
        let role = func
            .loop_roles
            .get(bid)
            .cloned()
            .unwrap_or(super::blocks::LoopRole::None);
        if role != super::blocks::LoopRole::LoopHeader {
            continue;
        }
        // Follow the chain from the LoopHeader to the CondBranch that
        // controls the loop body.  TIR may insert guard blocks (type
        // checks, bounds checks) with their own CondBranch terminators
        // in the header region.  We identify the loop-controlling
        // CondBranch as the one whose successors do NOT contain Raise.
        //
        // The chain is split into:
        //   header_chain: non-guard blocks with Branch terminators
        //                 (emitted inline before break)
        //   guard_chain:  guard blocks with CondBranch + Raise path
        //                 (emitted after break, with labels + br_if)
        let mut header_chain: Vec<BlockId> = Vec::new();
        let mut guard_chain: Vec<BlockId> = Vec::new();
        let mut guard_raise_blocks: Vec<BlockId> = Vec::new();
        let explicit_cond_bid = func.loop_cond_blocks.get(bid).copied();
        let mut cond_bid = explicit_cond_bid.unwrap_or(*bid);
        let mut chain_visited: HashSet<BlockId> = HashSet::new();
        chain_visited.insert(*bid);
        if cond_bid != *bid {
            let mut cur = *bid;
            let mut visited_chain = HashSet::from([cur]);
            while cur != cond_bid {
                let Some(blk) = func.blocks.get(&cur) else {
                    break;
                };
                let Terminator::Branch { target, .. } = &blk.terminator else {
                    break;
                };
                if *target == cond_bid {
                    break;
                }
                if !visited_chain.insert(*target) {
                    break;
                }
                header_chain.push(*target);
                cur = *target;
            }
        }

        // Helper: detect if a block is a raise/error path (within 2 hops).
        let is_guard_raise_path = |check_bid: &BlockId| -> bool {
            let Some(blk) = func.blocks.get(check_bid) else {
                return false;
            };
            if blk.ops.iter().any(|op| op.opcode == OpCode::Raise) {
                return true;
            }
            // One hop further (guard → join block → raise)
            if let Terminator::Branch { target, .. } = &blk.terminator
                && let Some(next) = func.blocks.get(target)
                && next.ops.iter().any(|op| op.opcode == OpCode::Raise)
            {
                return true;
            }
            false
        };

        if explicit_cond_bid.is_none() {
            loop {
                let Some(blk) = func.blocks.get(&cond_bid) else {
                    break;
                };
                match &blk.terminator {
                    Terminator::CondBranch {
                        then_block,
                        else_block,
                        ..
                    } => {
                        let then_raises = is_guard_raise_path(then_block);
                        let else_raises = is_guard_raise_path(else_block);
                        if !then_raises && !else_raises {
                            break; // Neither path raises — this is the loop control
                        }
                        // One path raises — this is a guard CondBranch.
                        // Record it as a guard (not a non-guard chain block).
                        if cond_bid != *bid {
                            guard_chain.push(cond_bid);
                        }
                        // Collect raise-path blocks for consumption.
                        let raise_bid = if then_raises {
                            *then_block
                        } else {
                            *else_block
                        };
                        guard_raise_blocks.extend(collect_guard_raise_path_blocks(func, raise_bid));
                        // Follow the non-raising path.
                        let next = if then_raises {
                            *else_block
                        } else {
                            *then_block
                        };
                        if !chain_visited.insert(next) {
                            break; // Cycle — this IS the loop control
                        }
                        cond_bid = next;
                    }
                    Terminator::Branch { target, .. } => {
                        if !chain_visited.insert(*target) {
                            break; // Cycle
                        }
                        if cond_bid != *bid {
                            header_chain.push(cond_bid);
                        }
                        cond_bid = *target;
                    }
                    _ => break, // Return/Unreachable — give up
                }
            }
        }
        let Some(cond_block_data) = func.blocks.get(&cond_bid) else {
            continue;
        };
        let Terminator::CondBranch {
            cond,
            then_block,
            then_args,
            else_block,
            else_args,
        } = &cond_block_data.terminator
        else {
            continue;
        };
        let break_kind = func
            .loop_break_kinds
            .get(bid)
            .copied()
            .unwrap_or(LoopBreakKind::BreakIfFalse);
        let (body_entry, exit_block, body_args, exit_args) = match break_kind {
            LoopBreakKind::BreakIfFalse => (
                *then_block,
                *else_block,
                then_args.clone(),
                else_args.clone(),
            ),
            LoopBreakKind::BreakIfTrue => (
                *else_block,
                *then_block,
                else_args.clone(),
                then_args.clone(),
            ),
        };
        if debug_loop_if_return {
            eprintln!(
                "LOWER_DEBUG_REGION bid={:?} cond_bid={:?} break_kind={:?} body_entry={:?} exit_block={:?} then={:?} else={:?}",
                bid, cond_bid, break_kind, body_entry, exit_block, then_block, else_block
            );
        }
        // Collect body blocks via DFS from body_entry, stopping at the
        // header (back-edge), header chain blocks, guard chain blocks,
        // cond block, and exit.
        let mut header_set: HashSet<BlockId> = HashSet::new();
        header_set.insert(*bid);
        header_set.insert(cond_bid);
        for hc in &header_chain {
            header_set.insert(*hc);
        }
        for gc in &guard_chain {
            header_set.insert(*gc);
        }
        let mut body_set = HashSet::new();
        {
            let mut stack = vec![body_entry];
            while let Some(b) = stack.pop() {
                if header_set.contains(&b) || b == exit_block || !body_set.insert(b) {
                    continue;
                }
                if let Some(blk) = func.blocks.get(&b) {
                    for succ in successors_of(blk) {
                        stack.push(succ);
                    }
                }
            }
        }
        // Exclude LoopEnd structural markers — they are not real body blocks.
        body_set.retain(|b| {
            func.loop_roles
                .get(b)
                .cloned()
                .unwrap_or(super::blocks::LoopRole::None)
                != super::blocks::LoopRole::LoopEnd
                && !exception_handler_blocks.contains(b)
        });
        guard_raise_blocks.retain(|b| !exception_handler_blocks.contains(b));
        // Mark body blocks, header chain, guard chain, cond block, AND
        // guard raise-path blocks as consumed.
        loop_consumed.extend(body_set.iter().copied());
        for hc in &header_chain {
            loop_consumed.insert(*hc);
        }
        for gc in &guard_chain {
            loop_consumed.insert(*gc);
        }
        for rb in &guard_raise_blocks {
            loop_consumed.insert(*rb);
        }
        if cond_bid != *bid {
            loop_consumed.insert(cond_bid);
        }
        loop_regions.insert(
            *bid,
            LoopRegion {
                guard_chain,
                guard_raise_blocks,
                cond_block: cond_bid,
                body_entry,
                exit_block,
                body_set,
                break_kind,
                cond: *cond,
                body_args,
                exit_args,
            },
        );
    }

    // ── Structured if/else/end_if detection ──
    // Detect simple CondBranch patterns where both successors:
    //   (a) have no check_exception ops (which require label blocks for implicit edges)
    //   (b) have simple terminators (Branch to same join block, or Return/Unreachable)
    //   (c) are not claimed by another pattern
    //   (d) neither successor is a loop header (loop body blocks need
    //       their own labels for back-edge resolution)
    //
    // These patterns are emitted as if/else/end_if + phi ops, producing
    // cleaner CLIF without extra unsealed label blocks.
    struct IfPattern {
        then_bid: BlockId,
        else_bid: BlockId,
        join_bid: Option<BlockId>,
    }
    let block_contains_nested_scf = |block: &TirBlock| {
        block.ops.iter().any(|op| {
            matches!(
                op.opcode,
                OpCode::ScfIf | OpCode::ScfFor | OpCode::ScfWhile | OpCode::ScfYield
            )
        })
    };
    let mut if_patterns: HashMap<BlockId, IfPattern> = HashMap::new();
    let mut if_inlined_blocks: HashSet<BlockId> = HashSet::new();
    let mut predecessors: HashMap<BlockId, HashSet<BlockId>> = HashMap::new();
    for (pred_bid, block) in &func.blocks {
        for succ in successors_of(block) {
            predecessors.entry(succ).or_default().insert(*pred_bid);
        }
    }

    for bid in &rpo {
        let role = func
            .loop_roles
            .get(bid)
            .cloned()
            .unwrap_or(super::blocks::LoopRole::None);
        if role != super::blocks::LoopRole::None || loop_consumed.contains(bid) {
            continue;
        }
        let Some(block) = func.blocks.get(bid) else {
            continue;
        };
        let Terminator::CondBranch {
            then_block,
            else_block,
            ..
        } = &block.terminator
        else {
            continue;
        };
        let (then_bid, else_bid) = (*then_block, *else_block);
        if then_bid == else_bid {
            continue;
        }
        // Successor blocks that are loop headers must not be inlined.
        let then_role = func
            .loop_roles
            .get(&then_bid)
            .cloned()
            .unwrap_or(super::blocks::LoopRole::None);
        let else_role = func
            .loop_roles
            .get(&else_bid)
            .cloned()
            .unwrap_or(super::blocks::LoopRole::None);
        if then_role != super::blocks::LoopRole::None || else_role != super::blocks::LoopRole::None
        {
            continue;
        }
        if exception_handler_blocks.contains(&then_bid)
            || exception_handler_blocks.contains(&else_bid)
        {
            continue;
        }
        let Some(then_blk) = func.blocks.get(&then_bid) else {
            continue;
        };
        let Some(else_blk) = func.blocks.get(&else_bid) else {
            continue;
        };
        let then_predecessors = predecessors.get(&then_bid).cloned().unwrap_or_default();
        if then_predecessors.iter().any(|pred| *pred != *bid) {
            continue;
        }
        let else_predecessors = predecessors.get(&else_bid).cloned().unwrap_or_default();
        if else_predecessors.iter().any(|pred| *pred != *bid) {
            continue;
        }
        if if_inlined_blocks.contains(&then_bid) || if_inlined_blocks.contains(&else_bid) {
            continue;
        }
        // Successors that carry exception-region ops need explicit labels.
        let successor_needs_label = |block: &TirBlock| {
            block.ops.iter().any(|op| {
                matches!(
                    op.opcode,
                    OpCode::CheckException | OpCode::TryStart | OpCode::TryEnd
                )
            })
        };
        if successor_needs_label(then_blk) {
            continue;
        }
        if successor_needs_label(else_blk) {
            continue;
        }
        // Only straight-line successors are safe to inline as structured
        // if/else/end_if. Nested SCF inside a successor needs label-based
        // lowering so the nested region retains an explicit merge edge.
        if block_contains_nested_scf(then_blk) || block_contains_nested_scf(else_blk) {
            continue;
        }
        // Simple terminators only.
        let then_target = match &then_blk.terminator {
            Terminator::Branch { target, .. } => Some(*target),
            Terminator::Return { .. } | Terminator::Unreachable => None,
            _ => {
                continue;
            }
        };
        let else_target = match &else_blk.terminator {
            Terminator::Branch { target, .. } => Some(*target),
            Terminator::Return { .. } | Terminator::Unreachable => None,
            _ => {
                continue;
            }
        };
        let join_bid = match (then_target, else_target) {
            (Some(t), Some(e)) if t == e => Some(t),
            (None, None) => None,
            _ => {
                continue;
            }
        };
        if join_bid.is_some_and(|join| exception_handler_blocks.contains(&join)) {
            continue;
        }
        if let Some(join) = join_bid {
            let join_role = func
                .loop_roles
                .get(&join)
                .cloned()
                .unwrap_or(super::blocks::LoopRole::None);
            if join_role != super::blocks::LoopRole::None {
                continue;
            }
            let join_predecessors = predecessors.get(&join).cloned().unwrap_or_default();
            if join_predecessors
                .iter()
                .any(|pred| *pred != then_bid && *pred != else_bid)
            {
                continue;
            }
        }
        if_patterns.insert(
            *bid,
            IfPattern {
                then_bid,
                else_bid,
                join_bid,
            },
        );
        if debug_loop_if_return {
            eprintln!(
                "LOWER_DEBUG_IF_PATTERN bid={:?} then={:?} else={:?} join={:?}",
                bid, then_bid, else_bid, join_bid
            );
        }
        if_inlined_blocks.insert(then_bid);
        if_inlined_blocks.insert(else_bid);
    }
    for bid in &rpo {
        if debug_loop_if_return {
            eprintln!(
                "LOWER_DEBUG_EMIT bid={:?} loop_consumed={} role={:?} if_inlined={}",
                bid,
                loop_consumed.contains(bid),
                func.loop_roles
                    .get(bid)
                    .cloned()
                    .unwrap_or(super::blocks::LoopRole::None),
                if_inlined_blocks.contains(bid)
            );
        }
        // Skip blocks consumed by structured loop emission.
        if loop_consumed.contains(bid) {
            continue;
        }

        let loop_role = func
            .loop_roles
            .get(bid)
            .cloned()
            .unwrap_or(super::blocks::LoopRole::None);
        // Skip LoopEnd blocks only when nothing still branches to them.
        // Some loop-end blocks survive optimization as explicit CFG targets,
        // and dropping their labels leaves dangling jump targets in the
        // round-tripped SimpleIR.
        let has_explicit_predecessor = predecessors.get(bid).is_some_and(|preds| !preds.is_empty());
        if loop_role == super::blocks::LoopRole::LoopEnd && !has_explicit_predecessor {
            continue;
        }

        // Skip blocks inlined inside structured if/else/end_if regions.
        if if_inlined_blocks.contains(bid) {
            continue;
        }

        // ── Structured loop emission ──
        // If this block is a LoopHeader with a detected region, emit the
        // entire structured loop (header + body + back-edge) and skip to
        // the next RPO block.
        if loop_role == super::blocks::LoopRole::LoopHeader && loop_regions.contains_key(bid) {
            emit_structured_loop_region(
                *bid,
                func,
                &loop_regions,
                &rpo,
                &block_param_vars,
                &block_label_id,
                &if_inlined_blocks,
                types,
                &original_to_new_label,
                &original_label_to_block,
                &mut out,
                &mut loop_consumed,
            );
            continue;
        }

        let block = match func.blocks.get(bid) {
            Some(b) => b,
            None => continue,
        };

        // Emit block header: label for non-entry blocks.
        // LoopHeaders with regions are handled above; remaining LoopHeaders
        // (no CondBranch terminator) fall through to emit loop_start + label.
        if *bid != func.entry_block {
            if loop_role == super::blocks::LoopRole::LoopHeader {
                out.push(OpIR {
                    kind: "label".to_string(),
                    value: Some(block_label_id(bid)),
                    ..OpIR::default()
                });
                out.push(OpIR {
                    kind: "loop_start".to_string(),
                    ..OpIR::default()
                });
            } else {
                out.push(OpIR {
                    kind: "label".to_string(),
                    value: Some(block_label_id(bid)),
                    ..OpIR::default()
                });
            }

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
        let emit_block_ops = |block: &TirBlock, out: &mut Vec<OpIR>| {
            emit_block_ops_inner(
                block,
                types,
                &original_to_new_label,
                &original_label_to_block,
                &block_param_vars,
                out,
            );
        };

        if let Some(pattern) = if_patterns.get(bid) {
            // ── Structured if/else/end_if emission ──
            // Emit the current block's ops, then inline the then/else
            // blocks between if/else/end_if markers with phi ops.
            emit_block_ops(block, &mut out);

            let Terminator::CondBranch { cond, .. } = &block.terminator else {
                unreachable!();
            };

            let then_blk = func
                .blocks
                .get(&pattern.then_bid)
                .expect("then block missing");
            let else_blk = func
                .blocks
                .get(&pattern.else_bid)
                .expect("else block missing");
            let original_has_ret = func
                .attrs
                .get("_original_has_ret")
                .map(|v| matches!(v, super::ops::AttrValue::Bool(true)))
                .unwrap_or(false);

            // Materialize join block arguments as explicit store_var writes on
            // the then/else edges. The join block itself already re-loads its
            // block args via load_var when emitted later.
            let join_arg_stores: Vec<(String, String, String)> =
                if let Some(join_bid) = pattern.join_bid {
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
                            let join_param_var =
                                join_param_vars.and_then(|vars| vars.get(i)).cloned()?;
                            let join_value_name = join_blk
                                .and_then(|b| b.args.get(i))
                                .map(|a| value_var(a.id))?;
                            let then_val = then_branch_args
                                .get(i)
                                .map(|v| value_var(*v))
                                .unwrap_or_else(|| join_value_name.clone());
                            let else_val = else_branch_args
                                .get(i)
                                .map(|v| value_var(*v))
                                .unwrap_or_else(|| join_value_name.clone());
                            Some((join_param_var, then_val, else_val))
                        })
                        .collect()
                } else {
                    vec![]
                };

            // Emit: if cond
            out.push(OpIR {
                kind: "if".to_string(),
                args: Some(vec![value_var(*cond)]),
                ..OpIR::default()
            });

            // Emit then-block ops inline.
            for op in &then_blk.ops {
                if let Some(mut opir) = lower_op(op) {
                    annotate_type_flags(&mut opir, op, types);
                    if matches!(
                        opir.kind.as_str(),
                        "check_exception" | "try_start" | "try_end"
                    ) && let Some(orig_id) = opir.value
                        && let Some(&new_id) = original_to_new_label.get(&orig_id)
                    {
                        opir.value = Some(new_id);
                    }
                    out.push(opir);
                }
            }
            // Emit then-block terminator if terminal (Return).
            if let Terminator::Return { values } = &then_blk.terminator {
                emit_return_ops(values, original_has_ret, &mut out);
            }

            for (join_param_var, then_val, _) in &join_arg_stores {
                out.push(OpIR {
                    kind: "store_var".to_string(),
                    var: Some(join_param_var.clone()),
                    args: Some(vec![then_val.clone()]),
                    ..OpIR::default()
                });
            }

            // Emit: else
            out.push(OpIR {
                kind: "else".to_string(),
                ..OpIR::default()
            });

            // Emit else-block ops inline.
            for op in &else_blk.ops {
                if let Some(mut opir) = lower_op(op) {
                    annotate_type_flags(&mut opir, op, types);
                    if matches!(
                        opir.kind.as_str(),
                        "check_exception" | "try_start" | "try_end"
                    ) && let Some(orig_id) = opir.value
                        && let Some(&new_id) = original_to_new_label.get(&orig_id)
                    {
                        opir.value = Some(new_id);
                    }
                    out.push(opir);
                }
            }
            // Emit else-block terminator if terminal (Return).
            if let Terminator::Return { values } = &else_blk.terminator {
                emit_return_ops(values, original_has_ret, &mut out);
            }

            for (join_param_var, _, else_val) in &join_arg_stores {
                out.push(OpIR {
                    kind: "store_var".to_string(),
                    var: Some(join_param_var.clone()),
                    args: Some(vec![else_val.clone()]),
                    ..OpIR::default()
                });
            }

            // Emit: end_if
            out.push(OpIR {
                kind: "end_if".to_string(),
                ..OpIR::default()
            });
        } else {
            // Non-loop, non-if-pattern block: emit ops and terminator normally.
            emit_block_ops(block, &mut out);
            let original_has_ret = func
                .attrs
                .get("_original_has_ret")
                .map(|v| matches!(v, super::ops::AttrValue::Bool(true)))
                .unwrap_or(false);
            emit_terminator(
                block,
                &block_param_vars,
                &block_label_id,
                &if_inlined_blocks,
                &func.loop_roles,
                &mut out,
                original_has_ret,
                loop_role,
                &func.loop_break_kinds,
            );
        }
    }

    VALUE_NAME_OVERRIDES.with(|overrides| overrides.borrow_mut().clear());

    if debug_loop_if_return {
        eprintln!("LOWER_DEBUG_PRE_ELIM: {out:#?}");
    }
    eliminate_dead_labels(&mut out);
    if let Err(detail) = validate_structured_if_markers(&out) {
        panic!(
            "[TIR] invalid structured if lowering for {}: {}",
            func.name, detail
        );
    }

    // Validate: every label referenced by check_exception/jump/br_if must
    // have a corresponding label op. If validation fails, it means the
    // TIR roundtrip lost a handler block's label mapping.
    let warn_invalid_labels = func.has_exception_handling
        || std::env::var("MOLT_TIR_WARN_INVALID_LABELS").as_deref() == Ok("1");
    if warn_invalid_labels && !validate_labels(&out) {
        let missing = missing_label_references(&out);
        eprintln!(
            "[TIR] WARNING: label validation failed for {} — missing labels {:?}",
            func.name, missing
        );
        for (idx, op) in out.iter().enumerate() {
            if matches!(
                op.kind.as_str(),
                "label"
                    | "state_label"
                    | "jump"
                    | "br_if"
                    | "check_exception"
                    | "try_start"
                    | "try_end"
                    | "if"
                    | "else"
                    | "end_if"
            ) {
                eprintln!(
                    "  [TIR:{}] {} kind={} value={:?} args={:?}",
                    func.name, idx, op.kind, op.value, op.args
                );
            }
        }
    }

    out
}

/// Remove dead `label` ops from the linearised op stream.
///
/// A "dead label" is a `label`/`state_label` op whose label id is never
/// the target of any `jump`, `br_if`, or `check_exception` op AND whose
/// preceding instruction has already terminated the block (i.e., the label
/// is not reachable via fallthrough either).
///
/// The Cranelift backend creates a block for every label it sees in its
/// pre-scan.  If that block ends up with no predecessors (no branch targets
/// it AND no fallthrough), Cranelift's alias_analysis and block ordering
/// panic with `Option::unwrap() on None`.
///
/// This pass strips only the dead label ops themselves.  The code following
/// a dead label is kept: it may be reachable via structured control flow
/// (e.g., `loop_end` switches to an `after_block` and the following ops
/// are emitted into that block).
fn eliminate_dead_labels(ops: &mut Vec<OpIR>) {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum FilledState {
        Open,
        Closed,
        LoopContinue,
    }

    loop {
        // Phase 1: collect all label ids that are explicit branch targets.
        let mut branch_targets: HashSet<i64> = HashSet::new();
        for op in ops.iter() {
            match op.kind.as_str() {
                "jump" | "br_if" | "check_exception" | "loop_continue" => {
                    if let Some(id) = op.value {
                        branch_targets.insert(id);
                    }
                }
                _ => {}
            }
        }

        // Phase 2: walk ops, detecting dead labels.
        // `is_filled` tracks whether the current block has been terminated
        // (by jump/ret/raise/loop_continue) without a subsequent label
        // starting a new live block.
        let mut filled_state = FilledState::Open;
        let mut current_block_started_at_live_label = false;
        let mut keep = vec![true; ops.len()];

        for i in 0..ops.len() {
            let kind = ops[i].kind.as_str();
            match kind {
                "jump" | "ret" | "raise" | "loop_break" => {
                    if filled_state != FilledState::Open {
                        keep[i] = false;
                    } else {
                        filled_state = FilledState::Closed;
                    }
                }
                "loop_continue" => {
                    if filled_state != FilledState::Open {
                        keep[i] = false;
                    } else {
                        filled_state = FilledState::LoopContinue;
                    }
                }
                "label" | "state_label" => {
                    let label_id = ops[i].value.unwrap_or(-1);
                    if filled_state != FilledState::Open && !branch_targets.contains(&label_id) {
                        // Dead label: preceded by a terminator and not a
                        // branch target.  Remove the label op but keep the
                        // code that follows (it may be reachable via
                        // structured control flow like loop_end → after_block).
                        keep[i] = false;
                    } else {
                        // Live label (reachable via fallthrough or branch).
                        filled_state = FilledState::Open;
                        current_block_started_at_live_label = true;
                    }
                }
                // loop_end resets the filled state only when the current block
                // is still live, or when it closes the implicit break path of
                // a structured loop after a textual `loop_continue`.
                "loop_end" => {
                    if filled_state == FilledState::Closed && !current_block_started_at_live_label {
                        keep[i] = false;
                    } else {
                        filled_state = FilledState::Open;
                        current_block_started_at_live_label = false;
                    }
                }
                "if" | "else" | "end_if" => {
                    // Structured if markers remain live even when the
                    // immediately preceding textual branch returned or raised.
                    // If a dead labeled path falls into a structured `if`,
                    // stripping only the opening `if` while preserving
                    // `else` / `end_if` corrupts the control stack.
                    filled_state = FilledState::Open;
                    current_block_started_at_live_label = false;
                }
                // loop_start, loop_break_if_false/true do not fill.
                "loop_start"
                | "loop_break_if_false"
                | "loop_break_if_true"
                | "loop_index_start" => {
                    // These are control-flow markers that don't terminate blocks.
                    if kind == "loop_start" {
                        current_block_started_at_live_label = false;
                    }
                }
                "br_if" => {
                    if filled_state != FilledState::Open {
                        keep[i] = false;
                    } else {
                        // br_if has a fallthrough path — does not fill.
                        filled_state = FilledState::Open;
                    }
                }
                _ => {
                    if filled_state != FilledState::Open {
                        // Once a block is terminated, any straight-line ops that
                        // follow before the next live label are unreachable. Keep
                        // only the structural boundary ops handled above.
                        keep[i] = false;
                    }
                }
            }
        }

        // Phase 3: compact — remove dead ops.
        let old_len = ops.len();
        let mut write_idx = 0;
        for read_idx in 0..ops.len() {
            if keep[read_idx] {
                if write_idx != read_idx {
                    ops.swap(write_idx, read_idx);
                }
                write_idx += 1;
            }
        }
        ops.truncate(write_idx);
        if ops.len() == old_len {
            break;
        }
    }
}

fn validate_structured_if_markers(ops: &[OpIR]) -> Result<(), String> {
    #[derive(Clone, Copy)]
    struct IfFrame {
        if_idx: usize,
        saw_else: bool,
    }

    let mut stack: Vec<IfFrame> = Vec::new();
    for (idx, op) in ops.iter().enumerate() {
        match op.kind.as_str() {
            "if" => stack.push(IfFrame {
                if_idx: idx,
                saw_else: false,
            }),
            "else" => {
                let Some(frame) = stack.last_mut() else {
                    return Err(format!("orphan else at op {idx}"));
                };
                if frame.saw_else {
                    return Err(format!(
                        "duplicate else at op {idx} for if starting at op {}",
                        frame.if_idx
                    ));
                }
                frame.saw_else = true;
            }
            "end_if" => {
                let Some(_frame) = stack.pop() else {
                    return Err(format!("orphan end_if at op {idx}"));
                };
            }
            _ => {}
        }
    }
    if let Some(frame) = stack.last() {
        return Err(format!("unterminated if starting at op {}", frame.if_idx));
    }
    Ok(())
}

/// Validate that every label referenced by jump/br_if/check_exception exists
/// as a label op in the output.  Returns false if any reference is dangling.
pub fn validate_labels(ops: &[crate::ir::OpIR]) -> bool {
    missing_label_references(ops).is_empty()
}

fn missing_label_references(ops: &[crate::ir::OpIR]) -> Vec<i64> {
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
    let mut missing: Vec<i64> = referenced_labels
        .difference(&defined_labels)
        .copied()
        .collect();
    missing.sort_unstable();
    missing
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
            // Both the SSA lift and SCCP store ConstBool values as
            // AttrValue::Bool.  Legacy AttrValue::Int is handled for
            // backward compatibility with cached TIR artifacts.
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
        OpCode::Shl => Some(binary_op("lshift", op, out_var)),
        OpCode::Shr => Some(binary_op("rshift", op, out_var)),

        // Boolean.
        OpCode::And => Some(binary_op("and", op, out_var)),
        OpCode::Or => Some(binary_op("or", op, out_var)),
        OpCode::Not => Some(unary_op("not", op, out_var)),
        OpCode::Bool => Some(unary_op("bool", op, out_var)),

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
            let mut opir = binary_op(&kind, op, out_var);
            // Restore container_type (e.g., "list_int") from the preserved attr
            // so the backend can emit inline list access.
            opir.container_type = attr_str(&op.attrs, "container_type");
            // Propagate BCE proof so codegen can skip bounds checks.
            opir.bce_safe = attr_bool(&op.attrs, "bce_safe");
            Some(opir)
        }
        OpCode::StoreIndex => {
            let kind =
                attr_str(&op.attrs, "_original_kind").unwrap_or_else(|| "store_index".to_string());
            Some(OpIR {
                kind,
                args: Some(operand_args(op)),
                out: out_var,
                container_type: attr_str(&op.attrs, "container_type"),
                // Propagate BCE proof so codegen can skip bounds checks.
                bce_safe: attr_bool(&op.attrs, "bce_safe"),
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
                        task_kind: attr_str(&op.attrs, "task_kind"),
                        container_type: attr_str(&op.attrs, "container_type"),
                        ic_index: attr_int(&op.attrs, "ic_index"),
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
                    task_kind: attr_str(&op.attrs, "task_kind"),
                    container_type: attr_str(&op.attrs, "container_type"),
                    ic_index: attr_int(&op.attrs, "ic_index"),
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
        OpCode::AllocTask => Some(OpIR {
            kind: "alloc_task".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            s_value: attr_str(&op.attrs, "s_value"),
            value: attr_int(&op.attrs, "value"),
            task_kind: attr_str(&op.attrs, "task_kind"),
            ..OpIR::default()
        }),
        OpCode::StateSwitch => Some(OpIR {
            kind: "state_switch".to_string(),
            ..OpIR::default()
        }),
        OpCode::StateTransition => Some(OpIR {
            kind: "state_transition".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            value: attr_int(&op.attrs, "value"),
            ..OpIR::default()
        }),
        OpCode::StateYield => Some(OpIR {
            kind: "state_yield".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            value: attr_int(&op.attrs, "value"),
            ..OpIR::default()
        }),
        OpCode::ChanSendYield => Some(OpIR {
            kind: "chan_send_yield".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            value: attr_int(&op.attrs, "value"),
            ..OpIR::default()
        }),
        OpCode::ChanRecvYield => Some(OpIR {
            kind: "chan_recv_yield".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            value: attr_int(&op.attrs, "value"),
            ..OpIR::default()
        }),
        OpCode::ClosureLoad => Some(OpIR {
            kind: "closure_load".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            value: attr_int(&op.attrs, "value"),
            ..OpIR::default()
        }),
        OpCode::ClosureStore => Some(OpIR {
            kind: "closure_store".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            value: attr_int(&op.attrs, "value"),
            ..OpIR::default()
        }),
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
        OpCode::Import => {
            let args = operand_args(op);
            if args.is_empty() {
                Some(OpIR {
                    kind: "import".to_string(),
                    s_value: attr_str(&op.attrs, "module"),
                    out: out_var,
                    ..OpIR::default()
                })
            } else {
                Some(OpIR {
                    kind: "module_import".to_string(),
                    args: Some(args),
                    out: out_var,
                    ..OpIR::default()
                })
            }
        }
        OpCode::ImportFrom => Some(OpIR {
            kind: "import_from".to_string(),
            s_value: attr_str(&op.attrs, "name"),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::WarnStderr => Some(OpIR {
            kind: "warn_stderr".to_string(),
            args: Some(operand_args(op)),
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
// Structured loop emission
// ---------------------------------------------------------------------------

/// Emit a structured loop region: label → loop_start → header ops →
/// loop_break_if_X → body blocks → loop_continue → loop_end.
///
/// Body blocks are emitted in RPO order with labels for internal control
/// flow.  Nested loops within the body are emitted recursively.
/// Back-edges (Branch → header) become `loop_continue`.
/// Branches to the exit block become `loop_break`.
fn emit_structured_loop_region(
    header: BlockId,
    func: &TirFunction,
    loop_regions: &HashMap<BlockId, LoopRegion>,
    rpo: &[BlockId],
    block_param_vars: &HashMap<BlockId, Vec<String>>,
    block_label_id: &dyn Fn(&BlockId) -> i64,
    if_inlined_blocks: &HashSet<BlockId>,
    types: &HashMap<ValueId, TirType>,
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
        .map(|v| matches!(v, super::ops::AttrValue::Bool(true)))
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
            loop {
                let Some(blk) = func.blocks.get(&cur) else {
                    break;
                };
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
                types,
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
                                types,
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
            .unwrap_or(super::blocks::LoopRole::None);

        // Nested LoopHeader: recursively emit its structured loop.
        // The recursive call handles label, loop_start, ops, break,
        // body, continue, loop_end — so we just call it and skip.
        if body_role == super::blocks::LoopRole::LoopHeader && loop_regions.contains_key(body_bid) {
            emit_structured_loop_region(
                *body_bid,
                func,
                loop_regions,
                rpo,
                block_param_vars,
                block_label_id,
                if_inlined_blocks,
                types,
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
                let mut visited = std::collections::HashSet::new();
                visited.insert(cur);
                loop {
                    let Some(blk) = func.blocks.get(&cur) else {
                        break;
                    };
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
            types,
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
                    &func.loop_roles,
                    out,
                    original_has_ret,
                    body_role,
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
            types,
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
        .unwrap_or(super::blocks::LoopRole::None);
    let exit_needs_fallthrough = if_inlined_blocks.contains(&region.exit_block)
        || region.exit_block == func.entry_block
        || exit_role == super::blocks::LoopRole::LoopHeader;
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
            types,
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
            &func.loop_roles,
            out,
            original_has_ret,
            super::blocks::LoopRole::None,
            &func.loop_break_kinds,
        );
    }
}

/// Emit the raise-path blocks for a guard CondBranch.
///
/// Follows Branch chains from `start_bid`, emitting each block with a
/// label, block arg loads, ops, and terminators.  This handles patterns
/// like guard → join → raise where the raise is 1-2 hops away.
fn emit_guard_raise_path(
    start_bid: BlockId,
    start_args: &[ValueId],
    raise_path_blocks: &HashSet<BlockId>,
    func: &TirFunction,
    block_param_vars: &HashMap<BlockId, Vec<String>>,
    block_label_id: &dyn Fn(&BlockId) -> i64,
    if_inlined_blocks: &HashSet<BlockId>,
    types: &HashMap<ValueId, TirType>,
    original_to_new_label: &HashMap<i64, i64>,
    label_to_block: &HashMap<i64, BlockId>,
    out: &mut Vec<OpIR>,
) {
    let original_has_ret = func
        .attrs
        .get("_original_has_ret")
        .map(|v| matches!(v, super::ops::AttrValue::Bool(true)))
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
            types,
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
                    &func.loop_roles,
                    out,
                    original_has_ret,
                    super::blocks::LoopRole::None,
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
                    &func.loop_roles,
                    out,
                    original_has_ret,
                    super::blocks::LoopRole::None,
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
                    &func.loop_roles,
                    out,
                    original_has_ret,
                    super::blocks::LoopRole::None,
                    &func.loop_break_kinds,
                );
                break;
            }
        }
    }
}

/// Emit a block's ops with type annotations and label remapping.
/// Shared by both the main emission loop and structured loop emission.
fn emit_block_ops_inner(
    block: &TirBlock,
    types: &HashMap<ValueId, TirType>,
    original_to_new_label: &HashMap<i64, i64>,
    original_label_to_block: &HashMap<i64, BlockId>,
    block_param_vars: &HashMap<BlockId, Vec<String>>,
    out: &mut Vec<OpIR>,
) {
    for op in &block.ops {
        if op.opcode == OpCode::CheckException
            && let Some(orig_id) = attr_int(&op.attrs, "value")
            && let Some(&handler_block) = original_label_to_block.get(&orig_id)
        {
            emit_block_arg_stores(handler_block, &op.operands, block_param_vars, out);
        }
        if let Some(mut opir) = lower_op(op) {
            annotate_type_flags(&mut opir, op, types);
            if matches!(
                opir.kind.as_str(),
                "check_exception" | "try_start" | "try_end"
            ) && let Some(orig_id) = opir.value
                && let Some(&new_id) = original_to_new_label.get(&orig_id)
            {
                opir.value = Some(new_id);
            }
            out.push(opir);
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
    if_inlined_blocks: &HashSet<BlockId>,
    loop_roles: &HashMap<BlockId, super::blocks::LoopRole>,
    out: &mut Vec<OpIR>,
    original_has_ret: bool,
    loop_role: super::blocks::LoopRole,
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

            // If target is a LoopHeader, this is a back-edge → loop_continue.
            let target_role = loop_roles
                .get(target)
                .cloned()
                .unwrap_or(super::blocks::LoopRole::None);
            if loop_role == super::blocks::LoopRole::LoopEnd
                && target_role == super::blocks::LoopRole::LoopHeader
            {
                out.push(OpIR {
                    kind: "loop_continue".to_string(),
                    ..OpIR::default()
                });
            } else if if_inlined_blocks.contains(target) {
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

        Terminator::Unreachable => {
            out.push(OpIR {
                kind: "unreachable".to_string(),
                ..OpIR::default()
            });
        }
    }
}

/// Emit `store_var` ops to pass values to the target block's argument variables.
fn emit_block_arg_stores(
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
    if func.has_exception_handling && visited.len() < func.blocks.len() {
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
// Structural annotation propagation
// ---------------------------------------------------------------------------

/// Annotate a SimpleIR [`OpIR`] with non-semantic transport metadata that is
/// still required by specific backend consumers.
fn annotate_type_flags(opir: &mut OpIR, tir_op: &TirOp, types: &HashMap<ValueId, TirType>) {
    // Propagate StackAlloc: if the TIR op is StackAlloc, mark the SimpleIR op
    // so the native backend can emit stack allocation instead of heap allocation.
    // Also mark it as arena-eligible for the scope arena integration.
    if tir_op.opcode == OpCode::StackAlloc {
        opir.stack_eligible = Some(true);
        opir.arena_eligible = Some(true);
    }

    // Restore col_offset/end_col_offset for traceback caret annotations.
    if opir.col_offset.is_none()
        && let Some(AttrValue::Int(col)) = tir_op.attrs.get("_col_offset")
    {
        opir.col_offset = Some(*col);
    }
    if opir.end_col_offset.is_none()
        && let Some(AttrValue::Int(ecol)) = tir_op.attrs.get("_end_col_offset")
    {
        opir.end_col_offset = Some(*ecol);
    }

    // Propagate proven type information from the TIR type map into type_hint
    // so the native backend's preanalysis can classify variables that the
    // op-based heuristics cannot (e.g. values proven int after a TypeGuard).
    //
    // We skip ops whose SimpleIR kind already fully determines the output
    // type in preanalysis (constants, comparisons, arithmetic, copies, etc.)
    // to avoid redundant metadata.  Only ops that fall into the catch-all
    // branch of the preanalysis benefit from type_hint.
    if opir.type_hint.is_none()
        && !tir_op.results.is_empty()
        && !op_kind_already_classified(opir.kind.as_str())
    {
        if let Some(ty) = tir_op
            .results
            .first()
            .and_then(|r| types.get(r))
        {
            match ty {
                TirType::I64 => {
                    opir.type_hint = Some("int".into());
                }
                TirType::F64 => {
                    opir.type_hint = Some("float".into());
                }
                TirType::Bool => {
                    opir.type_hint = Some("bool".into());
                }
                TirType::Str => {
                    opir.type_hint = Some("str".into());
                }
                _ => {}
            }
        }
    }
}

/// Returns true if the SimpleIR op kind is already classified by the native
/// backend's preanalysis without needing a type_hint.  These ops have their
/// output type fully determined by the op kind or by source-argument analysis.
fn op_kind_already_classified(kind: &str) -> bool {
    matches!(
        kind,
        // Constants — type determined by kind.
        "const"
            | "const_bool"
            | "const_float"
            | "const_none"
            | "const_str"
            | "loop_index_start"
            | "loop_index_next"
            | "len"
            // Comparisons — always Bool.
            | "lt"
            | "le"
            | "gt"
            | "ge"
            | "eq"
            | "ne"
            | "is"
            | "bool"
            | "cast_bool"
            | "builtin_bool"
            | "is_truthy"
            | "not"
            // Copies — type comes from source argument.
            | "copy"
            | "copy_var"
            | "load_var"
            | "identity_alias"
            // Arithmetic — type inferred from operand lanes.
            | "add"
            | "inplace_add"
            | "sub"
            | "mul"
            | "inplace_sub"
            | "inplace_mul"
            | "floordiv"
            | "mod"
            | "mod_"
            | "inplace_floordiv"
            | "inplace_mod"
            | "bit_and"
            | "bit_or"
            | "bit_xor"
            | "bitand"
            | "bitor"
            | "bitxor"
            | "inplace_bit_and"
            | "inplace_bit_or"
            | "inplace_bit_xor"
            | "lshift"
            | "rshift"
            | "shl"
            | "shr"
            | "neg"
            | "pos"
            | "abs"
            | "invert"
            | "builtin_abs"
            // GPU intrinsics — always Int.
            | "gpu_thread_id"
            | "gpu_block_id"
            | "gpu_block_dim"
            | "gpu_grid_dim"
            // Heap-allocating container constructors — kind alone fully
            // determines the output type, so propagating a scalar type_hint
            // ("int"/"float"/...) would lie to the backend.  The TIR type
            // map can spuriously bind I64 to these results through SSA
            // aliasing of constant operands; classifying them here ensures
            // we never reseed legacy container hints.
            | "list_new"
            | "dict_new"
            | "set_new"
            | "tuple_new"
            | "frozenset_new"
            | "build_list"
            | "build_dict"
            | "build_tuple"
            | "build_set"
            | "build_slice"
            // Container side-effect ops — return None or a container handle
            // whose type is structurally implied; never benefit from a
            // scalar type_hint.
            | "list_append"
            | "list_extend"
            | "dict_set"
            | "set_add"
            | "store_attr"
            | "store_index"
            | "del_attr"
            | "del_index"
            | "store_var"
    )
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
    use crate::tir::blocks::{LoopBreakKind, LoopRole, Terminator, TirBlock};
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::{TirValue, ValueId};

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
    fn lower_shift_ops_use_runtime_simple_ir_names() {
        let mut func = TirFunction::new(
            "shift_names".into(),
            vec![TirType::I64, TirType::I64],
            TirType::DynBox,
        );
        let shl = ValueId(func.next_value);
        func.next_value += 1;
        let shr = ValueId(func.next_value);
        func.next_value += 1;
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Shl,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![shl],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Shr,
            operands: vec![shl, ValueId(1)],
            results: vec![shr],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return { values: vec![shr] };

        let type_map = HashMap::from([
            (ValueId(0), TirType::I64),
            (ValueId(1), TirType::I64),
            (shl, TirType::DynBox),
            (shr, TirType::DynBox),
        ]);
        let ops = lower_to_simple_ir(&func, &type_map);
        assert!(ops.iter().any(|op| op.kind == "lshift"));
        assert!(ops.iter().any(|op| op.kind == "rshift"));
        assert!(!ops.iter().any(|op| op.kind == "shl"));
        assert!(!ops.iter().any(|op| op.kind == "shr"));
    }

    #[test]
    fn lower_import_with_operand_roundtrips_as_module_import() {
        let mut func = TirFunction::new("import_roundtrip".into(), vec![], TirType::DynBox);
        let name = ValueId(func.next_value);
        func.next_value += 1;
        let imported = ValueId(func.next_value);
        func.next_value += 1;
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstStr,
            operands: vec![],
            results: vec![name],
            attrs: {
                let mut attrs = AttrDict::new();
                attrs.insert("s_value".into(), AttrValue::Str("builtins".into()));
                attrs
            },
            source_span: None,
        });
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Import,
            operands: vec![name],
            results: vec![imported],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![imported],
        };

        let type_map = HashMap::from([(name, TirType::Str), (imported, TirType::DynBox)]);
        let ops = lower_to_simple_ir(&func, &type_map);
        let import_op = ops
            .iter()
            .find(|op| op.kind == "module_import")
            .expect("expected module_import op");
        assert_eq!(import_op.args.as_ref().map(Vec::len), Some(1));
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
            ret_op
                .args
                .as_ref()
                .and_then(|args| args.first())
                .map(String::as_str),
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
            is_extern: false,
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
    fn structured_if_skips_join_with_external_predecessor() {
        let mut func = TirFunction::new(
            "branch_with_shared_join".into(),
            vec![TirType::Bool, TirType::Bool],
            TirType::None,
        );

        let inner_if = func.fresh_block();
        let external_pred = func.fresh_block();
        let then_blk = func.fresh_block();
        let else_blk = func.fresh_block();
        let join_blk = func.fresh_block();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.terminator = Terminator::CondBranch {
            cond: ValueId(0),
            then_block: inner_if,
            then_args: vec![],
            else_block: external_pred,
            else_args: vec![],
        };

        func.blocks.insert(
            inner_if,
            TirBlock {
                id: inner_if,
                args: vec![],
                ops: vec![],
                terminator: Terminator::CondBranch {
                    cond: ValueId(1),
                    then_block: then_blk,
                    then_args: vec![],
                    else_block: else_blk,
                    else_args: vec![],
                },
            },
        );
        func.blocks.insert(
            external_pred,
            TirBlock {
                id: external_pred,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: join_blk,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            then_blk,
            TirBlock {
                id: then_blk,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: join_blk,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            else_blk,
            TirBlock {
                id: else_blk,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: join_blk,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            join_blk,
            TirBlock {
                id: join_blk,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let ops = lower_to_simple_ir(&func, &HashMap::new());
        assert!(
            validate_labels(&ops),
            "shared join labels must remain valid after lower_to_simple: {ops:?}"
        );
        assert!(
            !ops.iter()
                .any(|op| op.kind == "if" || op.kind == "else" || op.kind == "end_if"),
            "shared-join lowering must stay label-based instead of inlining to structured if/else: {ops:?}"
        );
        assert!(
            ops.iter().filter(|op| op.kind == "label").count() >= 4,
            "shared-join lowering must preserve explicit labels for the merge shape: {ops:?}"
        );
    }

    #[test]
    fn structured_if_skips_arm_with_external_predecessor() {
        let mut func = TirFunction::new(
            "branch_with_shared_then_arm".into(),
            vec![TirType::Bool, TirType::Bool],
            TirType::None,
        );

        let inner_if = func.fresh_block();
        let external_pred = func.fresh_block();
        let then_blk = func.fresh_block();
        let else_blk = func.fresh_block();
        let join_blk = func.fresh_block();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.terminator = Terminator::CondBranch {
            cond: ValueId(0),
            then_block: inner_if,
            then_args: vec![],
            else_block: external_pred,
            else_args: vec![],
        };

        func.blocks.insert(
            inner_if,
            TirBlock {
                id: inner_if,
                args: vec![],
                ops: vec![],
                terminator: Terminator::CondBranch {
                    cond: ValueId(1),
                    then_block: then_blk,
                    then_args: vec![],
                    else_block: else_blk,
                    else_args: vec![],
                },
            },
        );
        func.blocks.insert(
            external_pred,
            TirBlock {
                id: external_pred,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: then_blk,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            then_blk,
            TirBlock {
                id: then_blk,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: join_blk,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            else_blk,
            TirBlock {
                id: else_blk,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: join_blk,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            join_blk,
            TirBlock {
                id: join_blk,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let ops = lower_to_simple_ir(&func, &HashMap::new());
        assert!(
            validate_labels(&ops),
            "shared-arm lowering must remain label-valid after lower_to_simple: {ops:?}"
        );
        assert!(
            !ops.iter()
                .any(|op| op.kind == "if" || op.kind == "else" || op.kind == "end_if"),
            "shared-arm lowering must stay label-based instead of inlining to structured if/else: {ops:?}"
        );
        assert!(
            ops.iter().filter(|op| op.kind == "label").count() >= 4,
            "shared-arm lowering must preserve explicit labels for the reused then-arm shape: {ops:?}"
        );
    }

    #[test]
    fn structured_if_emits_join_arg_store_load_without_phi() {
        let mut func = TirFunction::new(
            "branch_with_join_arg".into(),
            vec![TirType::Bool],
            TirType::I64,
        );

        let then_blk = func.fresh_block();
        let else_blk = func.fresh_block();
        let join_blk = func.fresh_block();
        let then_val = func.fresh_value();
        let else_val = func.fresh_value();
        let join_arg = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.terminator = Terminator::CondBranch {
            cond: ValueId(0),
            then_block: then_blk,
            then_args: vec![],
            else_block: else_blk,
            else_args: vec![],
        };

        let mut then_attrs = AttrDict::new();
        then_attrs.insert("value".into(), AttrValue::Int(1));
        func.blocks.insert(
            then_blk,
            TirBlock {
                id: then_blk,
                args: vec![],
                ops: vec![TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstInt,
                    operands: vec![],
                    results: vec![then_val],
                    attrs: then_attrs,
                    source_span: None,
                }],
                terminator: Terminator::Branch {
                    target: join_blk,
                    args: vec![then_val],
                },
            },
        );

        let mut else_attrs = AttrDict::new();
        else_attrs.insert("value".into(), AttrValue::Int(2));
        func.blocks.insert(
            else_blk,
            TirBlock {
                id: else_blk,
                args: vec![],
                ops: vec![TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstInt,
                    operands: vec![],
                    results: vec![else_val],
                    attrs: else_attrs,
                    source_span: None,
                }],
                terminator: Terminator::Branch {
                    target: join_blk,
                    args: vec![else_val],
                },
            },
        );

        func.blocks.insert(
            join_blk,
            TirBlock {
                id: join_blk,
                args: vec![TirValue {
                    id: join_arg,
                    ty: TirType::DynBox,
                }],
                ops: vec![],
                terminator: Terminator::Return {
                    values: vec![join_arg],
                },
            },
        );

        let ops = lower_to_simple_ir(&func, &HashMap::new());
        let kinds: Vec<&str> = ops.iter().map(|op| op.kind.as_str()).collect();

        assert!(kinds.contains(&"if"), "{ops:?}");
        assert!(kinds.contains(&"else"), "{ops:?}");
        assert!(kinds.contains(&"end_if"), "{ops:?}");
        assert!(
            !kinds.contains(&"phi"),
            "structured if join args must round-trip as store/load, not phi: {ops:?}"
        );
        assert!(
            ops.iter().filter(|op| op.kind == "store_var").count() >= 2,
            "structured if join args must emit branch-site stores: {ops:?}"
        );
        assert!(
            ops.iter().any(|op| op.kind == "load_var"),
            "structured if join args must reload the merged value after end_if: {ops:?}"
        );
    }

    #[test]
    fn check_exception_materializes_handler_arg_stores() {
        let mut func =
            TirFunction::new("check_exception_handler_args".into(), vec![], TirType::I64);

        let value = func.fresh_value();
        let exit_block = func.fresh_block();
        let handler_block = func.fresh_block();
        let handler_arg = func.fresh_value();

        let mut const_attrs = AttrDict::new();
        const_attrs.insert("value".into(), AttrValue::Int(7));
        let mut handler_attrs = AttrDict::new();
        handler_attrs.insert("value".into(), AttrValue::Int(100));

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![value],
            attrs: const_attrs,
            source_span: None,
        });
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::CheckException,
            operands: vec![value],
            results: vec![],
            attrs: handler_attrs,
            source_span: None,
        });
        entry.terminator = Terminator::Branch {
            target: exit_block,
            args: vec![],
        };

        func.blocks.insert(
            exit_block,
            TirBlock {
                id: exit_block,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        func.blocks.insert(
            handler_block,
            TirBlock {
                id: handler_block,
                args: vec![TirValue {
                    id: handler_arg,
                    ty: TirType::DynBox,
                }],
                ops: vec![],
                terminator: Terminator::Return {
                    values: vec![handler_arg],
                },
            },
        );

        func.has_exception_handling = true;
        func.label_id_map.insert(handler_block.0, 100);

        let ops = lower_to_simple_ir(&func, &HashMap::new());
        let handler_param = format!("_bb{}_arg0", handler_block.0);
        let handler_value = value_var(handler_arg);
        let entry_value = value_var(value);

        assert!(
            ops.iter().any(|op| {
                op.kind == "store_var"
                    && op.var.as_deref() == Some(handler_param.as_str())
                    && op
                        .args
                        .as_ref()
                        .is_some_and(|args| args == &vec![entry_value.clone()])
            }),
            "check_exception lowering must materialize handler arg stores before the handler label: {ops:?}"
        );
        assert!(
            ops.iter().any(|op| {
                op.kind == "load_var"
                    && op.var.as_deref() == Some(handler_param.as_str())
                    && op.out.as_deref() == Some(handler_value.as_str())
            }),
            "handler block must still reload its synthesized arg slot: {ops:?}"
        );
    }

    #[test]
    fn structured_if_skips_one_return_one_continue_shape() {
        let mut func = TirFunction::new(
            "branch_with_fallthrough_join".into(),
            vec![TirType::Bool],
            TirType::None,
        );

        let then_blk = func.fresh_block();
        let else_blk = func.fresh_block();
        let join_blk = func.fresh_block();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.terminator = Terminator::CondBranch {
            cond: ValueId(0),
            then_block: then_blk,
            then_args: vec![],
            else_block: else_blk,
            else_args: vec![],
        };

        func.blocks.insert(
            then_blk,
            TirBlock {
                id: then_blk,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.blocks.insert(
            else_blk,
            TirBlock {
                id: else_blk,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: join_blk,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            join_blk,
            TirBlock {
                id: join_blk,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let ops = lower_to_simple_ir(&func, &HashMap::new());
        assert!(
            validate_labels(&ops),
            "mixed return/fallthrough shape must keep valid labels after lower_to_simple: {ops:?}"
        );
        assert!(
            !ops.iter()
                .any(|op| op.kind == "if" || op.kind == "else" || op.kind == "end_if"),
            "mixed return/fallthrough shape must stay label-based until region analysis proves it safe: {ops:?}"
        );
    }

    #[test]
    fn structured_if_skips_successor_with_nested_scf() {
        let mut func = TirFunction::new(
            "branch_with_nested_scf_successor".into(),
            vec![TirType::Bool],
            TirType::None,
        );

        let then_blk = func.fresh_block();
        let else_blk = func.fresh_block();
        let join_blk = func.fresh_block();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.terminator = Terminator::CondBranch {
            cond: ValueId(0),
            then_block: then_blk,
            then_args: vec![],
            else_block: else_blk,
            else_args: vec![],
        };

        func.blocks.insert(
            then_blk,
            TirBlock {
                id: then_blk,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: join_blk,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            else_blk,
            TirBlock {
                id: else_blk,
                args: vec![],
                ops: vec![TirOp {
                    dialect: super::super::ops::Dialect::Scf,
                    opcode: OpCode::ScfWhile,
                    operands: vec![],
                    results: vec![],
                    attrs: HashMap::new(),
                    source_span: None,
                }],
                terminator: Terminator::Branch {
                    target: join_blk,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            join_blk,
            TirBlock {
                id: join_blk,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let ops = lower_to_simple_ir(&func, &HashMap::new());
        assert!(
            validate_labels(&ops),
            "nested-scf successor lowering must keep valid labels after lower_to_simple: {ops:?}"
        );
        assert!(
            !ops.iter()
                .any(|op| op.kind == "if" || op.kind == "else" || op.kind == "end_if"),
            "successors containing nested SCF must stay label-based instead of inlining to structured if/else: {ops:?}"
        );
    }

    #[test]
    fn structured_if_skips_successor_with_try_region_markers() {
        let mut func = TirFunction::new(
            "branch_with_try_region_successor".into(),
            vec![TirType::Bool],
            TirType::None,
        );

        let then_blk = func.fresh_block();
        let else_blk = func.fresh_block();
        let join_blk = func.fresh_block();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.terminator = Terminator::CondBranch {
            cond: ValueId(0),
            then_block: then_blk,
            then_args: vec![],
            else_block: else_blk,
            else_args: vec![],
        };

        let mut try_attrs = AttrDict::new();
        try_attrs.insert("value".into(), AttrValue::Int(100));
        func.blocks.insert(
            then_blk,
            TirBlock {
                id: then_blk,
                args: vec![],
                ops: vec![
                    TirOp {
                        dialect: Dialect::Molt,
                        opcode: OpCode::TryStart,
                        operands: vec![],
                        results: vec![],
                        attrs: try_attrs.clone(),
                        source_span: None,
                    },
                    TirOp {
                        dialect: Dialect::Molt,
                        opcode: OpCode::TryEnd,
                        operands: vec![],
                        results: vec![],
                        attrs: try_attrs,
                        source_span: None,
                    },
                ],
                terminator: Terminator::Branch {
                    target: join_blk,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            else_blk,
            TirBlock {
                id: else_blk,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: join_blk,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            join_blk,
            TirBlock {
                id: join_blk,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        let ops = lower_to_simple_ir(&func, &HashMap::new());
        assert!(
            validate_labels(&ops),
            "try-region successor lowering must keep valid labels after lower_to_simple: {ops:?}"
        );
        assert!(
            !ops.iter()
                .any(|op| op.kind == "if" || op.kind == "else" || op.kind == "end_if"),
            "successors containing try_start/try_end must stay label-based instead of inlining to structured if/else: {ops:?}"
        );
    }

    #[test]
    fn structured_if_skips_join_that_is_loop_header() {
        let mut func = TirFunction::new(
            "branch_with_loop_header_join".into(),
            vec![TirType::Bool],
            TirType::None,
        );

        let then_blk = func.fresh_block();
        let else_blk = func.fresh_block();
        let join_blk = func.fresh_block();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.terminator = Terminator::CondBranch {
            cond: ValueId(0),
            then_block: then_blk,
            then_args: vec![],
            else_block: else_blk,
            else_args: vec![],
        };

        func.blocks.insert(
            then_blk,
            TirBlock {
                id: then_blk,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: join_blk,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            else_blk,
            TirBlock {
                id: else_blk,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: join_blk,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            join_blk,
            TirBlock {
                id: join_blk,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.loop_roles
            .insert(join_blk, crate::tir::blocks::LoopRole::LoopHeader);

        let ops = lower_to_simple_ir(&func, &HashMap::new());
        assert!(
            validate_labels(&ops),
            "loop-header join lowering must keep valid labels after lower_to_simple: {ops:?}"
        );
        assert!(
            !ops.iter()
                .any(|op| op.kind == "if" || op.kind == "else" || op.kind == "end_if"),
            "join blocks that are loop headers must stay label-based instead of inlining to structured if/else: {ops:?}"
        );
    }

    #[test]
    fn loop_end_block_target_must_keep_its_label() {
        let mut func = TirFunction::new(
            "loop_end_block_target_must_keep_its_label".into(),
            vec![],
            TirType::None,
        );

        let target_block = func.fresh_block();
        let exit_block = func.fresh_block();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.terminator = Terminator::Branch {
            target: target_block,
            args: vec![],
        };

        func.blocks.insert(
            target_block,
            TirBlock {
                id: target_block,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: exit_block,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            exit_block,
            TirBlock {
                id: exit_block,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.loop_roles
            .insert(target_block, crate::tir::blocks::LoopRole::LoopEnd);

        let ops = lower_to_simple_ir(&func, &HashMap::new());
        assert!(
            validate_labels(&ops),
            "loop-end block labels must survive when explicit branches still target them: {ops:?}"
        );
        assert!(
            ops.iter()
                .any(|op| matches!(op.kind.as_str(), "label" | "state_label")
                    && op.value.is_some()),
            "expected a materialized target label for the loop-end block: {ops:?}"
        );
    }

    #[test]
    fn eliminate_dead_loop_end_after_return() {
        let mut ops = vec![
            OpIR {
                kind: "ret".into(),
                out: None,
                var: Some("_ret0".into()),
                args: Some(vec!["_ret0".into()]),
                value: None,
                f_value: None,
                s_value: None,
                bytes: None,
                fast_int: None,
                fast_float: None,
                stack_eligible: None,
                task_kind: None,
                container_type: None,
                type_hint: None,
                ic_index: None,
                col_offset: None,
                end_col_offset: None,
                bce_safe: None,
                arena_eligible: None,
            },
            OpIR {
                kind: "loop_end".into(),
                out: None,
                var: None,
                args: Some(vec![]),
                value: None,
                f_value: None,
                s_value: None,
                bytes: None,
                fast_int: None,
                fast_float: None,
                stack_eligible: None,
                task_kind: None,
                container_type: None,
                type_hint: None,
                ic_index: None,
                col_offset: None,
                end_col_offset: None,
                bce_safe: None,
                arena_eligible: None,
            },
            OpIR {
                kind: "label".into(),
                out: None,
                var: None,
                args: Some(vec![]),
                value: Some(42),
                f_value: None,
                s_value: None,
                bytes: None,
                fast_int: None,
                fast_float: None,
                stack_eligible: None,
                task_kind: None,
                container_type: None,
                type_hint: None,
                ic_index: None,
                col_offset: None,
                end_col_offset: None,
                bce_safe: None,
                arena_eligible: None,
            },
        ];

        eliminate_dead_labels(&mut ops);

        assert!(
            !ops.iter().any(|op| op.kind == "loop_end"),
            "dead loop_end must not survive after a real return: {ops:?}"
        );
    }

    #[test]
    fn eliminate_dead_jump_after_return() {
        let mut ops = vec![
            OpIR {
                kind: "ret".into(),
                out: None,
                var: Some("_ret0".into()),
                args: Some(vec!["_ret0".into()]),
                value: None,
                f_value: None,
                s_value: None,
                bytes: None,
                fast_int: None,
                fast_float: None,
                stack_eligible: None,
                task_kind: None,
                container_type: None,
                type_hint: None,
                ic_index: None,
                col_offset: None,
                end_col_offset: None,
                bce_safe: None,
                arena_eligible: None,
            },
            OpIR {
                kind: "jump".into(),
                out: None,
                var: None,
                args: None,
                value: Some(42),
                f_value: None,
                s_value: None,
                bytes: None,
                fast_int: None,
                fast_float: None,
                stack_eligible: None,
                task_kind: None,
                container_type: None,
                type_hint: None,
                ic_index: None,
                col_offset: None,
                end_col_offset: None,
                bce_safe: None,
                arena_eligible: None,
            },
            OpIR {
                kind: "label".into(),
                out: None,
                var: None,
                args: Some(vec![]),
                value: Some(42),
                f_value: None,
                s_value: None,
                bytes: None,
                fast_int: None,
                fast_float: None,
                stack_eligible: None,
                task_kind: None,
                container_type: None,
                type_hint: None,
                ic_index: None,
                col_offset: None,
                end_col_offset: None,
                bce_safe: None,
                arena_eligible: None,
            },
        ];

        eliminate_dead_labels(&mut ops);

        assert!(
            !ops.iter().any(|op| op.kind == "jump"),
            "dead jump must not survive after a real return: {ops:?}"
        );
    }

    #[test]
    fn preserve_loop_end_after_live_labeled_raise_path() {
        let mut ops = vec![
            OpIR {
                kind: "br_if".into(),
                args: Some(vec!["cond".into()]),
                value: Some(7),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_continue".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "label".into(),
                value: Some(7),
                args: Some(vec![]),
                ..OpIR::default()
            },
            OpIR {
                kind: "raise".into(),
                args: Some(vec!["exc".into()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_end".into(),
                args: Some(vec![]),
                ..OpIR::default()
            },
        ];

        eliminate_dead_labels(&mut ops);

        assert!(
            ops.iter().any(|op| op.kind == "loop_end"),
            "loop_end must survive after a live labeled terminal block because it still closes the structured loop break path: {ops:?}"
        );
    }

    #[test]
    fn eliminate_dead_labels_keeps_if_marker_after_dead_label_before_structured_if() {
        let mut ops = vec![
            OpIR {
                kind: "ret".into(),
                args: Some(vec!["_ret0".into()]),
                var: Some("_ret0".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "label".into(),
                value: Some(42),
                args: Some(vec![]),
                ..OpIR::default()
            },
            OpIR {
                kind: "if".into(),
                args: Some(vec!["cond".into()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_none".into(),
                out: Some("_v0".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "else".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "raise".into(),
                args: Some(vec!["exc".into()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "end_if".into(),
                ..OpIR::default()
            },
        ];

        eliminate_dead_labels(&mut ops);

        let kinds: Vec<&str> = ops.iter().map(|op| op.kind.as_str()).collect();
        assert_eq!(
            kinds,
            vec!["ret", "if", "const_none", "else", "raise", "end_if"],
            "dead-label elimination must not orphan structured if markers: {ops:?}"
        );
        assert!(
            validate_structured_if_markers(&ops).is_ok(),
            "structured if markers must remain balanced after dead-label elimination: {ops:?}"
        );
    }

    #[test]
    fn validate_structured_if_markers_rejects_orphan_else() {
        let ops = vec![
            OpIR {
                kind: "ret".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "else".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "end_if".into(),
                ..OpIR::default()
            },
        ];

        let err = validate_structured_if_markers(&ops).expect_err("must reject orphan else");
        assert!(err.contains("orphan else"), "{err}");
    }

    #[test]
    fn value_var_naming() {
        assert_eq!(value_var(ValueId(0)), "_v0");
        assert_eq!(value_var(ValueId(42)), "_v42");
    }

    /// Verify that typed TIR does not re-emit integer transport hints.
    #[test]
    fn type_propagation_does_not_emit_fast_int_on_arithmetic() {
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

        let ops = lower_to_simple_ir(&func, &type_map);
        let add_ops: Vec<&OpIR> = ops.iter().filter(|o| o.kind == "add").collect();
        assert!(!add_ops.is_empty(), "expected an 'add' op in output");
        for add_op in &add_ops {
            assert!(
                add_op.fast_int.is_none(),
                "typed TIR must not emit fast_int transport hints: {:?}",
                add_op
            );
        }
    }

    /// Verify that typed TIR does not re-emit float transport hints.
    #[test]
    fn type_propagation_does_not_emit_fast_float_on_float_arithmetic() {
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
            assert!(
                add_op.fast_float.is_none(),
                "typed TIR must not emit fast_float transport hints: {:?}",
                add_op
            );
        }
    }

    /// Verify that typed TIR does not re-emit bool type hints.
    #[test]
    fn type_propagation_does_not_emit_type_hint_for_bool() {
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
            assert!(
                eq_op.type_hint.is_none(),
                "typed TIR must not emit bool type_hint metadata: {:?}",
                eq_op
            );
            assert!(
                eq_op.fast_float.is_none(),
                "bool op should not have fast_float"
            );
            assert!(
                eq_op.fast_int.is_none(),
                "comparison op should not carry fast_int metadata"
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
            is_extern: false,
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
            is_extern: false,
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

    #[test]
    fn tir_round_trip_preserves_method_guarded_field_set_sequence() {
        use crate::ir::{FunctionIR, OpIR};
        use crate::tir::lower_from_simple::lower_to_tir;
        use crate::tir::passes::run_pipeline;
        use crate::tir::type_refine::{extract_type_map, refine_types};

        let func_ir = FunctionIR {
            name: "method_trace__C_f".into(),
            params: vec!["self".into()],
            ops: vec![
                OpIR {
                    kind: "exception_stack_enter".into(),
                    out: Some("v88".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_stack_depth".into(),
                    out: Some("v89".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store_var".into(),
                    var: Some("self".into()),
                    args: Some(vec!["self".into()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "line".into(),
                    value: Some(3),
                    col_offset: Some(8),
                    end_col_offset: Some(18),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".into(),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".into(),
                    value: Some(1),
                    out: Some("v90".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".into(),
                    s_value: Some("C".into()),
                    out: Some("v91".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".into(),
                    s_value: Some("method_trace".into()),
                    out: Some("v92".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_cache_get".into(),
                    args: Some(vec!["v92".into()]),
                    out: Some("v93".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".into(),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_get_attr".into(),
                    args: Some(vec!["v93".into(), "v91".into()]),
                    out: Some("v94".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".into(),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".into(),
                    value: Some(3),
                    out: Some("v95".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "guarded_field_set".into(),
                    args: Some(vec![
                        "self".into(),
                        "v94".into(),
                        "v95".into(),
                        "v90".into(),
                    ]),
                    s_value: Some("x".into()),
                    value: Some(0),
                    out: Some("none".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".into(),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".into(),
                    value: Some(0),
                    out: Some("v96".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".into(),
                    var: Some("v96".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "label".into(),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_stack_set_depth".into(),
                    args: Some(vec!["v89".into()]),
                    out: Some("none".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_stack_exit".into(),
                    args: Some(vec!["v88".into()]),
                    out: Some("none".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".into(),
                    ..OpIR::default()
                },
            ],
            param_types: Some(vec!["i64".into()]),
            source_file: None,
            is_extern: false,
        };

        let mut tir_func = lower_to_tir(&func_ir);
        refine_types(&mut tir_func);
        run_pipeline(&mut tir_func);
        refine_types(&mut tir_func);
        let type_map = extract_type_map(&tir_func);
        let round_tripped = lower_to_simple_ir(&tir_func, &type_map);

        let cache_get_idx = round_tripped
            .iter()
            .position(|op| op.kind == "module_cache_get")
            .expect("module_cache_get must survive TIR roundtrip");
        let module_get_idx = round_tripped
            .iter()
            .position(|op| op.kind == "module_get_attr")
            .expect("module_get_attr must survive TIR roundtrip");
        let field_set_idx = round_tripped
            .iter()
            .position(|op| op.kind == "guarded_field_set")
            .expect("guarded_field_set must survive TIR roundtrip");

        assert!(
            cache_get_idx < module_get_idx && module_get_idx < field_set_idx,
            "method guarded field set path must preserve class lookup ordering: {round_tripped:?}"
        );

        let producer_by_out: std::collections::HashMap<String, &OpIR> = round_tripped
            .iter()
            .filter_map(|op| op.out.as_ref().map(|out| (out.clone(), op)))
            .collect();

        let cache_get = &round_tripped[cache_get_idx];
        let cache_arg = cache_get
            .args
            .as_ref()
            .and_then(|args| args.first())
            .expect("module_cache_get must keep module-name operand");
        // Follow through Copy/copy chains to find the original const_str
        // (GVN may deduplicate identical constants, replacing the second
        // with a copy of the first).
        let mut cache_arg_name = cache_arg.clone();
        for _ in 0..10 {
            let op = producer_by_out
                .get(&cache_arg_name)
                .expect("module_cache_get operand must come from an op");
            if op.kind == "const_str" {
                assert_eq!(op.s_value.as_deref(), Some("method_trace"));
                break;
            }
            if op.kind == "copy" || op.kind == "copy_var" {
                cache_arg_name = op
                    .args
                    .as_ref()
                    .and_then(|a| a.first().cloned())
                    .unwrap_or_else(|| cache_arg_name.clone());
            } else {
                panic!(
                    "expected const_str or copy, got {} for module_cache_get operand",
                    op.kind
                );
            }
        }

        let class_lookup = &round_tripped[module_get_idx];
        let class_lookup_args = class_lookup
            .args
            .as_ref()
            .expect("module_get_attr must keep operands");
        assert_eq!(class_lookup_args.len(), 2);
        assert_eq!(class_lookup_args[0], cache_get.out.clone().unwrap());
        let class_name_op = producer_by_out
            .get(&class_lookup_args[1])
            .expect("module_get_attr class-name operand must come from an op");
        assert_eq!(class_name_op.kind, "const_str");
        assert_eq!(class_name_op.s_value.as_deref(), Some("C"));

        let field_set = &round_tripped[field_set_idx];
        let field_set_args = field_set
            .args
            .as_ref()
            .expect("guarded_field_set must keep operands");
        assert_eq!(field_set_args.len(), 4);
        let self_value_op = producer_by_out
            .get(&field_set_args[0])
            .expect("guarded_field_set receiver must come from an op");
        assert_eq!(self_value_op.kind, "copy_var");
        assert_eq!(self_value_op.var.as_deref(), Some("self"));
        assert_eq!(field_set_args[1], class_lookup.out.clone().unwrap());
        let expected_version_op = producer_by_out
            .get(&field_set_args[2])
            .expect("guarded_field_set version operand must come from an op");
        assert_eq!(expected_version_op.kind, "const");
        assert_eq!(expected_version_op.value, Some(3));
        let stored_value_op = producer_by_out
            .get(&field_set_args[3])
            .expect("guarded_field_set value operand must come from an op");
        assert_eq!(stored_value_op.kind, "const");
        assert_eq!(stored_value_op.value, Some(1));
        assert_eq!(field_set.s_value.as_deref(), Some("x"));
        assert_eq!(field_set.value, Some(0));

        let set_depth_idx = round_tripped
            .iter()
            .position(|op| op.kind == "exception_stack_set_depth")
            .expect("handler cleanup must preserve exception_stack_set_depth");
        let exit_idx = round_tripped
            .iter()
            .position(|op| op.kind == "exception_stack_exit")
            .expect("handler cleanup must preserve exception_stack_exit");
        let set_depth_arg = round_tripped[set_depth_idx]
            .args
            .as_ref()
            .and_then(|args| args.first())
            .expect("exception_stack_set_depth must keep its operand");
        let exit_arg = round_tripped[exit_idx]
            .args
            .as_ref()
            .and_then(|args| args.first())
            .expect("exception_stack_exit must keep its operand");
        let set_depth_arg_op = producer_by_out
            .get(set_depth_arg)
            .expect("exception_stack_set_depth operand must come from a load_var");
        let exit_arg_op = producer_by_out
            .get(exit_arg)
            .expect("exception_stack_exit operand must come from a load_var");
        assert_eq!(set_depth_arg_op.kind, "load_var");
        assert_eq!(exit_arg_op.kind, "load_var");
    }

    #[test]
    fn tir_round_trip_preserves_object_argument_call_sequence() {
        use crate::ir::{FunctionIR, OpIR};
        use crate::tir::lower_from_simple::lower_to_tir;
        use crate::tir::passes::run_pipeline;
        use crate::tir::type_refine::{extract_type_map, refine_types};

        let callee_ir = FunctionIR {
            name: "func_objarg__g".into(),
            params: vec!["x".into()],
            ops: vec![
                OpIR {
                    kind: "store_var".into(),
                    var: Some("x".into()),
                    args: Some(vec!["x".into()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "line".into(),
                    value: Some(5),
                    col_offset: Some(4),
                    end_col_offset: Some(18),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "type_of".into(),
                    args: Some(vec!["x".into()]),
                    out: Some("v99".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "print".into(),
                    args: Some(vec!["v99".into()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_none".into(),
                    out: Some("v100".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".into(),
                    var: Some("v100".into()),
                    ..OpIR::default()
                },
            ],
            param_types: Some(vec!["i64".into()]),
            source_file: None,
            is_extern: false,
        };

        let caller_ir = FunctionIR {
            name: "func_objarg__molt_module_chunk_1".into(),
            params: vec!["__molt_module_obj__".into()],
            ops: vec![
                OpIR {
                    kind: "line".into(),
                    value: Some(1),
                    col_offset: Some(0),
                    end_col_offset: Some(8),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".into(),
                    value: Some(100),
                    out: Some("v63".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "builtin_type".into(),
                    args: Some(vec!["v63".into()]),
                    out: Some("v64".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".into(),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".into(),
                    s_value: Some("C".into()),
                    out: Some("v65".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".into(),
                    s_value: Some("C".into()),
                    out: Some("v66".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".into(),
                    s_value: Some("__main__".into()),
                    out: Some("v67".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".into(),
                    value: Some(1),
                    out: Some("v68".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".into(),
                    s_value: Some("__name__".into()),
                    out: Some("v69".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".into(),
                    s_value: Some("__qualname__".into()),
                    out: Some("v70".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".into(),
                    s_value: Some("__module__".into()),
                    out: Some("v71".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".into(),
                    s_value: Some("__firstlineno__".into()),
                    out: Some("v72".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "class_def".into(),
                    args: Some(vec![
                        "v65".into(),
                        "v64".into(),
                        "v69".into(),
                        "v65".into(),
                        "v70".into(),
                        "v66".into(),
                        "v71".into(),
                        "v67".into(),
                        "v72".into(),
                        "v68".into(),
                    ]),
                    s_value: Some("1,4,8,1,1".into()),
                    out: Some("v73".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".into(),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".into(),
                    s_value: Some("C".into()),
                    out: Some("v74".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_set_attr".into(),
                    args: Some(vec![
                        "__molt_module_obj__".into(),
                        "v74".into(),
                        "v73".into(),
                    ]),
                    out: Some("none".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "line".into(),
                    value: Some(4),
                    col_offset: Some(0),
                    end_col_offset: Some(18),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".into(),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "func_new".into(),
                    s_value: Some("func_objarg__g".into()),
                    value: Some(1),
                    out: Some("v75".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".into(),
                    s_value: Some("g".into()),
                    out: Some("v76".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "set_attr_generic_obj".into(),
                    args: Some(vec!["v75".into(), "v76".into()]),
                    s_value: Some("__name__".into()),
                    out: Some("none".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".into(),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".into(),
                    s_value: Some("g".into()),
                    out: Some("v77".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "set_attr_generic_obj".into(),
                    args: Some(vec!["v75".into(), "v77".into()]),
                    s_value: Some("__qualname__".into()),
                    out: Some("none".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".into(),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".into(),
                    s_value: Some("func_objarg".into()),
                    out: Some("v78".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "set_attr_generic_obj".into(),
                    args: Some(vec!["v75".into(), "v78".into()]),
                    s_value: Some("__module__".into()),
                    out: Some("none".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".into(),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".into(),
                    s_value: Some("x".into()),
                    out: Some("v79".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "tuple_new".into(),
                    args: Some(vec!["v79".into()]),
                    out: Some("v80".into()),
                    type_hint: Some("tuple".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "set_attr_generic_obj".into(),
                    args: Some(vec!["v75".into(), "v80".into()]),
                    s_value: Some("__molt_arg_names__".into()),
                    out: Some("none".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".into(),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".into(),
                    value: Some(0),
                    out: Some("v81".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "set_attr_generic_obj".into(),
                    args: Some(vec!["v75".into(), "v81".into()]),
                    s_value: Some("__molt_posonly__".into()),
                    out: Some("none".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".into(),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "tuple_new".into(),
                    args: Some(vec![]),
                    out: Some("v82".into()),
                    type_hint: Some("tuple".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "set_attr_generic_obj".into(),
                    args: Some(vec!["v75".into(), "v82".into()]),
                    s_value: Some("__molt_kwonly_names__".into()),
                    out: Some("none".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".into(),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_none".into(),
                    out: Some("v83".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "set_attr_generic_obj".into(),
                    args: Some(vec!["v75".into(), "v83".into()]),
                    s_value: Some("__molt_vararg__".into()),
                    out: Some("none".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".into(),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "set_attr_generic_obj".into(),
                    args: Some(vec!["v75".into(), "v83".into()]),
                    s_value: Some("__molt_varkw__".into()),
                    out: Some("none".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".into(),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "set_attr_generic_obj".into(),
                    args: Some(vec!["v75".into(), "v83".into()]),
                    s_value: Some("__defaults__".into()),
                    out: Some("none".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".into(),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "set_attr_generic_obj".into(),
                    args: Some(vec!["v75".into(), "v83".into()]),
                    s_value: Some("__kwdefaults__".into()),
                    out: Some("none".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".into(),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "set_attr_generic_obj".into(),
                    args: Some(vec!["v75".into(), "v83".into()]),
                    s_value: Some("__doc__".into()),
                    out: Some("none".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".into(),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".into(),
                    s_value: Some("/tmp/func_objarg.py".into()),
                    out: Some("v88".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".into(),
                    value: Some(4),
                    out: Some("v89".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".into(),
                    s_value: Some("g".into()),
                    out: Some("v90".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".into(),
                    s_value: Some("x".into()),
                    out: Some("v92".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "tuple_new".into(),
                    args: Some(vec!["v92".into()]),
                    out: Some("v93".into()),
                    type_hint: Some("tuple".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "code_new".into(),
                    args: Some(vec![
                        "v88".into(),
                        "v90".into(),
                        "v89".into(),
                        "v83".into(),
                        "v93".into(),
                        "v68".into(),
                        "v81".into(),
                        "v81".into(),
                    ]),
                    out: Some("v97".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "set_attr_generic_obj".into(),
                    args: Some(vec!["v75".into(), "v97".into()]),
                    s_value: Some("__code__".into()),
                    out: Some("none".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".into(),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "code_slot_set".into(),
                    value: Some(0),
                    args: Some(vec!["v97".into()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".into(),
                    s_value: Some("g".into()),
                    out: Some("v98".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_set_attr".into(),
                    args: Some(vec![
                        "__molt_module_obj__".into(),
                        "v98".into(),
                        "v75".into(),
                    ]),
                    out: Some("none".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "line".into(),
                    value: Some(7),
                    col_offset: Some(0),
                    end_col_offset: Some(7),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".into(),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".into(),
                    s_value: Some("C".into()),
                    out: Some("v101".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_get_attr".into(),
                    args: Some(vec!["__molt_module_obj__".into(), "v101".into()]),
                    out: Some("v102".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".into(),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "callargs_new".into(),
                    out: Some("v103".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "call_bind".into(),
                    args: Some(vec!["v102".into(), "v103".into()]),
                    out: Some("v104".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".into(),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".into(),
                    s_value: Some("c".into()),
                    out: Some("v105".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_set_attr".into(),
                    args: Some(vec![
                        "__molt_module_obj__".into(),
                        "v105".into(),
                        "v104".into(),
                    ]),
                    out: Some("none".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "line".into(),
                    value: Some(8),
                    col_offset: Some(0),
                    end_col_offset: Some(4),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".into(),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "call".into(),
                    s_value: Some("func_objarg__g".into()),
                    args: Some(vec!["v104".into()]),
                    value: Some(0),
                    out: Some("v106".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".into(),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "label".into(),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_last".into(),
                    out: Some("v107".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_none".into(),
                    out: Some("v108".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_none".into(),
                    out: Some("v108".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "is".into(),
                    args: Some(vec!["v107".into(), "v108".into()]),
                    out: Some("v109".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "not".into(),
                    args: Some(vec!["v109".into()]),
                    out: Some("v110".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "if".into(),
                    args: Some(vec!["v110".into()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".into(),
                    s_value: Some("func_objarg".into()),
                    out: Some("v111".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_cache_del".into(),
                    args: Some(vec!["v111".into()]),
                    out: Some("none".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".into(),
                    s_value: Some("__main__".into()),
                    out: Some("v112".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_cache_del".into(),
                    args: Some(vec!["v112".into()]),
                    out: Some("none".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "end_if".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".into(),
                    ..OpIR::default()
                },
            ],
            param_types: Some(vec!["i64".into()]),
            source_file: None,
            is_extern: false,
        };

        for func_ir in [callee_ir, caller_ir] {
            let mut tir_func = lower_to_tir(&func_ir);
            refine_types(&mut tir_func);
            run_pipeline(&mut tir_func);
            refine_types(&mut tir_func);
            let type_map = extract_type_map(&tir_func);
            let round_tripped = lower_to_simple_ir(&tir_func, &type_map);

            for op in &round_tripped {
                assert!(
                    op.fast_float.is_none(),
                    "roundtrip must not mark object-arg call path as fast_float: {round_tripped:?}"
                );
            }

            if func_ir.name == "func_objarg__g" {
                let type_of = round_tripped
                    .iter()
                    .find(|op| op.kind == "type_of")
                    .expect("callee must preserve type_of");
                assert_eq!(type_of.args.as_ref().map(Vec::len), Some(1));
                let arg_name = type_of.args.as_ref().unwrap()[0].clone();
                let producer_by_out: std::collections::HashMap<String, &OpIR> = round_tripped
                    .iter()
                    .filter_map(|op| op.out.as_ref().map(|out| (out.clone(), op)))
                    .collect();
                let arg_op = producer_by_out
                    .get(&arg_name)
                    .expect("type_of operand must come from a copy_var");
                assert_eq!(arg_op.kind, "copy_var");
                assert_eq!(arg_op.var.as_deref(), Some("x"));
            } else {
                let producer_by_out: std::collections::HashMap<String, &OpIR> = round_tripped
                    .iter()
                    .filter_map(|op| op.out.as_ref().map(|out| (out.clone(), op)))
                    .collect();
                let call = round_tripped
                    .iter()
                    .find(|op| op.kind == "call" && op.s_value.as_deref() == Some("func_objarg__g"))
                    .expect("caller must preserve direct call to func_objarg__g");
                let call_args = call
                    .args
                    .as_ref()
                    .expect("direct call must keep its argument");
                assert_eq!(call_args.len(), 1);
                let arg_op = producer_by_out
                    .get(&call_args[0])
                    .expect("direct call argument must come from an op");
                assert_eq!(arg_op.kind, "call_bind");
                assert_eq!(arg_op.s_value, None);
            }
        }
    }

    /// Regression test: counted loops are normalized into loop-carried
    /// store_var/load_var form, and control flow must not re-enter above the
    /// first carrier load after loop_start.
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
            is_extern: false,
        };

        let tir_func = lower_to_tir(&func_ir);
        let round_tripped = lower_to_simple_ir(&tir_func, &HashMap::new());

        let loop_start_idx = round_tripped
            .iter()
            .position(|op| op.kind == "loop_start")
            .expect("expected loop_start after round-trip");
        let carrier_load_idx = round_tripped
            .iter()
            .position(|op| op.kind == "load_var")
            .expect("expected loop-carried load_var after round-trip");
        assert!(
            round_tripped[loop_start_idx + 1..carrier_load_idx]
                .iter()
                .all(|op| op.kind != "label" && op.kind != "jump" && op.kind != "br_if"),
            "counted loop must not place control-flow re-entry before the carrier load; ops: {:?}",
            round_tripped
                .iter()
                .map(|op| op.kind.as_str())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn structured_if_must_not_inline_exception_handler_target_blocks() {
        let mut func = TirFunction::new("eh_handler_if".into(), vec![TirType::Bool], TirType::I64);

        let handler_block = func.fresh_block();
        let else_block = func.fresh_block();
        let handler_value = func.fresh_value();
        let else_value = func.fresh_value();

        let mut handler_attrs = AttrDict::new();
        handler_attrs.insert("value".into(), AttrValue::Int(7));
        let mut else_attrs = AttrDict::new();
        else_attrs.insert("value".into(), AttrValue::Int(9));

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        let mut check_exc_attrs = AttrDict::new();
        check_exc_attrs.insert("value".into(), AttrValue::Int(100));
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::CheckException,
            operands: vec![],
            results: vec![],
            attrs: check_exc_attrs,
            source_span: None,
        });
        entry.terminator = Terminator::CondBranch {
            cond: ValueId(0),
            then_block: handler_block,
            then_args: vec![],
            else_block,
            else_args: vec![],
        };

        func.blocks.insert(
            handler_block,
            TirBlock {
                id: handler_block,
                args: vec![],
                ops: vec![TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstInt,
                    operands: vec![],
                    results: vec![handler_value],
                    attrs: handler_attrs,
                    source_span: None,
                }],
                terminator: Terminator::Return {
                    values: vec![handler_value],
                },
            },
        );
        func.blocks.insert(
            else_block,
            TirBlock {
                id: else_block,
                args: vec![],
                ops: vec![TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstInt,
                    operands: vec![],
                    results: vec![else_value],
                    attrs: else_attrs,
                    source_span: None,
                }],
                terminator: Terminator::Return {
                    values: vec![else_value],
                },
            },
        );
        func.label_id_map.insert(handler_block.0, 100);

        let ops = lower_to_simple_ir(&func, &HashMap::new());

        assert!(
            validate_labels(&ops),
            "exception handler labels must survive lowering: {ops:?}"
        );
        assert!(
            ops.iter()
                .any(|op| matches!(op.kind.as_str(), "label" | "state_label")
                    && op.value == Some(100)),
            "handler target label 100 must remain materialized: {ops:?}"
        );
    }

    #[test]
    fn emit_guard_raise_path_keeps_cleanup_blocks_after_raise() {
        let mut func = TirFunction::new(
            "emit_guard_raise_path_keeps_cleanup_blocks_after_raise".into(),
            vec![],
            TirType::I64,
        );
        let raise_block = func.fresh_block();
        let cleanup_block = func.fresh_block();
        let raise_value = func.fresh_value();
        let cleanup_value = func.fresh_value();

        let mut raise_attrs = AttrDict::new();
        raise_attrs.insert("value".into(), AttrValue::Int(7));
        func.blocks.insert(
            raise_block,
            TirBlock {
                id: raise_block,
                args: vec![],
                ops: vec![
                    TirOp {
                        dialect: Dialect::Molt,
                        opcode: OpCode::ConstInt,
                        operands: vec![],
                        results: vec![raise_value],
                        attrs: raise_attrs,
                        source_span: None,
                    },
                    TirOp {
                        dialect: Dialect::Molt,
                        opcode: OpCode::Raise,
                        operands: vec![raise_value],
                        results: vec![],
                        attrs: AttrDict::new(),
                        source_span: None,
                    },
                ],
                terminator: Terminator::Branch {
                    target: cleanup_block,
                    args: vec![],
                },
            },
        );

        let mut cleanup_attrs = AttrDict::new();
        cleanup_attrs.insert("value".into(), AttrValue::Int(2));
        func.blocks.insert(
            cleanup_block,
            TirBlock {
                id: cleanup_block,
                args: vec![],
                ops: vec![TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstInt,
                    operands: vec![],
                    results: vec![cleanup_value],
                    attrs: cleanup_attrs,
                    source_span: None,
                }],
                terminator: Terminator::Return {
                    values: vec![cleanup_value],
                },
            },
        );

        let block_param_vars =
            HashMap::from([(raise_block, Vec::new()), (cleanup_block, Vec::new())]);
        let mut out = Vec::new();
        let labels = HashMap::from([(raise_block, 99_i64), (cleanup_block, 100_i64)]);
        let original_label_to_block =
            HashMap::from([(99_i64, raise_block), (100_i64, cleanup_block)]);
        let block_label_id =
            |bid: &BlockId| -> i64 { *labels.get(bid).expect("missing test label") };

        emit_guard_raise_path(
            raise_block,
            &[],
            &HashSet::from([raise_block, cleanup_block]),
            &func,
            &block_param_vars,
            &block_label_id,
            &HashSet::new(),
            &HashMap::new(),
            &HashMap::new(),
            &original_label_to_block,
            &mut out,
        );

        assert!(
            validate_labels(&out),
            "guard raise path lowering must keep labels reachable after a raise block: {out:?}"
        );
        assert!(
            out.iter()
                .any(|op| matches!(op.kind.as_str(), "label" | "state_label")
                    && op.value == Some(100)),
            "cleanup label 100 must remain materialized after a raise-and-branch chain: {out:?}"
        );
    }

    #[test]
    fn explicit_loop_cond_block_is_not_reclassified_as_guard_when_exit_raises() {
        let mut func = TirFunction::new(
            "explicit_loop_cond_block_is_not_reclassified_as_guard_when_exit_raises".into(),
            vec![TirType::Bool, TirType::Bool],
            TirType::None,
        );
        let header = func.entry_block;
        let cond = func.fresh_block();
        let exit_raise = func.fresh_block();
        let body = func.fresh_block();
        let nested_cond = func.fresh_block();
        let nested_then = func.fresh_block();
        let nested_join = func.fresh_block();
        let cleanup = func.fresh_block();
        let raise_value = func.fresh_value();

        func.loop_roles.insert(header, LoopRole::LoopHeader);
        func.loop_break_kinds
            .insert(header, LoopBreakKind::BreakIfTrue);
        func.loop_cond_blocks.insert(header, cond);

        func.blocks.get_mut(&header).unwrap().terminator = Terminator::Branch {
            target: cond,
            args: vec![],
        };
        func.blocks.insert(
            cond,
            TirBlock {
                id: cond,
                args: vec![],
                ops: vec![],
                terminator: Terminator::CondBranch {
                    cond: ValueId(0),
                    then_block: exit_raise,
                    then_args: vec![],
                    else_block: body,
                    else_args: vec![],
                },
            },
        );
        func.blocks.insert(
            exit_raise,
            TirBlock {
                id: exit_raise,
                args: vec![],
                ops: vec![
                    TirOp {
                        dialect: Dialect::Molt,
                        opcode: OpCode::ConstInt,
                        operands: vec![],
                        results: vec![raise_value],
                        attrs: AttrDict::new(),
                        source_span: None,
                    },
                    TirOp {
                        dialect: Dialect::Molt,
                        opcode: OpCode::Raise,
                        operands: vec![raise_value],
                        results: vec![],
                        attrs: AttrDict::new(),
                        source_span: None,
                    },
                ],
                terminator: Terminator::Branch {
                    target: cleanup,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            body,
            TirBlock {
                id: body,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: nested_cond,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            nested_cond,
            TirBlock {
                id: nested_cond,
                args: vec![],
                ops: vec![],
                terminator: Terminator::CondBranch {
                    cond: ValueId(1),
                    then_block: nested_then,
                    then_args: vec![],
                    else_block: nested_join,
                    else_args: vec![],
                },
            },
        );
        func.blocks.insert(
            nested_then,
            TirBlock {
                id: nested_then,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: nested_join,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            nested_join,
            TirBlock {
                id: nested_join,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            cleanup,
            TirBlock {
                id: cleanup,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let ops = lower_to_simple_ir(&func, &HashMap::new());

        assert!(
            validate_labels(&ops),
            "explicit loop condition lowering must not leave dangling labels: {ops:?}"
        );
    }

    #[test]
    fn loop_guard_raise_chain_keeps_cleanup_handler_label() {
        let mut func = TirFunction::new(
            "loop_guard_raise_chain_keeps_cleanup_handler_label".into(),
            vec![TirType::Bool, TirType::Bool, TirType::Bool],
            TirType::I64,
        );

        let header = func.fresh_block();
        let guard = func.fresh_block();
        let cond_block = func.fresh_block();
        let raise_block = func.fresh_block();
        let body_block = func.fresh_block();
        let exit_block = func.fresh_block();
        let cleanup_block = func.fresh_block();
        let return_block = func.fresh_block();
        let continue_block = func.fresh_block();

        let raise_value = func.fresh_value();
        let exit_value = func.fresh_value();
        let cleanup_value = func.fresh_value();
        let return_value = func.fresh_value();

        let mut handler_attrs = AttrDict::new();
        handler_attrs.insert("value".into(), AttrValue::Int(100));

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::CheckException,
            operands: vec![],
            results: vec![],
            attrs: handler_attrs.clone(),
            source_span: None,
        });
        entry.terminator = Terminator::Branch {
            target: header,
            args: vec![],
        };

        func.blocks.insert(
            header,
            TirBlock {
                id: header,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: guard,
                    args: vec![],
                },
            },
        );
        func.blocks.insert(
            guard,
            TirBlock {
                id: guard,
                args: vec![],
                ops: vec![],
                terminator: Terminator::CondBranch {
                    cond: ValueId(0),
                    then_block: raise_block,
                    then_args: vec![],
                    else_block: cond_block,
                    else_args: vec![],
                },
            },
        );
        func.blocks.insert(
            cond_block,
            TirBlock {
                id: cond_block,
                args: vec![],
                ops: vec![],
                terminator: Terminator::CondBranch {
                    cond: ValueId(1),
                    then_block: body_block,
                    then_args: vec![],
                    else_block: exit_block,
                    else_args: vec![],
                },
            },
        );

        let mut raise_attrs = AttrDict::new();
        raise_attrs.insert("value".into(), AttrValue::Int(7));
        func.blocks.insert(
            raise_block,
            TirBlock {
                id: raise_block,
                args: vec![],
                ops: vec![
                    TirOp {
                        dialect: Dialect::Molt,
                        opcode: OpCode::ConstInt,
                        operands: vec![],
                        results: vec![raise_value],
                        attrs: raise_attrs,
                        source_span: None,
                    },
                    TirOp {
                        dialect: Dialect::Molt,
                        opcode: OpCode::Raise,
                        operands: vec![raise_value],
                        results: vec![],
                        attrs: AttrDict::new(),
                        source_span: None,
                    },
                ],
                terminator: Terminator::Branch {
                    target: cleanup_block,
                    args: vec![],
                },
            },
        );

        let mut exit_attrs = AttrDict::new();
        exit_attrs.insert("value".into(), AttrValue::Int(0));
        func.blocks.insert(
            exit_block,
            TirBlock {
                id: exit_block,
                args: vec![],
                ops: vec![TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstInt,
                    operands: vec![],
                    results: vec![exit_value],
                    attrs: exit_attrs,
                    source_span: None,
                }],
                terminator: Terminator::Return {
                    values: vec![exit_value],
                },
            },
        );

        func.blocks.insert(
            body_block,
            TirBlock {
                id: body_block,
                args: vec![],
                ops: vec![TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::CheckException,
                    operands: vec![],
                    results: vec![],
                    attrs: handler_attrs.clone(),
                    source_span: None,
                }],
                terminator: Terminator::CondBranch {
                    cond: ValueId(2),
                    then_block: return_block,
                    then_args: vec![],
                    else_block: continue_block,
                    else_args: vec![],
                },
            },
        );

        let mut return_attrs = AttrDict::new();
        return_attrs.insert("value".into(), AttrValue::Int(1));
        func.blocks.insert(
            return_block,
            TirBlock {
                id: return_block,
                args: vec![],
                ops: vec![TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstInt,
                    operands: vec![],
                    results: vec![return_value],
                    attrs: return_attrs,
                    source_span: None,
                }],
                terminator: Terminator::Return {
                    values: vec![return_value],
                },
            },
        );

        func.blocks.insert(
            continue_block,
            TirBlock {
                id: continue_block,
                args: vec![],
                ops: vec![TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::CheckException,
                    operands: vec![],
                    results: vec![],
                    attrs: handler_attrs.clone(),
                    source_span: None,
                }],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![],
                },
            },
        );

        let mut cleanup_attrs = AttrDict::new();
        cleanup_attrs.insert("value".into(), AttrValue::Int(2));
        func.blocks.insert(
            cleanup_block,
            TirBlock {
                id: cleanup_block,
                args: vec![],
                ops: vec![TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstInt,
                    operands: vec![],
                    results: vec![cleanup_value],
                    attrs: cleanup_attrs,
                    source_span: None,
                }],
                terminator: Terminator::Return {
                    values: vec![cleanup_value],
                },
            },
        );

        func.has_exception_handling = true;
        func.label_id_map.insert(cleanup_block.0, 100);
        func.loop_roles.insert(header, LoopRole::LoopHeader);
        func.loop_break_kinds
            .insert(header, LoopBreakKind::BreakIfFalse);
        func.loop_cond_blocks.insert(header, cond_block);

        let ops = lower_to_simple_ir(&func, &HashMap::new());

        assert!(
            validate_labels(&ops),
            "guard raise cleanup handler labels must survive structured loop lowering: {ops:?}"
        );
        assert!(
            ops.iter()
                .any(|op| op.kind == "check_exception" && op.value == Some(100)),
            "check_exception must keep targeting handler label 100: {ops:?}"
        );
        assert!(
            ops.iter()
                .any(|op| matches!(op.kind.as_str(), "label" | "state_label")
                    && op.value == Some(100)),
            "cleanup handler label 100 must remain materialized: {ops:?}"
        );
    }
}

#[cfg(test)]
mod not_roundtrip_tests {
    use crate::ir::{FunctionIR, OpIR};
    use crate::tir::lower_from_simple::lower_to_tir;

    fn op_const_bool(out: &str, val: bool) -> OpIR {
        OpIR {
            kind: "const_bool".to_string(),
            out: Some(out.to_string()),
            value: Some(if val { 1 } else { 0 }),
            ..OpIR::default()
        }
    }

    fn op_not(arg: &str, out: &str) -> OpIR {
        OpIR {
            kind: "not".to_string(),
            args: Some(vec![arg.to_string()]),
            out: Some(out.to_string()),
            ..OpIR::default()
        }
    }

    fn op_ret(arg: &str) -> OpIR {
        OpIR {
            kind: "ret".to_string(),
            args: Some(vec![arg.to_string()]),
            var: Some(arg.to_string()),
            ..OpIR::default()
        }
    }

    /// Regression: `__bool__` returning a literal `False`/`True` must round-trip
    /// through TIR with the const_bool value preserved as `AttrValue::Bool` (not
    /// `AttrValue::Int`).  When the SSA lift stored ConstBool as `AttrValue::Int`,
    /// downstream codegen at the function-return path silently TAG_INT-boxed
    /// the 0/1 value, producing a boxed int instead of a boxed bool.  The
    /// runtime's `as_bool()` predicate then rejected the value, raising
    /// `TypeError: __bool__ should return bool, returned int`.
    ///
    /// This test exercises the exact `__bool__`-method shape: `const_bool;
    /// ret`.  After the fix in commit 8662b45f and the matching
    /// `ensure_boxed_primitive_safe` bool-aware repath, the const_bool's
    /// `value` attribute must arrive at lower_to_simple_ir as
    /// `AttrValue::Bool(false)`/`AttrValue::Bool(true)` and the resulting
    /// const_bool OpIR must carry a 0/1 value field intact.
    #[test]
    fn bool_method_return_preserves_const_bool_value() {
        for (return_value, expected_int) in [(false, 0i64), (true, 1i64)] {
            let func = FunctionIR {
                name: "Falsy___bool__".to_string(),
                params: vec!["self".to_string()],
                ops: vec![op_const_bool("retv", return_value), op_ret("retv")],
                param_types: None,
                source_file: None,
                is_extern: false,
            };

            let tir = lower_to_tir(&func);
            // SSA lift must store ConstBool's value as AttrValue::Bool, not Int.
            let mut found_const_bool = false;
            for block in tir.blocks.values() {
                for op in &block.ops {
                    if op.opcode == crate::tir::ops::OpCode::ConstBool {
                        found_const_bool = true;
                        match op.attrs.get("value") {
                            Some(crate::tir::ops::AttrValue::Bool(b)) => {
                                assert_eq!(
                                    *b, return_value,
                                    "const_bool value attribute must match the literal"
                                );
                            }
                            other => panic!(
                                "const_bool value attribute must be AttrValue::Bool({return_value}), got {other:?}"
                            ),
                        }
                    }
                }
            }
            assert!(
                found_const_bool,
                "TIR must contain a const_bool op for the __bool__ return"
            );

            // Roundtrip: TIR → SimpleIR.
            let types = std::collections::HashMap::new();
            let roundtripped = super::lower_to_simple_ir(&tir, &types);

            // The roundtripped const_bool must carry value=0 for False, value=1
            // for True.  If the ssa lift stored AttrValue::Int(0) instead of
            // AttrValue::Bool(false), the downstream None branch would fall
            // through to value=Some(0) — masking the bug at this layer but
            // failing at the cranelift box site.  Asserting on the roundtripped
            // value pins the contract end-to-end.
            let const_bool_op = roundtripped
                .iter()
                .find(|op| op.kind == "const_bool")
                .expect("const_bool must survive roundtrip");
            assert_eq!(
                const_bool_op.value,
                Some(expected_int),
                "const_bool value field must be {expected_int} for return_value={return_value}"
            );

            // The ret op must reference the const_bool variable directly, not
            // an int copy or coerced value.
            let ret_op = roundtripped
                .iter()
                .find(|op| op.kind == "ret")
                .expect("ret op must survive roundtrip");
            let ret_args = ret_op.args.as_ref().expect("ret must have args");
            assert_eq!(ret_args.len(), 1, "ret must have exactly 1 arg");
            let const_bool_out = const_bool_op
                .out
                .as_ref()
                .expect("const_bool must have out var");
            assert_eq!(
                &ret_args[0], const_bool_out,
                "ret must consume the const_bool variable directly"
            );
        }
    }

    #[test]
    fn not_true_roundtrip_preserves_operand() {
        let func = FunctionIR {
            name: "test_not".to_string(),
            params: vec![],
            ops: vec![
                op_const_bool("x", true),
                op_not("x", "y"),
                op_ret("y"),
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        let tir = lower_to_tir(&func);
        // Roundtrip: TIR → SimpleIR
        let types = std::collections::HashMap::new();
        let roundtripped = super::lower_to_simple_ir(&tir, &types);

        // Find the "not" op
        let not_op = roundtripped.iter().find(|op| op.kind == "not");
        assert!(not_op.is_some(), "not op must survive roundtrip");

        let not_op = not_op.unwrap();
        let not_args = not_op.args.as_ref().expect("not must have args");
        assert_eq!(not_args.len(), 1, "not must have exactly 1 arg");

        // The arg must reference a variable that is defined by const_bool
        let arg_name = &not_args[0];
        let const_op = roundtripped
            .iter()
            .find(|op| op.kind == "const_bool" && op.out.as_deref() == Some(arg_name));
        assert!(
            const_op.is_some(),
            "not's operand '{}' must be defined by a const_bool op. ops: {:?}",
            arg_name,
            roundtripped
                .iter()
                .map(|op| format!("{} out={:?} args={:?}", op.kind, op.out, op.args))
                .collect::<Vec<_>>()
        );
    }
}
