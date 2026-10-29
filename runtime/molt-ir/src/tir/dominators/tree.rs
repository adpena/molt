//! The block dominator tree of one TIR CFG edge policy.
//!
//! [`compute_dominator_tree_with`](super::compute_dominator_tree_with) solves
//! the tree once (Semi-NCA) and numbers it once (`RpoDominatorTree`). Every
//! query is then O(1): `dominates` compares two pre/post intervals, as LLVM's
//! `DominatorTree::dominates` compares DFS in/out numbers and Cranelift's
//! `DominatorTree::dominates` compares pre-order intervals. A pass that
//! asks once per block or edge therefore costs O(blocks + edges), not
//! O(blocks x tree depth).
//!
//! The tree is immutable. A changed CFG builds a new tree, and the new tree
//! numbers itself, so no numbering can outlive the shape it describes. The
//! analysis manager drops the cached tree with every other CFG-sensitive
//! analysis.

use std::collections::HashMap;

use super::indexed::RpoDominatorTree;
use crate::tir::blocks::BlockId;

/// Immediate dominators of the blocks reachable under one edge policy, with a
/// pre/post numbering for constant-time dominance.
#[derive(Debug, Clone, Default)]
pub struct DominatorTree {
    /// Reachable blocks in the policy's reverse postorder; the entry is first.
    blocks: Vec<BlockId>,
    /// Reverse-postorder position of each reachable block.
    positions: HashMap<BlockId, usize>,
    tree: RpoDominatorTree,
}

impl DominatorTree {
    /// `blocks` are the reachable blocks in reverse postorder and
    /// `predecessors[position]` their reachable predecessor positions.
    pub(super) fn from_reverse_postorder(
        blocks: Vec<BlockId>,
        positions: HashMap<BlockId, usize>,
        predecessors: &[Vec<usize>],
    ) -> Self {
        let tree = RpoDominatorTree::from_predecessors(predecessors);
        debug_assert_eq!(tree.len(), blocks.len());
        Self {
            blocks,
            positions,
            tree,
        }
    }

    /// Whether `block` is reachable, and so a node of the tree.
    pub fn contains(&self, block: BlockId) -> bool {
        self.positions.contains_key(&block)
    }

    /// The immediate dominator of a reachable block; `None` for the entry and
    /// for blocks outside the tree.
    pub fn idom(&self, block: BlockId) -> Option<BlockId> {
        let position = *self.positions.get(&block)?;
        self.tree
            .immediate_dominator(position)
            .map(|parent| self.blocks[parent])
    }

    /// Whether every path from the entry to `target` passes `dominator`.
    /// A reachable block dominates itself. A block outside the tree dominates
    /// nothing and is dominated by nothing.
    pub fn dominates(&self, dominator: BlockId, target: BlockId) -> bool {
        match (self.positions.get(&dominator), self.positions.get(&target)) {
            (Some(&dominator), Some(&target)) => self.tree.dominates(dominator, target),
            _ => false,
        }
    }

    /// The reachable blocks in reverse postorder, the entry first.
    pub fn blocks(&self) -> &[BlockId] {
        &self.blocks
    }

    /// Each reachable block with its immediate dominator, in reverse postorder.
    pub fn iter(&self) -> impl Iterator<Item = (BlockId, Option<BlockId>)> + '_ {
        self.blocks.iter().map(|&block| (block, self.idom(block)))
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }
}

/// Two trees are equal when they relate the same blocks by the same immediate
/// dominators. The visit order and the numbering derive from that relation and
/// from successor order, so they take no part: an analysis self-check compares
/// what a tree answers, not how it was walked.
impl PartialEq for DominatorTree {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len()
            && self
                .iter()
                .all(|(block, idom)| other.contains(block) && other.idom(block) == idom)
    }
}

