//! CFG surgery that wires a cloned generator body into the consumer loop.
//!
//! After [`super::clone::clone_and_rewrite_poll`] produces the fresh
//! [`super::clone::ClonedPoll`], whose frame slots are already SSA values,
//! these helpers splice the consumer body at the yield site, delete the frame
//! creation ops, bind the promoted parameter slots, and rewire the consumer's
//! loop-entry edges. The orchestrating `apply_fusion` and recognition live in
//! [`super`].

use std::collections::HashSet;

use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

use super::clone::{ClonedPoll, const_int_op};
use super::{FusionCandidate, is_get_iter_op};

// ---------------------------------------------------------------------------
// Wire the fused loop
// ---------------------------------------------------------------------------

/// Wire the cloned (rewritten) poll body into the consumer loop:
///  * splice the consumer body at the yield site (bind `elem`, run the body,
///    return to the post-yield continuation);
///  * route the cloned exhausted-return to the consumer's loop exit;
///  * delete the frame-creation ops (`AllocTask`/`GetIter`/`IterNext`), bind
///    the promoted parameter slots at the top of the generator preheader, and
///    redirect the consumer's loop entry to it.
///
/// The frame slots are already SSA values of the cloned body, joins included
/// ([`super::slots`]). Returns `false` (bail) on a structural surprise.
pub(super) fn wire_fused_loop(
    caller: &mut TirFunction,
    candidate: &FusionCandidate,
    clone: &ClonedPoll,
    preheader_init_ops: Vec<TirOp>,
) -> bool {
    // --- 1. Splice the consumer body at the yield site. ---
    // Split the cloned yield block into [pre-yield | post-yield].
    let (pre_block, post_block) = match split_block_at(caller, clone.yield_block, clone.yield_idx) {
        Some(pair) => pair,
        None => return false,
    };
    // pre_block ends (currently) with a Branch to post_block (from split). We
    // instead extract elem = Index(yield_pair, 0) and branch to the consumer
    // body. The runtime returns the element owned, as it did the eliminated
    // `IterNext` pair's element, and the drop plane releases that result once:
    // fusion places no reference operation (design 20 §1.2). The consumer body
    // (the caller's body_block) on continue branches to post_block.
    let elem_index_op = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Index,
        operands: vec![clone.yield_pair, const_zero(caller)],
        results: vec![candidate.elem_val],
        attrs: {
            let mut a = AttrDict::new();
            a.insert("container_type".into(), AttrValue::Str("tuple".into()));
            a
        },
        source_span: None,
    };
    {
        let pb = caller.blocks.get_mut(&pre_block).unwrap();
        pb.ops.push(elem_index_op);
        pb.terminator = Terminator::Branch {
            target: candidate.body_block,
            args: Vec::new(),
        };
    }

    // The consumer body block currently starts with `elem = Index(orig_pair, 0)`
    // (referencing the now-dead IterNext pair). Remove that leading op (elem is
    // now bound by `pre_block`).
    remove_orig_elem_index(caller, candidate);

    // --- 2. Route the cloned exhausted-return blocks to the loop exit. A
    //        straight-line poll returns from its yield block, whose terminator
    //        the split moved to the post-yield half. ---
    for &rb in &clone.return_blocks {
        let rb = if rb == clone.yield_block { post_block } else { rb };
        caller.blocks.get_mut(&rb).unwrap().terminator = Terminator::Branch {
            target: candidate.exit_block,
            args: Vec::new(),
        };
    }

    // --- 3. Delete the frame-creation ops and bind the parameter slots. ---
    delete_frame_creation_ops(caller, candidate, clone.entry, preheader_init_ops);

    // --- 4. Rewire the consumer's old loop header edges. The old loop header
    //        (`loop_header`, e.g. the `loop_start` block) had two kinds of
    //        predecessor: the loop ENTRY (from outside the loop) and the
    //        CONTINUE back-edge (from the consumer body). After fusion:
    //          * the ENTRY edge → the generator preheader (the cloned entry);
    //          * the CONTINUE edge → the generator post-yield block.
    //        We split the old-header preds by whether they are reachable from
    //        `body_block` (continue) or not (entry). The old header + the old
    //        cond/iter_next block become unreachable and DCE removes them.
    if !rewire_consumer_header_edges(caller, candidate, clone.entry, post_block) {
        return false;
    }

    // --- 5. Prune the now-unreachable blocks: the consumer's old loop header
    //        and cond block (with its `IterNext`/done-`Index` on the deleted
    //        pair), and any cloned poll block the poll itself never entered.
    //        `verify_function` skips unreachable blocks, but the
    //        TIR→SimpleIR back-conversion would still emit their `jump`/`label`
    //        ops + dangling uses of the deleted pair value — which the native
    //        codegen's `jump` handler rejects (`label_blocks[&target_id]` panic).
    //        Remove them here so codegen never sees them. ---
    if let Some(header) = candidate.loop_header {
        caller.retire_loop_metadata(header);
    }
    let retained = super::super::reachability::metadata_preserving_reachable_blocks(caller);
    caller
        .retain_blocks(&retained)
        .expect("generator fusion must preserve unrelated loops and live block references");

    true
}

