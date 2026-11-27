//! CFG surgery that wires a cloned generator body into the consumer loop.
//!
//! After [`super::clone::clone_and_rewrite_poll`] produces the fresh
//! [`super::clone::ClonedPoll`], whose frame slots are already SSA values,
//! these helpers splice the consumer body at the yield site, delete the frame
//! creation ops, bind the promoted parameter slots, rewire the consumer's
//! loop-entry edges and thread the values the consumer's loop carries through
//! the generator's control flow. The orchestrating `apply_fusion` and
//! recognition live in [`super`].

use molt_ir::deterministic_hash::{HashMap, HashSet};

use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::clone_support::{LabelAllocator, function_label_ids};
use crate::tir::dominators::{
    exception_edge_binds_handler_arguments, exception_label_to_block, exception_transfer_target,
};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::passes::ownership_lattice_min::Replacements;
use crate::tir::passes::spliced_frame::{finish_op, publish_if_observed};
use crate::tir::ssa_update::{self, Variable};
use crate::tir::types::TirType;
use crate::tir::values::{TirValue, ValueId};

use super::clone::{ClonedPoll, const_int_op};
use super::{FusionCandidate, is_get_iter_op};

// ---------------------------------------------------------------------------
// Wire the fused loop
// ---------------------------------------------------------------------------

/// Wire the cloned (rewritten) poll body into the consumer loop:
///  * splice the consumer body at the yield site (bind `elem`, run the body,
///    return to the post-yield continuation);
///  * route the cloned exhausted return to the consumer's loop exit, and each
///    exception exit to the handler of the consumer's iteration check;
///  * delete the frame-creation ops (`AllocTask`/`GetIter`), bind the
///    promoted parameter slots at the top of the generator preheader, and
///    redirect the consumer's loop entry to it;
///  * publish the consumer's iteration location where the generator body
///    starts and resumes: a spliced frame hangs below that call site
///    (spliced_frames.md 4.2);
///  * thread each value the consumer's loop header carries through the
///    generator's control flow (SSA repair).
///
/// The frame slots are already SSA values of the cloned body, joins included
/// ([`super::slots`]). Returns `false` (bail) on a structural surprise.
pub(super) fn wire_fused_loop(
    caller: &mut TirFunction,
    candidate: &FusionCandidate,
    clone: &ClonedPoll,
    preheader_init_ops: Vec<TirOp>,
    owners: &mut Replacements,
    identity: ValueId,
) -> bool {
    let old_header = candidate.loop_header.unwrap_or(candidate.cond_block);
    // --- 1. Splice the consumer body at the yield site. ---
    let (pre_block, post_block) = match split_block_at(caller, clone.yield_block, clone.yield_idx) {
        Some(pair) => pair,
        None => return false,
    };
    // The yield handed its consumer one reference of its own, as the
    // eliminated `IterNextUnboxed` returned its element owned: elem is a copy
    // that keeps that reference (`owners`), and the drop plane releases it
    // once. Fusion places no reference operation (design 20 §1.2).
    let elem_op = TirOp {
        origin: None,
        runtime: None,
        callee: None,
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![clone.yield_value],
        results: vec![candidate.elem_val],
        attrs: AttrDict::default(),
        source_span: None,
    };
    let yielded_type = caller
        .value_types
        .get(&clone.yield_value)
        .cloned()
        .unwrap_or(TirType::DynBox);
    caller.value_types.insert(candidate.elem_val, yielded_type);
    owners.record_held(candidate.elem_val);
    {
        let pb = caller.blocks.get_mut(&pre_block).unwrap();
        pb.ops.push(elem_op);
        pb.terminator = Terminator::Branch {
            target: candidate.body_block,
            args: Vec::new(),
        };
    }
    if let Some(frame) = &clone.yield_frame {
        publish_if_observed(caller, pre_block, frame);
    }

    // --- 2. Route the cloned returns: the exhausted return leaves the loop,
    //        an exception exit reaches the iteration's handler with the
    //        exception pending. A straight-line poll returns from its yield
    //        block, whose terminator the split moved to the post-yield half. ---
    for (&cloned_return, exception_exit) in clone
        .return_blocks
        .iter()
        .map(|rb| (rb, clone.exception_exits.contains(rb)))
    {
        let rb = if cloned_return == clone.yield_block {
            post_block
        } else {
            cloned_return
        };
        // The handler takes what the iteration's check passed it; the
        // values are defined before the loop, so they dominate the fused body.
        let (target, args) = if exception_exit {
            (candidate.handler, candidate.handler_args.clone())
        } else {
            (candidate.exit_block, Vec::new())
        };
        if caller
            .blocks
            .get(&target)
            .is_none_or(|t| t.args.len() != args.len())
        {
            return false;
        }
        caller.blocks.get_mut(&rb).unwrap().terminator = Terminator::Branch { target, args };
        if let Some(frame) = clone.return_frames.get(&cloned_return) {
            publish_if_observed(caller, rb, frame);
        }
    }

    // --- 3. Delete the frame-creation ops and bind the parameter slots. ---
    delete_frame_creation_ops(caller, candidate, clone.entry, preheader_init_ops);

    // --- 4. The generator body runs at the consumer's iteration site. ---
    if let Some(location) = candidate.call_location {
        for block in [clone.entry, post_block] {
            let publication = location_op(location);
            caller
                .blocks
                .get_mut(&block)
                .unwrap()
                .ops
                .insert(0, publication);
        }
    }

    // --- 5. Rewire the consumer's old loop header edges. Its entry edge
    //        reaches the generator preheader, its continue edges the post-yield
    //        block, each with the values the header carries. ---
    let carried: Vec<TirValue> = caller.blocks[&old_header].args.clone();
    let continued: Vec<TirValue> = carried
        .iter()
        .map(|arg| {
            let id = caller.fresh_value();
            caller.value_types.insert(id, arg.ty.clone());
            TirValue {
                id,
                ty: arg.ty.clone(),
            }
        })
        .collect();
    caller.blocks.get_mut(&old_header).unwrap().args.clear();
    caller.blocks.get_mut(&clone.entry).unwrap().args = carried.clone();
    caller.blocks.get_mut(&post_block).unwrap().args = continued.clone();
    if !rewire_consumer_header_edges(caller, candidate, old_header, clone.entry, post_block) {
        return false;
    }

    // --- 6. Prune the now-unreachable blocks: the consumer's old loop header,
    //        the iteration's block and its chain, and any cloned poll block the
    //        poll itself never entered. ---
    if let Some(header) = candidate.loop_header {
        caller.retire_loop_metadata(header);
    }
    let retained = super::super::reachability::metadata_preserving_reachable_blocks(caller);
    caller
        .retain_blocks(&retained)
        .expect("generator fusion must preserve unrelated loops and live block references");

    // --- 7. Each carried value is now defined twice: on entry (the
    //        preheader's argument, which keeps the header argument's name) and
    //        on each resume (the post-yield argument). Every use reads the
    //        definition that reaches it. ---
    let variables: Vec<Variable> = carried
        .iter()
        .zip(&continued)
        .map(|(entry, resume)| Variable {
            names: vec![entry.id, resume.id],
            ty: entry.ty.clone(),
        })
        .collect();
    if !variables.is_empty() && ssa_update::repair(caller, &variables).is_err() {
        return false;
    }
    retire_identity(caller, candidate, clone.entry, identity);
    true
}

