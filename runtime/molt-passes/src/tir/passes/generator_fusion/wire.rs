//! CFG surgery that wires a cloned generator body into the consumer loop.
//!
//! After [`super::clone::clone_and_rewrite_poll`] produces the fresh
//! [`super::clone::ClonedPoll`], these helpers splice the consumer body at the
//! yield site, thread slot phis through the loop header, delete the frame
//! creation ops, and rewire the consumer's loop-entry edges. Split out of
//! `generator_fusion.rs` as a move-only decomposition; the orchestrating
//! `apply_fusion` and recognition live in [`super`].

use std::collections::HashSet;

use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};

use super::clone::{ClonedPoll, const_int_op};
use super::{FusionCandidate, SlotInfo, is_get_iter_op};

// ---------------------------------------------------------------------------
// Wire the fused loop
// ---------------------------------------------------------------------------

/// Wire the cloned (rewritten) poll body into the consumer loop:
///  * add the slot phis as args on the cloned loop header; thread init values
///    from the preheader and back-edge values from the loop latch;
///  * splice the consumer body at the yield site (bind `elem`, `IncRef`, run the
///    body, return to the post-yield continuation);
///  * route the cloned exhausted-return to the consumer's loop exit;
///  * delete the frame-creation ops (`AllocTask`/`GetIter`/`IterNext`) and
///    redirect the consumer's loop entry to the generator preheader.
///
/// Returns `false` (bail) on any structural surprise (no detectable loop header
/// for a yield that the recognition required be in a loop, etc.).
pub(super) fn wire_fused_loop(
    caller: &mut TirFunction,
    candidate: &FusionCandidate,
    clone: &ClonedPoll,
    slot_infos: &[SlotInfo],
    preheader_init_ops: Vec<TirOp>,
) -> bool {
    let n_slots = slot_infos.len();

    // --- 1. Detect the cloned loop header (the back-edge target). ---
    // The cloned blocks are NOT yet connected to the caller's CFG (the
    // preheader is wired in step 5), so a global dominance walk would treat them
    // as unreachable. Detect the loop header purely WITHIN the cloned subgraph:
    // a DFS from the cloned entry over cloned successors; a back-edge is an edge
    // C→H where H is still on the DFS stack (an ancestor of C). H is the loop
    // header, C the latch. If no back-edge exists the yield is straight-line
    // (`def g(): yield x`) and the slots flow through without a phi.
    let cloned_set: HashSet<BlockId> = clone.cloned_blocks.iter().copied().collect();
    let (loop_header, latch) = detect_cloned_back_edge(caller, clone.entry, &cloned_set);

    // --- 2. Add slot phis as header args + thread the slot values. ---
    if let Some(header) = loop_header {
        let Some(latch) = latch else { return false };
        // Append slot phis to the header's args. Precompute the phi types
        // BEFORE the mutable header borrow (the type comes from the slot's init
        // value's recorded fact).
        let phi_types: Vec<TirType> = (0..n_slots)
            .map(|i| caller_value_ty(caller_ty_lookup(caller, slot_infos, i)))
            .collect();
        {
            let hdr = caller.blocks.get_mut(&header).unwrap();
            for (i, &phi) in clone.slot_phis.iter().enumerate() {
                hdr.args.push(TirValue {
                    id: phi,
                    ty: phi_types[i].clone(),
                });
            }
        }
        // Every predecessor of `header` must now pass `n_slots` extra args.
        //   * the preheader (cloned entry): the init values.
        //   * the latch (back-edge): the back-edge values (phi for invariants).
        //   * any other pred is unexpected for a generator loop → bail.
        // Compute preds within the cloned subgraph (the cloned blocks are not yet
        // connected to the rest of the caller).
        let preds: Vec<BlockId> = cloned_set
            .iter()
            .copied()
            .filter(|&b| block_targets(caller, b, header))
            .collect();
        for pred in preds {
            let init_args: Vec<ValueId> = if pred == clone.entry {
                slot_infos.iter().map(|s| s.init_caller_val).collect()
            } else if pred == latch {
                (0..n_slots)
                    .map(|i| clone.slot_backedge[i].unwrap_or(clone.slot_phis[i]))
                    .collect()
            } else {
                // A third pred (e.g. an irreducible edge) — Phase-1 bail.
                return false;
            };
            append_branch_args(caller, pred, header, &init_args);
        }
    } else {
        // No loop: the slots are straight-line. Replace each slot phi's uses by
        // its init value directly (no phi needed). We do this by retargeting the
        // value in every cloned op — but since the clone already substituted
        // closure-loads to the phi id, we instead seed the phi as a Copy of the
        // init at the entry. Insert `phi = Copy(init)` at the cloned entry top.
        let entry = clone.entry;
        let entry_block = caller.blocks.get_mut(&entry).unwrap();
        for (i, &phi) in clone.slot_phis.iter().enumerate() {
            entry_block.ops.insert(
                0,
                TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::Copy,
                    operands: vec![slot_infos[i].init_caller_val],
                    results: vec![phi],
                    attrs: AttrDict::new(),
                    source_span: None,
                },
            );
        }
    }

    // --- 3. Splice the consumer body at the yield site. ---
    // Split the cloned yield block into [pre-yield | post-yield].
    let (pre_block, post_block) = match split_block_at(caller, clone.yield_block, clone.yield_idx) {
        Some(pair) => pair,
        None => return false,
    };
    // pre_block ends (currently) with a Branch to post_block (from split). We
    // instead: extract elem = Index(yield_pair, 0), IncRef(elem), branch to the
    // consumer body. The consumer body (the caller's body_block) on continue
    // branches to post_block.
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
    let incref_op = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::IncRef,
        operands: vec![candidate.elem_val],
        results: vec![],
        attrs: AttrDict::new(),
        source_span: None,
    };
    {
        let pb = caller.blocks.get_mut(&pre_block).unwrap();
        pb.ops.push(elem_index_op);
        pb.ops.push(incref_op);
        pb.terminator = Terminator::Branch {
            target: candidate.body_block,
            args: Vec::new(),
        };
    }

    // The consumer body block currently starts with `elem = Index(orig_pair, 0)`
    // (referencing the now-dead IterNext pair). Remove that leading op (elem is
    // now bound by `pre_block`).
    remove_orig_elem_index(caller, candidate);

    // --- 4. Route the cloned exhausted-return blocks to the loop exit. ---
    for &rb in &clone.return_blocks {
        caller.blocks.get_mut(&rb).unwrap().terminator = Terminator::Branch {
            target: candidate.exit_block,
            args: Vec::new(),
        };
    }

    // --- 5. Delete the frame-creation ops. ---
    delete_frame_creation_ops(caller, candidate, clone.entry, preheader_init_ops);

    // --- 6. Rewire the consumer's old loop header edges. The old loop header
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

    // --- 7. Prune the now-unreachable old consumer-loop blocks. After the
    //        rewiring, the consumer's old loop header + cond block (with its
    //        `IterNext`/done-`Index` on the deleted pair) are unreachable from
    //        entry. `verify_function` skips unreachable blocks, but the
    //        TIR→SimpleIR back-conversion would still emit their `jump`/`label`
    //        ops + dangling uses of the deleted pair value — which the native
    //        codegen's `jump` handler rejects (`label_blocks[&target_id]` panic).
    //        Remove them here so codegen never sees them. ---
    prune_unreachable_blocks(caller);

    true
}

