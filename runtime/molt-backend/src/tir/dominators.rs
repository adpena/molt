//! Shared dominator tree utilities for TIR passes.
//!
//! Provides the Cooper-Harvey-Kennedy algorithm for computing immediate
//! dominators and a `dominates` query. Factored out of `refcount_elim.rs`
//! so that multiple passes (refcount elimination, guard-to-type propagation)
//! can reuse the same dominator computation.

use std::collections::{HashMap, HashSet};

use super::blocks::{BlockId, Terminator, TirBlock};
use super::function::TirFunction;
use super::op_kinds_generated::opcode_is_exception_transfer_edge_table;
use super::ops::{AttrValue, OpCode};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Collect successor BlockIds from a terminator.
pub fn terminator_successors(term: &Terminator) -> Vec<BlockId> {
    match term {
        Terminator::Branch { target, .. } => vec![*target],
        Terminator::CondBranch {
            then_block,
            else_block,
            ..
        } => vec![*then_block, *else_block],
        Terminator::Switch { cases, default, .. }
        | Terminator::StateDispatch { cases, default, .. } => {
            let mut targets: Vec<BlockId> = cases.iter().map(|(_, t, _)| *t).collect();
            targets.push(*default);
            targets.dedup();
            targets
        }
        Terminator::Return { .. } | Terminator::Unreachable => vec![],
    }
}

/// Map each exception-handler label id to the `BlockId` that owns it.
///
/// `CheckException`/`TryStart` ops encode their handler target as a
/// *label id* in the `value` attribute (not a `BlockId`), because the implicit
/// exception edge leaves mid-block and so cannot be expressed as a terminator
/// successor. This is the inverse of `func.label_id_map` and is the single
/// source of truth for resolving those label ids to blocks.
///
/// Public so the LLVM lowering driver can reuse exactly this edge-resolution
/// logic (via [`exception_successors`]) when building its block-lowering order,
/// instead of re-deriving the label→block mapping in `lowering.rs`.
pub fn exception_label_to_block(func: &TirFunction) -> HashMap<i64, BlockId> {
    func.label_id_map
        .iter()
        .map(|(&bid, &label_id)| (label_id, BlockId(bid)))
        .collect()
}

/// Which CFG edges a dominator/reachability computation should traverse.
///
/// The TIR analysis passes (GVN, LICM, BCE, refcount-elim, …) reason about the
/// *full* control-flow graph including implicit exception edges, so that a
/// handler block reachable only via `CheckException`/`TryStart` still
/// gets a sound dominator and is treated as reachable. This is the default
/// (`Full`).
///
/// The TIR verifier deliberately restricts its SSA-dominance check to the
/// strict terminator-only CFG (`TerminatorOnly`): handler blocks reached only
/// through exception edges are intentionally *not* checked for SSA dominance,
/// because their defs may legitimately come from the protected region rather
/// than via a terminator-dominating block. Both views are produced by the same
/// algorithm here so there is exactly ONE dominator implementation over
/// `TirFunction`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CfgEdgePolicy {
    /// Terminator edges plus implicit exception edges (the analysis view).
    Full,
    /// Terminator edges only (the strict-CFG verifier view).
    TerminatorOnly,
}

/// Collect the implicit exception-edge successors of `block`.
///
/// `CheckException` branches to a handler block *mid-block* when the pending
/// exception flag is set. `TryStart` contributes the region-level exceptional
/// reachability edge so a handler remains alive even when the protected body has
/// no surviving explicit poll. `TryEnd` deliberately does **not** contribute a
/// successor: it carries the same label id for stack/lifetime pairing and
/// round-tripping, but the normal close of a region is not a transfer into the
/// handler. Treating it as one manufactures a false depth-zero handler entry and
/// makes exception ownership path-dependent.
///
/// This is the single source of truth for exception-transfer CFG edges; both
/// the dominator/reachability analyses here and the LLVM lowering driver's
/// block-ordering pass route through it so there is exactly one place that knows
/// how TIR reaches exception handlers.
///
/// Public so `lowering.rs::compute_function_rpo` can include exception-reachable
/// handler blocks in its lowering order instead of duplicating the
/// edge-extraction logic.
pub fn exception_successors(
    block: &TirBlock,
    label_to_block: &HashMap<i64, BlockId>,
) -> Vec<BlockId> {
    let mut successors = Vec::new();
    for op in &block.ops {
        if is_exception_transfer_edge(op.opcode)
            && let Some(AttrValue::Int(target_label)) = op.attrs.get("value")
            && let Some(&target) = label_to_block.get(target_label)
        {
            successors.push(target);
        }
    }
    successors
}

