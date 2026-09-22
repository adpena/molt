use std::collections::{HashMap, HashSet};

use crate::ir::OpIR;
use crate::tir::blocks::{BlockId, LoopBreakKind, LoopRole, Terminator, TirBlock};
use crate::tir::clone_support::LabelAllocator;
use crate::tir::dominators;
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    opcode_has_exception_label_attr_table, opcode_is_structured_scf_marker_table,
};
use crate::tir::ops::{AttrValue, OpCode};

use super::cfg::{collect_guard_raise_path_blocks, reverse_postorder, successor_reaches_header};
use super::cleanup::{
    eliminate_dead_labels, missing_label_references, validate_structured_if_markers,
};
use super::op_utils::attr_int;
use super::structured::{
    LoopRegion, emit_block_arg_loads, emit_block_arg_stores, emit_block_ops_inner, emit_return_ops,
    emit_structured_loop_region, emit_terminator,
};
use crate::tir::simple_value_names::{
    SimpleValueNames, reset_value_names, set_value_names, value_var,
};

pub fn lower_to_simple_ir(func: &TirFunction) -> Vec<OpIR> {
    let simple_value_names = SimpleValueNames::for_function(func);
    set_value_names(simple_value_names.clone());

    let mut out = Vec::new();

    let state_dispatch_targets_by_state: HashMap<i64, BlockId> = func
        .blocks
        .values()
        .flat_map(|block| match &block.terminator {
            Terminator::StateDispatch { cases, .. } => cases
                .iter()
                .map(|(state_id, target, _)| (*state_id, *target))
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .collect();
    let mut state_yield_resume_after: HashMap<BlockId, BlockId> = HashMap::new();
    let mut state_yield_resume_states: HashMap<BlockId, Option<i64>> = HashMap::new();
    for (bid, block) in &func.blocks {
        let Some(state_id) = block.ops.iter().find_map(|op| {
            (op.opcode == OpCode::StateYield)
                .then(|| attr_int(&op.attrs, "value"))
                .flatten()
        }) else {
            continue;
        };
        let Some(&resume_target) = state_dispatch_targets_by_state.get(&state_id) else {
            continue;
        };
        state_yield_resume_after.insert(*bid, resume_target);
        state_yield_resume_states
            .entry(resume_target)
            .and_modify(|slot| {
                if *slot != Some(state_id) {
                    *slot = None;
                }
            })
            .or_insert(Some(state_id));
    }
    let state_yield_resume_state_for_block: HashMap<BlockId, i64> = state_yield_resume_states
        .into_iter()
        .filter_map(|(bid, state)| state.map(|state| (bid, state)))
        .collect();

    // RC drop-insertion substrate (design 20): function-level attrs do NOT
    // round-trip through `FunctionIR`, so drop facts are carried as leading no-op
    // marker `OpIR`s. `drop_inserted` is the full-function RC authority marker
    // native preanalysis consumes to suppress legacy value tracking. The
    // exception-region marker is narrower: it preserves handler-safe
    // CreationRef/MatchRef releases for idempotent relifts and `refcount_elim`
    // protection, but native must ignore it as a full RC suppression signal.
    // Emitted before the body so every SimpleIR consumer sees the facts first.
    if matches!(
        func.attrs
            .get(crate::tir::passes::drop_insertion::DROP_INSERTED_ATTR),
        Some(AttrValue::Bool(true))
    ) {
        out.push(OpIR {
            kind: crate::tir::passes::drop_insertion::DROP_INSERTED_ATTR.to_string(),
            ..OpIR::default()
        });
    }
    if matches!(
        func.attrs
            .get(crate::tir::passes::drop_insertion::EXCEPTION_REGION_DROPS_INSERTED_ATTR),
        Some(AttrValue::Bool(true))
    ) {
        out.push(OpIR {
            kind: crate::tir::passes::drop_insertion::EXCEPTION_REGION_DROPS_INSERTED_ATTR
                .to_string(),
            ..OpIR::default()
        });
    }

    // Compute block visit order (reverse-postorder from entry).
    let rpo = reverse_postorder(func, &state_yield_resume_after);
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
        let reserved_state_ids: HashSet<i64> = state_yield_resume_state_for_block
            .values()
            .copied()
            .collect();
        let mut fresh_labels = LabelAllocator::after_labels(
            used_ids
                .iter()
                .copied()
                .chain(reserved_state_ids.iter().copied())
                .chain(func.blocks.keys().map(|block| block.0 as i64)),
        );
        let mut mapping = HashMap::new();
        let mut assigned_ids: HashSet<i64> = HashSet::new();
        let mut block_ids: Vec<BlockId> = func.blocks.keys().copied().collect();
        block_ids.sort_by_key(|bid| bid.0);
        for bid in block_ids {
            if let Some(&state_id) = state_yield_resume_state_for_block.get(&bid) {
                let collides_with_other_original_label = func
                    .label_id_map
                    .iter()
                    .any(|(&other_bid, &label_id)| other_bid != bid.0 && label_id == state_id);
                if !collides_with_other_original_label && assigned_ids.insert(state_id) {
                    mapping.insert(bid, state_id);
                    continue;
                }
            }
            if let Some(&label_val) = func.label_id_map.get(&bid.0)
                && assigned_ids.insert(label_val)
            {
                mapping.insert(bid, label_val);
            } else {
                let fresh = fresh_labels.fresh();
                mapping.insert(bid, fresh);
                assigned_ids.insert(fresh);
            }
        }
        mapping
    };
    let block_label_id = |bid: &BlockId| -> i64 {
        *label_id_for_block
            .get(bid)
            .unwrap_or_else(|| panic!("missing linearization label for {bid}"))
    };
    let mut trampoline_blocks: Vec<BlockId> = func
        .blocks
        .iter()
        .filter_map(|(&block_id, block)| match &block.terminator {
            Terminator::CondBranch { then_args, .. } if !then_args.is_empty() => Some(block_id),
            _ => None,
        })
        .collect();
    trampoline_blocks.sort_unstable_by_key(|block| block.0);
    let mut synthetic_labels = LabelAllocator::after_labels(label_id_for_block.values().copied());
    let trampoline_label_id_for_block: HashMap<BlockId, i64> = trampoline_blocks
        .into_iter()
        .map(|block| (block, synthetic_labels.fresh()))
        .collect();
    let trampoline_label_id = |block: &BlockId| -> i64 {
        *trampoline_label_id_for_block
            .get(block)
            .unwrap_or_else(|| panic!("missing conditional trampoline label for {block}"))
    };
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
            opcode if dominators::is_exception_transfer_edge(opcode) => {
                attr_int(&op.attrs, "value")
                    .and_then(|label_id| original_label_to_block.get(&label_id).copied())
            }
            _ => None,
        })
        .collect();

    // Every block owns an exact argument-slot vector, including empty vectors.
    // Predecessor stores and block-entry loads consume the same map.
    let block_param_vars: HashMap<BlockId, Vec<String>> = func
        .blocks
        .iter()
        .map(|(bid, block)| {
            (
                *bid,
                simple_value_names.block_arg_slots(*bid, block.args.len()),
            )
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

    // Full predecessor map covering BOTH normal terminator edges AND implicit
    // exception edges (CheckException / TryStart handler-label edges).
    // Used by the structured-loop external-reentry guard below: a loop region
    // can only be reconstructed (which merges away the labels of its inline
    // header/cond/guard blocks) when those consumed blocks have no predecessor
    // outside the region — otherwise an external `jump`/`check_exception` to a
    // merged-away label would dangle (the coroutine/generator `_poll`
    // state-machine resume case).
    let all_predecessors: HashMap<BlockId, Vec<BlockId>> = {
        let mut preds: HashMap<BlockId, Vec<BlockId>> = HashMap::new();
        for (pred_bid, block) in &func.blocks {
            for succ in block.terminator.successors() {
                preds.entry(succ).or_default().push(*pred_bid);
            }
            for op in &block.ops {
                if dominators::is_exception_transfer_edge(op.opcode)
                    && let Some(label_id) = attr_int(&op.attrs, "value")
                    && let Some(&target) = original_label_to_block.get(&label_id)
                {
                    preds.entry(target).or_default().push(*pred_bid);
                }
            }
        }
        preds
    };

    for bid in &rpo {
        if debug_loop_if_return {
            eprintln!(
                "LOWER_DEBUG_PRE_IFPATTERN bid={:?} role={:?} loop_consumed={}",
                bid,
                func.loop_roles.get(bid).cloned().unwrap_or(LoopRole::None),
                loop_consumed.contains(bid)
            );
        }
        let role = func.loop_roles.get(bid).cloned().unwrap_or(LoopRole::None);
        if role != LoopRole::LoopHeader {
            continue;
        }
        // Skip structured-region reconstruction for loops that contain a
        // `loop_break_if_exception` (now an `ExceptionPending`-conditioned
        // CondBranch in the loop body).  The structured-region model's
        // primary-break detection assumes a SINGLE non-raising loop-controlling
        // CondBranch (the done-flag break); a second non-raising mid-body
        // CondBranch (the exception-flag break emitted after ITER_NEXT in
        // iterator-consumer loops without the function exception stack) is
        // ambiguous to that detector and corrupts the reconstructed loop shape.
        // The generic block-by-block lowering below emits each CondBranch as a
        // proper `br_if`, preserving both breaks — correct on native (which
        // consumes the generic form directly).  NOTE: the WASM backend's jumpful
        // state machine does not yet number the generic exception-break edge
        // correctly (its target state falls outside the per-function br_table),
        // so the WASM/LLVM/Luau TIR-roundtripping paths still hang on this case;
        // see the baton note.  Native (the primary OOM/hang fix) is correct.
        let loop_has_exception_break = {
            let end_bid = func.loop_pairs.get(bid).copied();
            let header_idx = bid.0 as usize;
            let end_idx = end_bid.map(|b| b.0 as usize).unwrap_or(header_idx);
            (header_idx..=end_idx.max(header_idx)).any(|i| {
                func.blocks.get(&BlockId(i as u32)).is_some_and(|blk| {
                    blk.ops
                        .iter()
                        .any(|op| op.opcode == OpCode::ExceptionPending)
                })
            })
        };
        if loop_has_exception_break {
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
            while let Some(blk) = func.blocks.get(&cond_bid) {
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
                    Terminator::Switch { .. }
                    | Terminator::StateDispatch { .. }
                    | Terminator::Return { .. }
                    | Terminator::Unreachable => break,
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
        // ── Body/exit polarity from the CFG, not the stale break-kind hint ──
        // The native structured loop emits `loop_break_if_false`/`_true` whose
        // polarity decides which cond successor is the loop BODY (continue) vs
        // the EXIT (after_block). The pre-TIR `loop_break_kinds` map records the
        // polarity of the ORIGINAL `loop_break_if_*` op, but molt double-roundtrips
        // through TIR (per-function pipeline → SimpleIR → relift for the drop
        // module phase → SimpleIR), and the SSA terminator builder + drop-phase
        // critical-edge reshaping can change which side of `cond_block`'s
        // `CondBranch` is the back-edge body vs the loop exit. Trusting the stale
        // map then swaps body_entry/exit_block: the EXIT (a Return block) becomes
        // `body_entry`, and the back-edge CONTINUE becomes `exit_block`. Native
        // codegen's `loop_start` materializes an `after_block`, the swapped
        // `loop_break_if_*` marks it reachable + jumps to it from the cleanup
        // edge, but the matching `loop_end` (which would switch-to/fill it) is
        // never emitted for the degenerate shape — leaving a reachable-but-empty
        // block that Cranelift's `unreachable_code` pass rejects
        // (`while True: …; if c: break` → `rc_sites_loop_break.py`; round-10).
        //
        // The ground truth is reducibility: the loop BODY is the cond successor
        // from which the loop HEADER (`*bid`, the back-edge target) is reachable
        // through in-loop blocks; the EXIT is the successor that leaves the loop.
        // Derive that here and emit a polarity consistent with it, so the
        // reconstruction is correct regardless of `break_kind` staleness.
        let then_reaches_header = successor_reaches_header(func, *then_block, *bid, cond_bid);
        let else_reaches_header = successor_reaches_header(func, *else_block, *bid, cond_bid);
        // Fall back to the recorded hint only when the CFG is ambiguous (both or
        // neither successor reaches the header — e.g. an infinite loop with no
        // exit, or an exit that re-enters). A reducible loop with a normal exit
        // has exactly one body successor.
        let body_is_then = match (then_reaches_header, else_reaches_header) {
            (true, false) => true,
            (false, true) => false,
            _ => {
                let break_kind = func
                    .loop_break_kinds
                    .get(bid)
                    .copied()
                    .unwrap_or(LoopBreakKind::BreakIfFalse);
                // BreakIfFalse: cond TRUE → body (then). BreakIfTrue: cond TRUE
                // → break, so body is the else side.
                matches!(break_kind, LoopBreakKind::BreakIfFalse)
            }
        };
        // Native polarity that matches the chosen body side:
        //   body == then  →  `loop_break_if_false` (cond TRUE continues to body)
        //   body == else  →  `loop_break_if_true`  (cond TRUE breaks to exit)
        let break_kind = if body_is_then {
            LoopBreakKind::BreakIfFalse
        } else {
            LoopBreakKind::BreakIfTrue
        };
        let (body_entry, exit_block, body_args, exit_args) = if body_is_then {
            (
                *then_block,
                *else_block,
                then_args.clone(),
                else_args.clone(),
            )
        } else {
            (
                *else_block,
                *then_block,
                else_args.clone(),
                then_args.clone(),
            )
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
                    for succ in blk.terminator.successors() {
                        stack.push(succ);
                    }
                }
            }
        }
        // Exclude LoopEnd structural markers — they are not real body blocks.
        //
        // Exception handler blocks are deliberately kept when the natural loop
        // reaches them.  They are semantically owned by the protected loop body;
        // removing them here makes their cleanup/continuation successors look
        // like external re-entry and forces a generic fallback that cannot emit
        // a matched loop_start/loop_end region.
        body_set.retain(|b| {
            func.loop_roles.get(b).cloned().unwrap_or(LoopRole::None) != LoopRole::LoopEnd
        });

        // ── Single-entry-region guard (structured-reconstruction soundness) ──
        // Structured loop reconstruction merges the region's interior blocks
        // (header-chain, cond, guard-chain, and body blocks) into one linear
        // `loop_start … loop_continue/loop_break … loop_end` sequence. Several
        // of those interior blocks are emitted INLINE without their own `label`
        // op — the cond/header-chain/guard-chain blocks, the FIRST body block,
        // and any body block whose terminator is the back-edge (emitted as a
        // bare `loop_continue`) or the loop-exit edge (emitted as a bare
        // `loop_break`). Reconstruction is therefore sound ONLY when the region
        // is single-entry: the loop HEADER is the unique block reachable from
        // outside the region. The header always keeps its label (it is the
        // forward-jump / back-edge target), so an external edge into the header
        // resolves; an external edge into any OTHER region block targets a block
        // whose label may be merged away, leaving that `jump`/`br_if`/
        // `check_exception` dangling — the
        // "TIR roundtrip emitted invalid labels" panic (and the native
        // `label_blocks[&target]` "no entry found for key" panic / WASM
        // "unknown jump label" warning on the same SimpleIR).
        //
        // This single-entry invariant is exactly natural-loop reducibility: in a
        // well-formed reducible loop only the header has predecessors outside
        // the loop, so this guard never rejects a well-formed region. It DOES
        // reject irreducible / multi-entry shapes such as:
        //   * a coroutine/generator `_poll` resume edge that re-enters the loop's
        //     COND block from outside the region (the historical case), and
        //   * a shared pre-header/latch block: `entry → P → header` where the
        //     back-edge also routes `latch → P → header`, so `P` is pulled into
        //     `body_set` (it is the back-edge's source-side block) yet still
        //     carries the external entry edge from `entry`. The drop-insertion
        //     terminal phase (critical-edge splits + retain placement) reshapes
        //     loop back-edges into exactly this funnel, which is why activating
        //     native drops surfaced it (`_typing_strip_wrapping_parens`).
        //
        // On rejection the generic block-by-block lowering below emits every
        // block with its own label and resolves every edge correctly (mirroring
        // the `loop_has_exception_break` bypass above and the single-predecessor
        // requirement enforced for structured-if inlining).
        let region_block_set: HashSet<BlockId> = {
            let mut s = HashSet::new();
            s.insert(*bid);
            s.insert(cond_bid);
            s.extend(header_chain.iter().copied());
            s.extend(guard_chain.iter().copied());
            s.extend(guard_raise_blocks.iter().copied());
            s.extend(body_set.iter().copied());
            s
        };
        // The header is the sole legal entry: every OTHER region block must be
        // entered exclusively from within the region. `all_predecessors` covers
        // both normal terminator edges and implicit exception edges, so this
        // catches an external `check_exception`/`try_*` handler edge into the
        // interior as well as plain `jump`/`br_if` reentry.
        let has_external_reentry = region_block_set.iter().any(|member| {
            *member != *bid
                && (*member == func.entry_block
                    || all_predecessors
                        .get(member)
                        .is_some_and(|preds| preds.iter().any(|p| !region_block_set.contains(p))))
        });
        if has_external_reentry {
            // Leave this loop to the generic block-by-block lowering, which
            // preserves every consumed block's label.  Do not mark any block
            // consumed and do not register a region for this header.
            continue;
        }

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
    }
    let block_contains_nested_scf = |block: &TirBlock| {
        block
            .ops
            .iter()
            .any(|op| opcode_is_structured_scf_marker_table(op.opcode))
    };
    let mut if_patterns: HashMap<BlockId, IfPattern> = HashMap::new();
    let mut if_inlined_blocks: HashSet<BlockId> = HashSet::new();
    let mut predecessors: HashMap<BlockId, HashSet<BlockId>> = HashMap::new();
    for (pred_bid, block) in &func.blocks {
        for succ in block.terminator.successors() {
            predecessors.entry(succ).or_default().insert(*pred_bid);
        }
    }

    for bid in &rpo {
        let role = func.loop_roles.get(bid).cloned().unwrap_or(LoopRole::None);
        if role != LoopRole::None || loop_consumed.contains(bid) {
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
        // Function invocation is an implicit predecessor of entry. It cannot
        // be consumed as an arm merely because all explicit edges come here.
        if then_bid == else_bid || then_bid == func.entry_block || else_bid == func.entry_block {
            continue;
        }
        // Successor blocks that are loop headers must not be inlined.
        let then_role = func
            .loop_roles
            .get(&then_bid)
            .cloned()
            .unwrap_or(LoopRole::None);
        let else_role = func
            .loop_roles
            .get(&else_bid)
            .cloned()
            .unwrap_or(LoopRole::None);
        if then_role != LoopRole::None || else_role != LoopRole::None {
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
            block
                .ops
                .iter()
                .any(|op| opcode_has_exception_label_attr_table(op.opcode))
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
            Terminator::CondBranch { .. }
            | Terminator::Switch { .. }
            | Terminator::StateDispatch { .. } => {
                continue;
            }
        };
        let else_target = match &else_blk.terminator {
            Terminator::Branch { target, .. } => Some(*target),
            Terminator::Return { .. } | Terminator::Unreachable => None,
            Terminator::CondBranch { .. }
            | Terminator::Switch { .. }
            | Terminator::StateDispatch { .. } => {
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
            // An entry join is a backedge, never lexical fallthrough after
            // end_if. Leave both predecessor transfers in the canonical CFG.
            if join == func.entry_block {
                continue;
            }
            let join_role = func
                .loop_roles
                .get(&join)
                .cloned()
                .unwrap_or(LoopRole::None);
            if join_role != LoopRole::None {
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
        if_patterns.insert(*bid, IfPattern { then_bid, else_bid });
        if debug_loop_if_return {
            eprintln!(
                "LOWER_DEBUG_IF_PATTERN bid={:?} then={:?} else={:?} join={:?}",
                bid, then_bid, else_bid, join_bid
            );
        }
        if_inlined_blocks.insert(then_bid);
        if_inlined_blocks.insert(else_bid);
    }
    // Invocation supplies entry arguments once. Reentry supplies them through
    // the same join slots as every other predecessor. Keep these seed stores
    // outside the entry label/loop so a backedge cannot replay initialization.
    let entry_needs_join = all_predecessors
        .get(&func.entry_block)
        .is_some_and(|preds| !preds.is_empty())
        || loop_regions.contains_key(&func.entry_block);
    if entry_needs_join {
        let entry = func
            .blocks
            .get(&func.entry_block)
            .expect("missing TIR entry block");
        let args: Vec<_> = entry.args.iter().map(|arg| arg.id).collect();
        emit_block_arg_stores(func.entry_block, &args, &block_param_vars, &mut out);
    }
    for bid in &rpo {
        if debug_loop_if_return {
            eprintln!(
                "LOWER_DEBUG_EMIT bid={:?} loop_consumed={} role={:?} if_inlined={}",
                bid,
                loop_consumed.contains(bid),
                func.loop_roles.get(bid).cloned().unwrap_or(LoopRole::None),
                if_inlined_blocks.contains(bid)
            );
        }
        // Skip blocks consumed by structured loop emission.
        if loop_consumed.contains(bid) {
            continue;
        }

        let loop_role = func.loop_roles.get(bid).cloned().unwrap_or(LoopRole::None);
        // Skip LoopEnd blocks only when nothing still branches to them.
        // Some loop-end blocks survive optimization as explicit CFG targets,
        // and dropping their labels leaves dangling jump targets in the
        // round-tripped SimpleIR.
        let has_explicit_predecessor = predecessors.get(bid).is_some_and(|preds| !preds.is_empty());
        if loop_role == LoopRole::LoopEnd && !has_explicit_predecessor {
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
        if loop_role == LoopRole::LoopHeader && loop_regions.contains_key(bid) {
            emit_structured_loop_region(
                *bid,
                func,
                &loop_regions,
                &rpo,
                &block_param_vars,
                &block_label_id,
                &trampoline_label_id,
                &if_inlined_blocks,
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

        // Emit every executable destination, including entry with predecessors.
        // LoopHeaders with proven regions are handled above.  Remaining loop
        // headers stay in the generic label/jump form: emitting only loop_start
        // here creates a half-structured loop with no matching loop_end.
        if *bid != func.entry_block || entry_needs_join {
            let label_id = block_label_id(bid);
            let label_kind = if state_yield_resume_state_for_block
                .get(bid)
                .is_some_and(|state_id| *state_id == label_id)
            {
                "state_label"
            } else {
                "label"
            };
            out.push(OpIR {
                kind: label_kind.to_string(),
                value: Some(label_id),
                ..OpIR::default()
            });

            emit_block_arg_loads(block, &block_param_vars, &mut out);
        }

        // Helper: emit a block's ops with type annotation.
        let emit_block_ops = |block: &TirBlock, out: &mut Vec<OpIR>| {
            emit_block_ops_inner(
                block,
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

            let Terminator::CondBranch {
                cond,
                then_args,
                else_args,
                ..
            } = &block.terminator
            else {
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

            // Emit: if cond
            out.push(OpIR {
                kind: "if".to_string(),
                args: Some(vec![value_var(*cond)]),
                ..OpIR::default()
            });

            for (index, (arm, args)) in [(then_blk, then_args), (else_blk, else_args)]
                .into_iter()
                .enumerate()
            {
                if index != 0 {
                    out.push(OpIR {
                        kind: "else".to_string(),
                        ..OpIR::default()
                    });
                }
                // Inlining removes labels, not predecessor value transport.
                emit_block_arg_stores(arm.id, args, &block_param_vars, &mut out);
                emit_block_arg_loads(arm, &block_param_vars, &mut out);
                emit_block_ops(arm, &mut out);
                match &arm.terminator {
                    Terminator::Return { values } => {
                        emit_return_ops(values, &mut out);
                    }
                    Terminator::Branch { target, args } => {
                        emit_block_arg_stores(*target, args, &block_param_vars, &mut out);
                    }
                    Terminator::Unreachable => {}
                    _ => unreachable!("non-simple terminator in structured if arm"),
                }
            }

            // Emit: end_if
            out.push(OpIR {
                kind: "end_if".to_string(),
                ..OpIR::default()
            });
        } else {
            // Non-loop, non-if-pattern block: emit ops and terminator normally.
            emit_block_ops(block, &mut out);
            emit_terminator(
                block,
                &block_param_vars,
                &block_label_id,
                &trampoline_label_id,
                &if_inlined_blocks,
                &mut out,
                &func.loop_break_kinds,
            );
        }
    }

    reset_value_names();

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

    // Never turn a lost destination into target-specific falloff. All consumers
    // must receive the same closed namespace, not an optional EH-only warning.
    let missing = missing_label_references(&out);
    assert!(
        missing.is_empty(),
        "[TIR] invalid label lowering for {}: missing labels {:?}",
        func.name,
        missing
    );

    out
}