/// True if `block`'s terminator targets `target`.
fn block_targets(caller: &TirFunction, block: BlockId, target: BlockId) -> bool {
    caller
        .blocks
        .get(&block)
        .is_some_and(|block| block.terminator.has_successor(target))
}

/// Materialize a `ConstInt(0)` in the caller (for the `Index(pair, 0)` element
/// extraction), returning its value id. Cached-free: a fresh const each call is
/// fine (copy-prop/GVN dedups them in the re-run pipeline).
fn const_zero(caller: &mut TirFunction) -> ValueId {
    let v = caller.fresh_value();
    caller.value_types.insert(v, TirType::I64);
    // The const op is inserted by the caller of this fn into the pre-yield block.
    // To keep it dominating, we must actually emit it; we stash it via a thread
    // local is overkill — instead emit it directly into the entry block top.
    let entry = caller.entry_block;
    caller
        .blocks
        .get_mut(&entry)
        .unwrap()
        .ops
        .insert(0, const_int_op(v, 0));
    v
}

/// Split block `bid` after op index `idx` (the yield op was already dropped, so
/// `idx` is the position the post-yield ops begin). Returns `(pre, post)` block
/// ids; `pre` keeps the original id, `post` is fresh and takes the original
/// terminator + the ops `[idx..]`. `pre` is given a placeholder Branch to `post`
/// (the caller rewrites it).
fn split_block_at(
    caller: &mut TirFunction,
    bid: BlockId,
    idx: usize,
) -> Option<(BlockId, BlockId)> {
    if idx > caller.blocks.get(&bid)?.ops.len() {
        return None;
    }
    let post_id = caller.fresh_block();
    let original = caller.blocks.get_mut(&bid).unwrap();
    let post_ops = original.ops.split_off(idx);
    let terminator = std::mem::replace(
        &mut original.terminator,
        Terminator::Branch {
            target: post_id,
            args: Vec::new(),
        },
    );
    caller.blocks.insert(
        post_id,
        TirBlock {
            id: post_id,
            args: Vec::new(),
            ops: post_ops,
            terminator,
        },
    );
    Some((bid, post_id))
}

/// Remove the consumer body's leading `Index(orig_pair, 0) -> elem_val` op (it
/// now references the deleted IterNext pair; `elem_val` is rebound by the
/// yield-pre block).
fn remove_orig_elem_index(caller: &mut TirFunction, candidate: &FusionCandidate) {
    let block = caller.blocks.get_mut(&candidate.elem_block).unwrap();
    block.ops.retain(|op| {
        !(op.opcode == OpCode::Index
            && op.operands.first() == Some(&candidate.pair_val)
            && op.results.first() == Some(&candidate.elem_val))
    });
}