/// Whether an op's label-valued `value` attr is an exception-transfer CFG edge.
///
/// `TryEnd` is intentionally excluded even though it has a label-valued `value`
/// attr. Its label is structural metadata for pairing/lowering, not a branch to
/// the handler.
pub fn is_exception_transfer_edge(opcode: OpCode) -> bool {
    opcode_is_exception_transfer_edge_table(opcode)
}

/// All CFG successors of `block` under the given edge policy.
fn block_successors(
    block: &TirBlock,
    label_to_block: &HashMap<i64, BlockId>,
    policy: CfgEdgePolicy,
) -> Vec<BlockId> {
    let mut succs = terminator_successors(&block.terminator);
    if policy == CfgEdgePolicy::Full {
        succs.extend(exception_successors(block, label_to_block));
    }
    succs
}

/// Build predecessor map for the full CFG (terminator + exception edges).
///
/// This is the analysis view consumed by GVN/LICM/BCE/refcount-elim/type-refine
/// via the [`crate::tir::analysis`] manager. For the strict-CFG verifier view,
/// use [`build_pred_map_with`] with [`CfgEdgePolicy::TerminatorOnly`].
pub fn build_pred_map(func: &TirFunction) -> HashMap<BlockId, Vec<BlockId>> {
    build_pred_map_with(func, CfgEdgePolicy::Full)
}

/// Build predecessor map under an explicit edge policy. Predecessor lists are
/// sorted by `BlockId` and de-duplicated so the result is the canonical CFG
/// predecessor *set* (a block is a predecessor or it is not — multiplicity from
/// a degenerate `CondBranch { then == else }` is not meaningful).
pub fn build_pred_map_with(
    func: &TirFunction,
    policy: CfgEdgePolicy,
) -> HashMap<BlockId, Vec<BlockId>> {
    let mut pred_map: HashMap<BlockId, Vec<BlockId>> = HashMap::new();
    for &bid in func.blocks.keys() {
        pred_map.entry(bid).or_default();
    }
    let label_to_block = exception_label_to_block(func);
    for (&bid, block) in &func.blocks {
        for succ in block_successors(block, &label_to_block, policy) {
            pred_map.entry(succ).or_default().push(bid);
        }
    }
    for preds in pred_map.values_mut() {
        preds.sort_unstable_by_key(|bid| bid.0);
        preds.dedup();
    }
    pred_map
}

/// Blocks executable from the function entry through explicit terminator edges
/// plus implicit exception edges.
///
/// Structural loop metadata can keep block records alive after optimization,
/// but those metadata-only blocks are not executable CFG nodes and must not
/// contribute typed predecessor edges.
pub fn executable_reachable_blocks(func: &TirFunction) -> HashSet<BlockId> {
    reachable_blocks_with(func, CfgEdgePolicy::Full)
}

/// Blocks reachable from the function entry under an explicit edge policy.
pub fn reachable_blocks_with(func: &TirFunction, policy: CfgEdgePolicy) -> HashSet<BlockId> {
    let mut visited: HashSet<BlockId> = HashSet::new();
    let mut stack: Vec<BlockId> = vec![func.entry_block];
    let label_to_block = exception_label_to_block(func);

    while let Some(bid) = stack.pop() {
        if !visited.insert(bid) {
            continue;
        }
        let Some(block) = func.blocks.get(&bid) else {
            continue;
        };
        for succ in block_successors(block, &label_to_block, policy) {
            stack.push(succ);
        }
    }

    visited
}

// ---------------------------------------------------------------------------
// Dominator tree
// ---------------------------------------------------------------------------

/// Compute immediate dominators using the Cooper-Harvey-Kennedy algorithm over
/// the *full* CFG (terminator + exception edges). Returns a map from
/// BlockId -> Option<BlockId> (idom). The entry block has no dominator (None).
///
/// `pred_map` MUST have been built under the same edge policy (here,
/// [`CfgEdgePolicy::Full`] via [`build_pred_map`]). For the strict-CFG verifier
/// view, use [`compute_idoms_with`] with [`CfgEdgePolicy::TerminatorOnly`].
pub fn compute_idoms(
    func: &TirFunction,
    pred_map: &HashMap<BlockId, Vec<BlockId>>,
) -> HashMap<BlockId, Option<BlockId>> {
    compute_idoms_with(func, pred_map, CfgEdgePolicy::Full)
}