impl Eq for DominatorTree {}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::tir::blocks::{BlockId, Terminator, TirBlock};
    use crate::tir::dominators::{CfgEdgePolicy, build_pred_map_with, compute_dominator_tree_with};
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;

    const BLOCKS: usize = 4;

    /// Whether `target` is reachable from the entry when `banned` is removed.
    fn reachable_without(edges: &[Vec<usize>], banned: Option<usize>, target: usize) -> bool {
        let mut seen = [false; BLOCKS];
        let mut stack = vec![0];
        while let Some(node) = stack.pop() {
            if Some(node) == banned || seen[node] {
                continue;
            }
            if node == target {
                return true;
            }
            seen[node] = true;
            stack.extend(&edges[node]);
        }
        false
    }

    /// Every successor set of at most two blocks, self edges included.
    fn successor_sets() -> Vec<Vec<usize>> {
        let mut sets = vec![vec![]];
        for first in 0..BLOCKS {
            sets.push(vec![first]);
            for second in first + 1..BLOCKS {
                sets.push(vec![first, second]);
            }
        }
        sets
    }

    fn function(terminators: &[&Vec<usize>], exception_edge: bool) -> TirFunction {
        let mut func = TirFunction::new(
            "dom".into(),
            vec![],
            TirType::None,
            crate::FunctionReturnAbi::Void,
        );
        let cond = func.fresh_value();
        let ids: Vec<BlockId> = std::iter::once(func.entry_block)
            .chain((1..BLOCKS).map(|_| func.fresh_block()))
            .collect();
        assert_eq!(ids, (0..BLOCKS as u32).map(BlockId).collect::<Vec<_>>());
        for (index, targets) in terminators.iter().enumerate() {
            let terminator = match targets.as_slice() {
                [] => Terminator::Return { values: vec![] },
                [target] => Terminator::Branch {
                    target: ids[*target],
                    args: vec![],
                },
                [then_target, else_target] => Terminator::CondBranch {
                    cond,
                    then_block: ids[*then_target],
                    then_args: vec![],
                    else_block: ids[*else_target],
                    else_args: vec![],
                },
                _ => unreachable!("at most two successors"),
            };
            func.blocks.insert(
                ids[index],
                TirBlock {
                    id: ids[index],
                    args: vec![],
                    ops: vec![],
                    terminator,
                },
            );
        }
        if exception_edge {
            // Block 1 observes a pending exception that enters block 3.
            let mut attrs = AttrDict::new();
            attrs.insert("value".into(), AttrValue::Int(7));
            func.blocks.get_mut(&ids[1]).unwrap().ops.push(TirOp {
                origin: None,
                dialect: Dialect::Molt,
                opcode: OpCode::CheckException,
                operands: vec![],
                results: vec![],
                attrs,
                source_span: None,
            });
            func.label_id_map.insert(ids[3].0, 7);
        }
        func
    }

    #[test]
    fn interval_dominance_matches_independent_path_search() {
        let sets = successor_sets();
        let mut graphs = 0;
        for a in &sets {
            for b in &sets {
                for c in &sets {
                    for d in &sets {
                        let terminators = [a, b, c, d];
                        for exception_edge in [false, true] {
                            let func = function(&terminators, exception_edge);
                            for policy in [CfgEdgePolicy::Full, CfgEdgePolicy::TerminatorOnly] {
                                let mut edges: Vec<Vec<usize>> = terminators
                                    .iter()
                                    .map(|targets| (*targets).clone())
                                    .collect();
                                if exception_edge && policy == CfgEdgePolicy::Full {
                                    edges[1].push(3);
                                }
                                let preds = build_pred_map_with(&func, policy);
                                let tree = compute_dominator_tree_with(&func, &preds, policy);
                                let block = |index: usize| BlockId(index as u32);
                                for target in 0..BLOCKS {
                                    let reachable = reachable_without(&edges, None, target);
                                    assert_eq!(tree.contains(block(target)), reachable);
                                    for dominator in 0..BLOCKS {
                                        assert_eq!(
                                            tree.dominates(block(dominator), block(target)),
                                            reachable
                                                && !reachable_without(
                                                    &edges,
                                                    Some(dominator),
                                                    target
                                                ),
                                            "{terminators:?} exception={exception_edge} \
                                             {policy:?}: {dominator} dom {target}"
                                        );
                                    }
                                    // The immediate dominator is the strict
                                    // dominator that every other strict
                                    // dominator dominates.
                                    let idom = tree.idom(block(target));
                                    if let Some(idom) = idom {
                                        assert!(tree.dominates(idom, block(target)));
                                        assert_ne!(idom, block(target));
                                        for other in 0..BLOCKS {
                                            if other != target
                                                && tree.dominates(block(other), block(target))
                                            {
                                                assert!(tree.dominates(block(other), idom));
                                            }
                                        }
                                    } else {
                                        assert!(target == 0 || !reachable);
                                    }
                                }
                                graphs += 1;
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(graphs, sets.len().pow(4) * 4);
    }

    #[test]
    fn equality_compares_the_idom_relation_not_the_walk_order() {
        // The same relation from two successor orders: entry -> {1, 2} -> 3.
        let sets = [vec![1, 2], vec![3], vec![3], vec![]];
        let swapped = [vec![2, 1], vec![3], vec![3], vec![]];
        let trees: Vec<_> = [&sets, &swapped]
            .into_iter()
            .map(|shape| {
                let refs: Vec<&Vec<usize>> = shape.iter().collect();
                let func = function(&refs, false);
                let preds = build_pred_map_with(&func, CfgEdgePolicy::Full);
                compute_dominator_tree_with(&func, &preds, CfgEdgePolicy::Full)
            })
            .collect();
        assert_ne!(trees[0].blocks(), trees[1].blocks(), "walk orders differ");
        assert_eq!(trees[0], trees[1]);
        assert_eq!(
            trees[0].iter().collect::<HashMap<_, _>>(),
            HashMap::from([
                (BlockId(0), None),
                (BlockId(1), Some(BlockId(0))),
                (BlockId(2), Some(BlockId(0))),
                (BlockId(3), Some(BlockId(0))),
            ])
        );
    }
}