/// Delete the frame-creation ops (`AllocTask`, `GetIter`/`iter`, `IterNext`) and
/// bind the promoted parameter slots at the top of the generator preheader. The
/// `GetIter` result is replaced by a non-`None` sentinel const so the consumer's
/// `is(iter, None)` not-iterable guard folds False (the iterator never escapes
/// after fusion).
fn delete_frame_creation_ops(
    caller: &mut TirFunction,
    candidate: &FusionCandidate,
    preheader: BlockId,
    preheader_init_ops: Vec<TirOp>,
) {
    // (a) Remove the AllocTask op.
    if let Some(block) = caller.blocks.get_mut(&candidate.alloc_block) {
        block.ops.retain(|op| {
            !(op.opcode == OpCode::AllocTask && op.results.first() == Some(&candidate.alloc_val))
        });
    }
    // (b) Replace the GetIter op with ConstInt(1) producing iter_val (sentinel).
    if let Some(block) = caller.blocks.get_mut(&candidate.get_iter_block) {
        for op in block.ops.iter_mut() {
            if is_get_iter_op(op) && op.results.first() == Some(&candidate.iter_val) {
                *op = const_int_op(candidate.iter_val, 1);
                break;
            }
        }
    }
    caller.value_types.insert(candidate.iter_val, TirType::I64);
    // (c) Remove the IterNext op.
    if let Some(block) = caller.blocks.get_mut(&candidate.cond_block) {
        block.ops.retain(|op| {
            !(op.opcode == OpCode::IterNext && op.results.first() == Some(&candidate.pair_val))
        });
    }
    // (d) Prepend the parameter-slot bindings at the TOP of the cloned
    //     preheader, so they dominate every promoted read.
    if !preheader_init_ops.is_empty()
        && let Some(pre) = caller.blocks.get_mut(&preheader)
    {
        for (i, op) in preheader_init_ops.into_iter().enumerate() {
            pre.ops.insert(i, op);
        }
    }
}

/// Rewire the consumer's old loop-header edges after the generator body has been
/// spliced in. The old loop header (`candidate.loop_header`, e.g. the
/// `loop_start` block; falls back to `cond_block`) has predecessors of two
/// kinds:
///   * the **continue** back-edge(s) from inside the consumer body region
///     (blocks reachable from `body_block` without leaving the loop) →
///     retargeted to `post_block` (the generator's post-yield continuation);
///   * the **entry** edge(s) from outside the loop → retargeted to `preheader`
///     (the generator's cloned entry).
///
/// Returns `false` if the header has a predecessor that is neither (an
/// unexpected irreducible shape) — a conservative bail.
fn rewire_consumer_header_edges(
    caller: &mut TirFunction,
    candidate: &FusionCandidate,
    preheader: BlockId,
    post_block: BlockId,
) -> bool {
    let old_header = candidate.loop_header.unwrap_or(candidate.cond_block);

    // The consumer body region: blocks reachable from `body_block` without
    // passing through the old header or the loop exit (those bound the region).
    let body_region = reachable_avoiding(
        caller,
        candidate.body_block,
        &[old_header, candidate.exit_block],
    );

    // Every predecessor of `old_header`: classify + retarget its edge.
    let preds: Vec<BlockId> = caller
        .blocks
        .keys()
        .copied()
        .filter(|&b| block_targets(caller, b, old_header))
        .collect();
    for pred in preds {
        let new_target = if body_region.contains(&pred) {
            post_block // continue edge
        } else {
            preheader // entry edge
        };
        retarget_edges(caller, pred, old_header, new_target);
    }
    true
}

/// Retarget every edge from `block` that targets `from` so it targets `to`,
/// clearing the edge's args (the new target — preheader / post-yield — takes no
/// args from this edge; slot args are threaded separately at the header).
fn retarget_edges(caller: &mut TirFunction, block: BlockId, from: BlockId, to: BlockId) {
    if let Some(b) = caller.blocks.get_mut(&block) {
        b.terminator.for_each_edge_mut(|target, args| {
            if *target == from {
                *target = to;
                args.clear();
            }
        });
    }
}

/// The set of blocks reachable from `start` via terminator edges WITHOUT
/// entering any block in `barriers` (the barriers bound the search; `start`
/// itself is included even if it is a barrier).
fn reachable_avoiding(
    caller: &TirFunction,
    start: BlockId,
    barriers: &[BlockId],
) -> HashSet<BlockId> {
    let barrier: HashSet<BlockId> = barriers.iter().copied().collect();
    let mut seen = HashSet::new();
    let mut stack = vec![start];
    seen.insert(start);
    while let Some(b) = stack.pop() {
        let succs = caller
            .blocks
            .get(&b)
            .map(|block| block.terminator.successors())
            .unwrap_or_default();
        for s in succs {
            if barrier.contains(&s) {
                continue;
            }
            if seen.insert(s) {
                stack.push(s);
            }
        }
    }
    seen
}