/// Compute immediate dominators under an explicit edge policy. The `pred_map`
/// must have been built with the *same* policy or the dominator tree is
/// undefined.
pub fn compute_idoms_with(
    func: &TirFunction,
    pred_map: &HashMap<BlockId, Vec<BlockId>>,
    policy: CfgEdgePolicy,
) -> HashMap<BlockId, Option<BlockId>> {
    // RPO numbering via DFS from entry.
    let mut rpo_order: Vec<BlockId> = Vec::new();
    let mut visited: HashSet<BlockId> = HashSet::new();

    fn dfs_postorder(
        bid: BlockId,
        func: &TirFunction,
        label_to_block: &HashMap<i64, BlockId>,
        policy: CfgEdgePolicy,
        visited: &mut HashSet<BlockId>,
        order: &mut Vec<BlockId>,
    ) {
        if !visited.insert(bid) {
            return;
        }
        if let Some(block) = func.blocks.get(&bid) {
            for succ in block_successors(block, label_to_block, policy) {
                dfs_postorder(succ, func, label_to_block, policy, visited, order);
            }
        }
        order.push(bid);
    }

    let label_to_block = exception_label_to_block(func);
    dfs_postorder(
        func.entry_block,
        func,
        &label_to_block,
        policy,
        &mut visited,
        &mut rpo_order,
    );
    rpo_order.reverse(); // Now in reverse postorder.

    // Map BlockId -> RPO index for fast lookup.
    let rpo_index: HashMap<BlockId, usize> = rpo_order
        .iter()
        .enumerate()
        .map(|(i, &bid)| (bid, i))
        .collect();

    // Intersect two dominator paths.
    let intersect = |mut a: usize, mut b: usize, doms: &[Option<usize>]| -> usize {
        while a != b {
            while a > b {
                a = doms[a].unwrap();
            }
            while b > a {
                b = doms[b].unwrap();
            }
        }
        a
    };

    let n = rpo_order.len();
    let mut doms: Vec<Option<usize>> = vec![None; n];
    doms[0] = Some(0); // Entry dominates itself.

    let mut changed = true;
    while changed {
        changed = false;
        for i in 1..n {
            let bid = rpo_order[i];
            let preds = &pred_map[&bid];

            // Find first processed predecessor.
            let mut new_idom: Option<usize> = None;
            for pred in preds {
                if let Some(&rpo_i) = rpo_index.get(pred)
                    && doms[rpo_i].is_some()
                {
                    new_idom = Some(rpo_i);
                    break;
                }
            }
            let Some(mut new_idom_val) = new_idom else {
                continue;
            };

            // Intersect with remaining processed predecessors.
            for pred in preds {
                if let Some(&rpo_i) = rpo_index.get(pred)
                    && doms[rpo_i].is_some()
                    && rpo_i != new_idom_val
                {
                    new_idom_val = intersect(rpo_i, new_idom_val, &doms);
                }
            }

            if doms[i] != Some(new_idom_val) {
                doms[i] = Some(new_idom_val);
                changed = true;
            }
        }
    }

    // Convert RPO-index idoms back to BlockIds.
    let mut result: HashMap<BlockId, Option<BlockId>> = HashMap::new();
    for (i, &bid) in rpo_order.iter().enumerate() {
        if i == 0 {
            result.insert(bid, None);
        } else {
            result.insert(bid, doms[i].map(|d| rpo_order[d]));
        }
    }
    result
}

/// Build the dominator-tree children map from an idom map: for each block, the
/// set of blocks it *immediately* dominates, in ascending `BlockId` order for
/// deterministic traversal. Every block in `idoms` appears as a key (with a
/// possibly-empty child list). The root maps any self-idom edge away.
pub fn build_dom_children(
    idoms: &HashMap<BlockId, Option<BlockId>>,
) -> HashMap<BlockId, Vec<BlockId>> {
    let mut children: HashMap<BlockId, Vec<BlockId>> = HashMap::new();
    for &bid in idoms.keys() {
        children.entry(bid).or_default();
    }
    for (&child, parent) in idoms {
        if let Some(parent) = parent
            && *parent != child
        {
            children.entry(*parent).or_default().push(child);
        }
    }
    for kids in children.values_mut() {
        kids.sort_unstable_by_key(|b| b.0);
    }
    children
}