/// The identity word owns the observed frame throughout the fused activation,
/// including when the consumer is executing between yields. Route every edge
/// out of that extent through one release landing per destination.
fn retire_identity(
    caller: &mut TirFunction,
    candidate: &FusionCandidate,
    entry: BlockId,
    identity: ValueId,
) {
    let labels = exception_label_to_block(caller);
    let reach = |root, stop: &dyn Fn(BlockId) -> bool| {
        let mut seen = HashSet::default();
        let mut pending = vec![root];
        while let Some(id) = pending.pop() {
            if stop(id) || !seen.insert(id) {
                continue;
            }
            let block = &caller.blocks[&id];
            block.terminator.append_successors(&mut pending);
            pending.extend(
                block
                    .ops
                    .iter()
                    .filter(|op| exception_edge_binds_handler_arguments(op.opcode))
                    .filter_map(|op| exception_transfer_target(op, &labels)),
            );
        }
        seen
    };
    // A shared continuation also visited outside the splice cannot borrow
    // its local place. Both traversals follow only executable edges.
    let outside = reach(caller.entry_block, &|id| id == entry);
    let active = reach(entry, &|id| {
        id == candidate.exit_block || id == candidate.handler || outside.contains(&id)
    });
    let is_exit = |target: BlockId| target == entry || !active.contains(&target);
    let entry_args: Vec<_> = caller.blocks[&entry]
        .args
        .iter()
        .map(|arg| arg.ty.clone())
        .collect();
    let mut ids: Vec<_> = active.iter().copied().collect();
    ids.sort_unstable();
    let mut fresh_labels = LabelAllocator::after_labels(function_label_ids(caller));
    let mut landings = HashMap::default();
    for id in ids {
        let mut block = caller.blocks.remove(&id).unwrap();
        if block.terminator.exits_activation() {
            block.ops.push(finish_op(identity));
        }
        let mut landing = |target| {
            *landings.entry(target).or_insert_with(|| {
                let id = caller.fresh_block();
                let types = if target == entry {
                    entry_args.clone()
                } else {
                    caller.blocks[&target]
                        .args
                        .iter()
                        .map(|arg| arg.ty.clone())
                        .collect()
                };
                let args: Vec<_> = types
                    .into_iter()
                    .map(|ty| {
                        let id = caller.fresh_value();
                        caller.value_types.insert(id, ty.clone());
                        TirValue { id, ty }
                    })
                    .collect();
                let terminator = Terminator::Branch {
                    target,
                    args: args.iter().map(|arg| arg.id).collect(),
                };
                let release = finish_op(identity);
                caller.blocks.insert(
                    id,
                    TirBlock {
                        id,
                        args,
                        ops: vec![release],
                        terminator,
                    },
                );
                let label = fresh_labels.fresh();
                caller.label_id_map.insert(id.0, label);
                (id, label)
            })
        };
        block.terminator.for_each_edge_mut(|target, _| {
            if is_exit(*target) {
                *target = landing(*target).0;
            }
        });
        for op in &mut block.ops {
            if exception_edge_binds_handler_arguments(op.opcode)
                && let Some(target) = exception_transfer_target(op, &labels)
                && is_exit(target)
            {
                op.attrs
                    .insert("value".into(), AttrValue::Int(landing(target).1));
            }
        }
        caller.blocks.insert(id, block);
    }
}

