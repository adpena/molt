//! One indexed graph projection for block and program-point dominance.
use super::idoms_in_rpo;

#[derive(Debug, Clone)]
pub struct IndexedDominance {
    rpo_indices: Vec<Option<usize>>,
    idoms: Vec<Option<usize>>,
    intervals: Vec<(usize, usize)>,
}

impl IndexedDominance {
    pub fn compute(successors: &[Vec<usize>], entry: usize) -> Self {
        let order = if entry < successors.len() {
            crate::tir::traversal::indexed_reverse_postorder(successors, entry)
        } else {
            Vec::new()
        };
        let mut rpo_indices = vec![None; successors.len()];
        for (index, &node) in order.iter().enumerate() {
            rpo_indices[node] = Some(index);
        }
        let mut predecessors = vec![Vec::new(); order.len()];
        for (index, &node) in order.iter().enumerate() {
            for &next in &successors[node] {
                if let Some(next) = rpo_indices[next] {
                    predecessors[next].push(index);
                }
            }
        }
        let rpo_idoms = idoms_in_rpo(&predecessors);
        let mut idoms = vec![None; successors.len()];
        for (index, &node) in order.iter().enumerate().skip(1) {
            idoms[node] = rpo_idoms[index].map(|parent| order[parent]);
        }
        Self {
            rpo_indices,
            idoms,
            intervals: dominator_intervals(&rpo_idoms),
        }
    }

    pub fn is_reachable(&self, node: usize) -> bool {
        self.rpo_indices.get(node).is_some_and(Option::is_some)
    }

    pub fn dominates(&self, definition: usize, usage: usize) -> bool {
        let Some(definition) = self.rpo_indices.get(definition).copied().flatten() else {
            return false;
        };
        let Some(usage) = self.rpo_indices.get(usage).copied().flatten() else {
            return false;
        };
        let (entered, exited) = self.intervals[definition];
        let (usage_entered, usage_exited) = self.intervals[usage];
        entered <= usage_entered && usage_exited <= exited
    }

    pub fn immediate_dominator(&self, node: usize) -> Option<usize> {
        self.idoms.get(node).copied().flatten()
    }

    pub fn immediate_dominators(&self) -> &[Option<usize>] {
        &self.idoms
    }
}

/// An iterative dominator-tree traversal keeps queries constant time without
/// recursion proportional to guest function size.
fn dominator_intervals(idoms: &[Option<usize>]) -> Vec<(usize, usize)> {
    let mut children = vec![Vec::new(); idoms.len()];
    let mut roots = Vec::new();
    for (node, idom) in idoms.iter().enumerate() {
        match *idom {
            Some(parent) if parent != node => children[parent].push(node),
            _ => roots.push(node),
        }
    }
    let mut intervals = vec![(0, 0); idoms.len()];
    let mut clock = 0;
    let mut stack: Vec<_> = roots.into_iter().rev().map(|root| (root, false)).collect();
    while let Some((node, exiting)) = stack.pop() {
        if exiting {
            intervals[node].1 = clock;
        } else {
            intervals[node].0 = clock;
            stack.push((node, true));
            stack.extend(children[node].iter().rev().map(|&child| (child, false)));
        }
        clock += 1;
    }
    intervals
}

#[cfg(test)]
mod tests {
    use super::*;
    fn reachable_without(
        graph: &[Vec<usize>],
        entry: usize,
        banned: Option<usize>,
        target: usize,
    ) -> bool {
        let mut seen = vec![false; graph.len()];
        let mut stack = vec![entry];
        while let Some(node) = stack.pop() {
            if Some(node) == banned || seen[node] {
                continue;
            }
            if node == target {
                return true;
            }
            seen[node] = true;
            stack.extend(&graph[node]);
        }
        false
    }

    #[test]
    fn intervals_match_independent_path_search_with_loops_and_disconnected_nodes() {
        // Exhaust every directed four-node graph with no self edges. Backedges,
        // irreducible loops, disconnected entries and diamonds all participate.
        for mask in 0u16..4096 {
            let mut graph = vec![Vec::new(); 4];
            let mut bit = 0;
            for (from, edges) in graph.iter_mut().enumerate() {
                for to in 0..4 {
                    if from == to {
                        continue;
                    }
                    if mask & (1 << bit) != 0 {
                        edges.push(to);
                    }
                    bit += 1;
                }
            }
            let tree = IndexedDominance::compute(&graph, 0);
            for usage in 0..4 {
                let reachable = reachable_without(&graph, 0, None, usage);
                assert_eq!(tree.is_reachable(usage), reachable);
                for definition in 0..4 {
                    assert_eq!(
                        tree.dominates(definition, usage),
                        reachable && !reachable_without(&graph, 0, Some(definition), usage),
                        "mask={mask} definition={definition} usage={usage}"
                    );
                }
            }
        }
        let empty = IndexedDominance::compute(&[], 0);
        assert!(!empty.dominates(0, 0));
    }
}
