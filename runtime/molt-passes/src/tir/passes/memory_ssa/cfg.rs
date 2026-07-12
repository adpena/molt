use std::collections::{HashMap, HashSet};

use crate::tir::blocks::{BlockId, TirBlock};
use crate::tir::dominators;
use crate::tir::function::TirFunction;

pub(super) fn reverse_postorder(func: &TirFunction, reachable: &HashSet<BlockId>) -> Vec<BlockId> {
    let mut visited: HashSet<BlockId> = HashSet::new();
    let mut post: Vec<BlockId> = Vec::new();
    let label_to_block: HashMap<i64, BlockId> = func
        .label_id_map
        .iter()
        .map(|(&bid, &label)| (label, BlockId(bid)))
        .collect();
    dfs_post(
        func,
        func.entry_block,
        reachable,
        &label_to_block,
        &mut visited,
        &mut post,
    );
    post.reverse();
    post
}

fn dfs_post(
    func: &TirFunction,
    bid: BlockId,
    reachable: &HashSet<BlockId>,
    label_to_block: &HashMap<i64, BlockId>,
    visited: &mut HashSet<BlockId>,
    post: &mut Vec<BlockId>,
) {
    if !reachable.contains(&bid) || !visited.insert(bid) {
        return;
    }
    if let Some(block) = func.blocks.get(&bid) {
        for s in full_cfg_successors(block, label_to_block) {
            dfs_post(func, s, reachable, label_to_block, visited, post);
        }
    }
    post.push(bid);
}

/// Full-CFG successors (terminator + implicit exception edges) — matches the
/// edge policy of the S1 dominator analyses.
fn full_cfg_successors(block: &TirBlock, label_to_block: &HashMap<i64, BlockId>) -> Vec<BlockId> {
    let mut succs = dominators::terminator_successors(&block.terminator);
    succs.extend(dominators::exception_successors(block, label_to_block));
    succs
}

/// Dominance frontiers via the Cooper/Harvey/Kennedy algorithm, computed from
/// the immediate-dominator tree and the predecessor map.
pub(super) fn compute_dominance_frontiers(
    idoms: &HashMap<BlockId, Option<BlockId>>,
    pred_map: &HashMap<BlockId, Vec<BlockId>>,
    reachable: &HashSet<BlockId>,
) -> HashMap<BlockId, HashSet<BlockId>> {
    let mut df: HashMap<BlockId, HashSet<BlockId>> = HashMap::new();
    for (&b, preds) in pred_map {
        if !reachable.contains(&b) {
            continue;
        }
        // Only join points (≥2 reachable preds) contribute to frontiers.
        let live_preds: Vec<BlockId> = preds
            .iter()
            .copied()
            .filter(|p| reachable.contains(p))
            .collect();
        if live_preds.len() < 2 {
            continue;
        }
        let idom_b = idoms.get(&b).and_then(|d| *d);
        for p in live_preds {
            let mut runner = p;
            // Walk up from `p` until we reach `b`'s idom, adding `b` to each
            // visited node's frontier.
            while Some(runner) != idom_b {
                df.entry(runner).or_default().insert(b);
                match idoms.get(&runner).and_then(|d| *d) {
                    Some(idom) if idom != runner => runner = idom,
                    // Reached the dominator-tree root; stop.
                    _ => break,
                }
            }
        }
    }
    df
}

/// The iterated dominance frontier of the Def-containing block set — the blocks
/// where memory phis must be placed. Standard worklist fixpoint.
pub(super) fn iterated_dominance_frontier(
    def_blocks: &HashSet<BlockId>,
    df: &HashMap<BlockId, HashSet<BlockId>>,
) -> HashSet<BlockId> {
    let mut phi_blocks: HashSet<BlockId> = HashSet::new();
    let mut worklist: Vec<BlockId> = def_blocks.iter().copied().collect();
    while let Some(b) = worklist.pop() {
        if let Some(frontier) = df.get(&b) {
            for &f in frontier {
                if phi_blocks.insert(f) {
                    // A new phi block is itself a "def" of memory; iterate.
                    worklist.push(f);
                }
            }
        }
    }
    phi_blocks
}

/// Dominator-tree preorder from the root, in deterministic (ascending child id)
/// order. Iterative to avoid deep recursion on long dominator chains.
pub(super) fn dom_tree_preorder(
    root: BlockId,
    dom_children: &HashMap<BlockId, Vec<BlockId>>,
) -> Vec<BlockId> {
    let mut order: Vec<BlockId> = Vec::new();
    let mut stack: Vec<BlockId> = vec![root];
    let mut seen: HashSet<BlockId> = HashSet::new();
    while let Some(b) = stack.pop() {
        if !seen.insert(b) {
            continue;
        }
        order.push(b);
        if let Some(children) = dom_children.get(&b) {
            // Push in reverse so children pop in ascending order.
            for &c in children.iter().rev() {
                stack.push(c);
            }
        }
    }
    order
}