/// Remove every block unreachable from the function entry, and drop any dangling
/// `loop_*` / `label_id_map` metadata keyed on them. This is a self-contained
/// cleanup so the splice never hands codegen an unreachable block carrying stale
/// ops (a use of a deleted value, a `jump` to a removed label).
///
/// Reachability uses the FULL CFG-edge policy (terminator edges PLUS the implicit
/// `CheckException` → handler/exit edges): a cloned exception-exit block is
/// reached only via the propagated-exception edge, never a terminator, so a
/// terminator-only walk would wrongly delete it — and then a surviving
/// `CheckException` whose `value` label targets it fails LLVM lowering
/// ("check_exception target label N is not present in label map").
fn prune_unreachable_blocks(caller: &mut TirFunction) {
    use crate::tir::dominators::{CfgEdgePolicy, reachable_blocks_with};
    let reachable = reachable_blocks_with(caller, CfgEdgePolicy::Full);
    let dead: Vec<BlockId> = caller
        .blocks
        .keys()
        .copied()
        .filter(|b| !reachable.contains(b))
        .collect();
    for b in dead {
        caller.blocks.remove(&b);
        caller.loop_roles.remove(&b);
        caller.loop_pairs.remove(&b);
        caller.loop_break_kinds.remove(&b);
        caller.loop_cond_blocks.remove(&b);
        caller.label_id_map.remove(&b.0);
    }
    // Drop loop metadata whose VALUE (end / cond block) was pruned.
    let live: HashSet<BlockId> = caller.blocks.keys().copied().collect();
    caller
        .loop_pairs
        .retain(|h, e| live.contains(h) && live.contains(e));
    caller
        .loop_cond_blocks
        .retain(|h, c| live.contains(h) && live.contains(c));
}