/// Returns `true` if `dominator` dominates `target` according to the idom tree.
pub fn dominates(
    dominator: BlockId,
    target: BlockId,
    idoms: &HashMap<BlockId, Option<BlockId>>,
) -> bool {
    if dominator == target {
        return true;
    }
    let mut current = target;
    loop {
        match idoms.get(&current) {
            Some(Some(idom)) => {
                if *idom == dominator {
                    return true;
                }
                if *idom == current {
                    // Reached the root without finding dominator.
                    return false;
                }
                current = *idom;
            }
            _ => return false,
        }
    }
}

/// Compute the set of blocks that make up the natural loop with the given
/// `header_bid`, using dominator-based natural-loop construction.
///
/// A natural loop is defined by its back edges: edges `tail → header` where
/// `header` dominates `tail`. The loop body is the header plus every block that
/// can reach a back-edge tail without passing through the header. Using
/// *dominance* (rather than mere reachability from the header) is what cleanly
/// distinguishes inner-loop bodies from outer-loop bodies in nested CFGs: an
/// inner-loop preheader is reachable from the inner header (via the outer
/// iteration cycle) but is not dominated by it, so it is correctly excluded
/// from the inner loop's body.
///
/// This is the canonical loop-body collection shared by `licm` and `bce`.
/// (It replaces an earlier id-ordering heuristic in `bce` that could mis-
/// attribute blocks to a loop after CFG-renumbering passes.)
pub(crate) fn collect_loop_blocks(
    func: &TirFunction,
    pred_map: &HashMap<BlockId, Vec<BlockId>>,
    idoms: &HashMap<BlockId, Option<BlockId>>,
    header_bid: BlockId,
) -> HashSet<BlockId> {
    let mut loop_blocks = HashSet::new();
    loop_blocks.insert(header_bid);

    // Back-edge tails: predecessors of header dominated by header.
    let header_preds: &[BlockId] = pred_map
        .get(&header_bid)
        .map(|v| v.as_slice())
        .unwrap_or(&[]);

    let mut worklist: Vec<BlockId> = Vec::new();
    for &p in header_preds {
        if dominates(header_bid, p, idoms) && loop_blocks.insert(p) {
            worklist.push(p);
        }
    }

    // Walk predecessors backwards from each back-edge tail, never crossing
    // the header. The header acts as the loop's single entry, so any node
    // reaching a tail without going through the header belongs to the loop.
    while let Some(bid) = worklist.pop() {
        if let Some(block_preds) = pred_map.get(&bid) {
            for &p in block_preds {
                if p == header_bid {
                    continue;
                }
                if loop_blocks.insert(p) {
                    worklist.push(p);
                }
            }
        }
    }

    // Defensive: only retain blocks that actually exist in the function.
    loop_blocks.retain(|bid| func.blocks.contains_key(bid));
    loop_blocks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::TirBlock;
    use crate::tir::types::TirType;

    #[test]
    fn single_block_dominates_itself() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.terminator = Terminator::Return { values: vec![] };
        }
        let pred_map = build_pred_map(&func);
        let idoms = compute_idoms(&func, &pred_map);
        assert!(dominates(func.entry_block, func.entry_block, &idoms));
    }

    #[test]
    fn linear_chain_dominance() {
        // bb0 -> bb1 -> bb2
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let bb1 = func.fresh_block();
        let bb2 = func.fresh_block();

        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.terminator = Terminator::Branch {
                target: bb1,
                args: vec![],
            };
        }

        func.blocks.insert(
            bb1,
            TirBlock {
                id: bb1,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: bb2,
                    args: vec![],
                },
            },
        );

        func.blocks.insert(
            bb2,
            TirBlock {
                id: bb2,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let pred_map = build_pred_map(&func);
        let idoms = compute_idoms(&func, &pred_map);

        assert!(dominates(func.entry_block, bb1, &idoms));
        assert!(dominates(func.entry_block, bb2, &idoms));
        assert!(dominates(bb1, bb2, &idoms));
        assert!(!dominates(bb2, bb1, &idoms));
    }

    #[test]
    fn diamond_dominance() {
        // bb0 -> bb1, bb0 -> bb2, bb1 -> bb3, bb2 -> bb3
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let bb1 = func.fresh_block();
        let bb2 = func.fresh_block();
        let bb3 = func.fresh_block();
        let cond = func.fresh_value();

        // Entry has a ConstBool to use as condition.
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(crate::tir::ops::TirOp {
                dialect: crate::tir::ops::Dialect::Molt,
                opcode: crate::tir::ops::OpCode::ConstBool,
                operands: vec![],
                results: vec![cond],
                attrs: crate::tir::ops::AttrDict::new(),
                source_span: None,
            });
            entry.terminator = Terminator::CondBranch {
                cond,
                then_block: bb1,
                then_args: vec![],
                else_block: bb2,
                else_args: vec![],
            };
        }

        func.blocks.insert(
            bb1,
            TirBlock {
                id: bb1,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: bb3,
                    args: vec![],
                },
            },
        );

        func.blocks.insert(
            bb2,
            TirBlock {
                id: bb2,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Branch {
                    target: bb3,
                    args: vec![],
                },
            },
        );

        func.blocks.insert(
            bb3,
            TirBlock {
                id: bb3,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let pred_map = build_pred_map(&func);
        let idoms = compute_idoms(&func, &pred_map);

        // bb0 dominates everything
        assert!(dominates(func.entry_block, bb1, &idoms));
        assert!(dominates(func.entry_block, bb2, &idoms));
        assert!(dominates(func.entry_block, bb3, &idoms));
        // bb1 does NOT dominate bb3 (bb2 also reaches bb3)
        assert!(!dominates(bb1, bb3, &idoms));
        assert!(!dominates(bb2, bb3, &idoms));
    }

    #[test]
    fn exception_edge_target_is_reachable_for_dominance() {
        use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
        use crate::tir::values::TirValue;

        let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::None);
        let normal = func.fresh_block();
        let handler = func.fresh_block();
        let entry_arg = func.blocks[&func.entry_block].args[0].id;

        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            let mut attrs = AttrDict::new();
            attrs.insert("value".into(), AttrValue::Int(100));
            entry.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::CheckException,
                operands: vec![entry_arg],
                results: vec![],
                attrs,
                source_span: None,
            });
            entry.terminator = Terminator::Branch {
                target: normal,
                args: vec![],
            };
        }

        func.blocks.insert(
            normal,
            TirBlock {
                id: normal,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        let handler_arg = func.fresh_value();
        func.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![TirValue {
                    id: handler_arg,
                    ty: TirType::DynBox,
                }],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.label_id_map.insert(handler.0, 100);

        let pred_map = build_pred_map(&func);
        let idoms = compute_idoms(&func, &pred_map);

        assert!(dominates(func.entry_block, handler, &idoms));

        // Under the strict terminator-only policy (the verifier view), the
        // handler is reached ONLY via the exception edge, so it is not in the
        // dominator preorder at all — the two policies diverge exactly here.
        let term_pred = build_pred_map_with(&func, CfgEdgePolicy::TerminatorOnly);
        let term_idoms = compute_idoms_with(&func, &term_pred, CfgEdgePolicy::TerminatorOnly);
        assert!(
            !term_idoms.contains_key(&handler),
            "terminator-only dominators must exclude the exception-only handler"
        );
        // The normal block reachable via the terminator IS present under both.
        assert!(term_idoms.contains_key(&normal));

        // Reachability views diverge the same way.
        let full_reach = executable_reachable_blocks(&func);
        let strict_reach = reachable_blocks_with(&func, CfgEdgePolicy::TerminatorOnly);
        assert!(full_reach.contains(&handler));
        assert!(!strict_reach.contains(&handler));
        assert!(strict_reach.contains(&normal));
    }

    #[test]
    fn try_end_label_is_not_an_exception_transfer_edge() {
        use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};

        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let normal = func.fresh_block();
        let handler = func.fresh_block();

        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            let mut attrs = AttrDict::new();
            attrs.insert("value".into(), AttrValue::Int(200));
            entry.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::TryEnd,
                operands: vec![],
                results: vec![],
                attrs,
                source_span: None,
            });
            entry.terminator = Terminator::Branch {
                target: normal,
                args: vec![],
            };
        }

        func.blocks.insert(
            normal,
            TirBlock {
                id: normal,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.label_id_map.insert(handler.0, 200);

        let label_to_block = exception_label_to_block(&func);
        let entry = &func.blocks[&func.entry_block];
        assert!(exception_successors(entry, &label_to_block).is_empty());

        let full_reach = executable_reachable_blocks(&func);
        assert!(full_reach.contains(&normal));
        assert!(
            !full_reach.contains(&handler),
            "TryEnd.value is pairing metadata, not a handler-transfer edge"
        );
    }
}