fn location_op(location: i64) -> TirOp {
    let mut op = TirOp::new(OpCode::FrameSetLocation, vec![], vec![]);
    op.attrs.insert("value".into(), AttrValue::Int(location));
    op
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
    caller.move_loop_terminator(bid, post_id);
    Some((bid, post_id))
}

/// Delete the frame-creation ops (`AllocTask`, `GetIter`/`iter`) and bind the
/// promoted parameter slots at the top of the generator preheader. The
/// `GetIter` result is replaced by a non-`None` sentinel const so the
/// consumer's `is(iter, None)` not-iterable guard folds False (the iterator
/// never escapes after fusion). The iteration itself retires with its block.
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
    // (c) Prepend the parameter-slot bindings at the TOP of the cloned
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
/// spliced in. The old loop header has predecessors of two kinds:
///   * the **continue** back-edge(s) from inside the consumer body region
///     (blocks reachable from `body_block` without leaving the loop) →
///     retargeted to `post_block` (the generator's post-yield continuation);
///   * the **entry** edge(s) from outside the loop → retargeted to `preheader`
///     (the generator's cloned entry).
///
/// Each edge keeps its arguments: the new target takes the values the header
/// carried. Returns `false` if the header has no entry edge.
fn rewire_consumer_header_edges(
    caller: &mut TirFunction,
    candidate: &FusionCandidate,
    old_header: BlockId,
    preheader: BlockId,
    post_block: BlockId,
) -> bool {
    // The consumer body region: blocks reachable from `body_block` without
    // passing through the old header or the loop exit (those bound the region).
    let body_region = reachable_avoiding(
        caller,
        candidate.body_block,
        &[old_header, candidate.exit_block],
    );

    let mut preds: Vec<BlockId> = caller
        .blocks
        .keys()
        .copied()
        .filter(|&b| {
            caller
                .blocks
                .get(&b)
                .is_some_and(|block| block.terminator.has_successor(old_header))
        })
        .collect();
    preds.sort_unstable();
    let mut entered = false;
    for pred in preds {
        let new_target = if body_region.contains(&pred) {
            post_block // continue edge
        } else {
            entered = true;
            preheader // entry edge
        };
        if let Some(b) = caller.blocks.get_mut(&pred) {
            b.terminator.for_each_edge_mut(|target, _| {
                if *target == old_header {
                    *target = new_target;
                }
            });
        }
    }
    entered
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
    let mut seen = HashSet::default();
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
