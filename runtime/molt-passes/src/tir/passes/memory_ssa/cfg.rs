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
