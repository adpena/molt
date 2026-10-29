use super::{DominanceGraph, DominatorTree};
use crate::tir::blocks::BlockId;
use std::collections::{HashMap, HashSet};

struct Graph {
    entry: BlockId,
    edges: HashMap<BlockId, Vec<BlockId>>,
}
impl DominanceGraph for Graph {
    fn entry_block(&self) -> BlockId {
        self.entry
    }
    fn block_count(&self) -> usize {
        self.edges.len()
    }
    fn append_successors(&self, block: BlockId, output: &mut Vec<BlockId>) -> bool {
        let Some(edges) = self.edges.get(&block) else {
            return false;
        };
        output.extend(edges);
        true
    }
}
fn reachable(graph: &Graph, target: BlockId, removed: Option<BlockId>) -> bool {
    let mut pending = vec![graph.entry];
    let mut seen = HashSet::new();
    while let Some(block) = pending.pop() {
        if Some(block) == removed || !seen.insert(block) {
            continue;
        }
        let Some(edges) = graph.edges.get(&block) else {
            continue;
        };
        if block == target {
            return true;
        }
        pending.extend(edges);
    }
    false
}

#[test]
fn sparse_graph_boundary_matches_path_removal_oracle() {
    let ids = [11, 29, 101, 400].map(BlockId);
    for mask in 0..4096 {
        let mut edges = HashMap::new();
        let mut bit = 0;
        for from in ids {
            let mut targets = vec![BlockId(99999)]; // Structural dangling edge.
            for to in ids {
                if from == to {
                    continue;
                }
                if mask & (1 << bit) != 0 {
                    targets.extend([to, to]);
                }
                bit += 1;
            }
            edges.insert(from, targets);
        }
        let graph = Graph {
            entry: ids[0],
            edges,
        };
        let tree = DominatorTree::compute(&graph);
        for target in ids {
            let live = reachable(&graph, target, None);
            assert_eq!(tree.contains(target), live);
            for dominator in ids {
                assert_eq!(
                    tree.dominates(dominator, target),
                    live && !reachable(&graph, target, Some(dominator)),
                    "mask={mask} {dominator:?} dom {target:?}"
                );
            }
            if let Some(parent) = tree.idom(target) {
                assert_ne!(parent, target);
                assert!(tree.dominates(parent, target));
                for other in ids {
                    if other != target && tree.dominates(other, target) {
                        assert!(tree.dominates(other, parent));
                    }
                }
            } else {
                assert!(target == graph.entry || !live);
            }
        }
        assert!(!tree.contains(BlockId(99999)));
    }
    let empty = Graph {
        entry: ids[0],
        edges: HashMap::new(),
    };
    assert!(DominatorTree::compute(&empty).is_empty());
}