/// Detect the loop header + latch within the cloned subgraph via a DFS from the
/// cloned entry. A back-edge is an edge `C -> H` where `H` is on the DFS stack
/// when `C`'s successors are walked (`H` is an ancestor of `C`). Returns
/// `(Some(header), Some(latch))` for the FIRST back-edge found, or `(None, None)`
/// if the cloned region is acyclic (a straight-line yield).
fn detect_cloned_back_edge(
    caller: &TirFunction,
    entry: BlockId,
    cloned: &HashSet<BlockId>,
) -> (Option<BlockId>, Option<BlockId>) {
    let succs = |b: BlockId| -> Vec<BlockId> {
        caller
            .blocks
            .get(&b)
            .map(|block| block.terminator.successors())
            .unwrap_or_default()
    };
    // Iterative DFS tracking the current path stack (ancestors).
    let mut visited: HashSet<BlockId> = HashSet::new();
    let mut on_stack: HashSet<BlockId> = HashSet::new();
    // Stack frames: (block, next-successor-index, successor-list).
    let mut stack: Vec<(BlockId, usize, Vec<BlockId>)> = Vec::new();
    visited.insert(entry);
    on_stack.insert(entry);
    stack.push((entry, 0, succs(entry)));
    while !stack.is_empty() {
        let (node, i, s) = {
            let top = stack.last().unwrap();
            (top.0, top.1, top.2.clone())
        };
        if i < s.len() {
            stack.last_mut().unwrap().1 += 1;
            let next = s[i];
            if !cloned.contains(&next) {
                continue; // an edge leaving the cloned region — ignore.
            }
            if on_stack.contains(&next) {
                // Back-edge node -> next: next is the header, node the latch.
                return (Some(next), Some(node));
            }
            if visited.insert(next) {
                on_stack.insert(next);
                let ns = succs(next);
                stack.push((next, 0, ns));
            }
        } else {
            on_stack.remove(&node);
            stack.pop();
        }
    }
    (None, None)
}

/// The TirType to record for slot `i`'s phi — derived from the slot's init
/// value's known type (param args carry their own type; const ints are I64).
fn caller_ty_lookup(caller: &TirFunction, slot_infos: &[SlotInfo], i: usize) -> Option<TirType> {
    caller
        .value_types
        .get(&slot_infos[i].init_caller_val)
        .cloned()
}

fn caller_value_ty(t: Option<TirType>) -> TirType {
    t.unwrap_or(TirType::DynBox)
}

/// True if `block`'s terminator targets `target`.
fn block_targets(caller: &TirFunction, block: BlockId, target: BlockId) -> bool {
    caller
        .blocks
        .get(&block)
        .is_some_and(|block| block.terminator.has_successor(target))
}

/// Append `extra` args to `pred`'s branch terminator edge that targets `header`.
fn append_branch_args(caller: &mut TirFunction, pred: BlockId, header: BlockId, extra: &[ValueId]) {
    let block = caller.blocks.get_mut(&pred).unwrap();
    block.terminator.for_each_edge_mut(|target, args| {
        if *target == header {
            args.extend_from_slice(extra);
        }
    });
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
    let original = caller.blocks.remove(&bid)?;
    let TirBlock {
        id,
        args,
        mut ops,
        terminator,
    } = original;
    if idx > ops.len() {
        // restore and bail
        caller.blocks.insert(
            bid,
            TirBlock {
                id,
                args,
                ops,
                terminator,
            },
        );
        return None;
    }
    let post_ops = ops.split_off(idx);
    let post_id = caller.fresh_block();
    caller.blocks.insert(
        bid,
        TirBlock {
            id: bid,
            args,
            ops,
            terminator: Terminator::Branch {
                target: post_id,
                args: Vec::new(),
            },
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
/// seed the generator preheader's slot-init constants. The `GetIter` result is
/// replaced by a non-`None` sentinel const so the consumer's `is(iter, None)`
/// not-iterable guard folds False (the iterator never escapes after fusion).
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
    // (d) Prepend the preheader slot-init ops at the TOP of the cloned preheader
    //     (so they dominate the loop-header phi-arg uses).
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
