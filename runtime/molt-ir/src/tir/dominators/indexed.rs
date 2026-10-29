//! One dominator-tree core for block and program-point dominance.
//!
//! Semi-NCA solves the immediate dominators, as LLVM's `DominatorTree` and
//! Cranelift's `DominatorTree` construction both do. One iterative depth-first
//! walk then gives every tree node a pre/post interval, so `dominates`
//! compares two intervals in O(1) instead of walking an idom chain. This is the
//! numbering LLVM keeps as DFS in/out numbers and Cranelift keeps as pre-order
//! intervals. The tree is immutable: a changed CFG builds a new tree, which
//! numbers itself again. No step recurses, so guest function size never sets
//! native stack depth.

/// A dominator tree over nodes numbered in reverse postorder (the entry is 0),
/// with the pre/post interval of each node.
#[derive(Debug, Clone, Default)]
pub(super) struct RpoDominatorTree {
    idoms: Vec<Option<usize>>,
    intervals: Vec<(usize, usize)>,
}

impl RpoDominatorTree {
    /// `predecessors[node]` lists the predecessors of each node; node 0 is
    /// the entry and every node is reachable from it.
    pub(super) fn from_predecessors(predecessors: &[Vec<usize>]) -> Self {
        let idoms = semi_nca_idoms(predecessors);
        let intervals = dominator_intervals(&idoms);
        Self { idoms, intervals }
    }

    pub(super) fn len(&self) -> usize {
        self.idoms.len()
    }

    pub(super) fn immediate_dominator(&self, position: usize) -> Option<usize> {
        self.idoms[position]
    }

    /// Ancestor-or-self in the dominator tree, from the intervals alone.
    pub(super) fn dominates(&self, dominator: usize, target: usize) -> bool {
        let (entered, exited) = self.intervals[dominator];
        let (target_entered, target_exited) = self.intervals[target];
        entered <= target_entered && target_exited <= exited
    }
}

#[derive(Debug, Clone)]
pub struct IndexedDominance {
    rpo_indices: Vec<Option<usize>>,
    idoms: Vec<Option<usize>>,
    tree: RpoDominatorTree,
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
        let tree = RpoDominatorTree::from_predecessors(&predecessors);
        let mut idoms = vec![None; successors.len()];
        for (index, &node) in order.iter().enumerate() {
            idoms[node] = tree.immediate_dominator(index).map(|parent| order[parent]);
        }
        Self {
            rpo_indices,
            idoms,
            tree,
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
        self.tree.dominates(definition, usage)
    }

    pub fn immediate_dominator(&self, node: usize) -> Option<usize> {
        self.idoms.get(node).copied().flatten()
    }

    pub fn immediate_dominators(&self) -> &[Option<usize>] {
        &self.idoms
    }
}

/// Immediate dominators by Semi-NCA (Georgiadis, "Linear-Time Algorithms for
/// Dominators and Related Problems", 2005). Node 0 is the entry; the result
/// has `None` there and at any node the edges do not reach.
///
/// A depth-first spanning tree numbers the nodes in preorder. Semidominators
/// come from a link-eval forest with path compression, visiting nodes in
/// reverse preorder; each immediate dominator is then the nearest ancestor of
/// the spanning-tree parent at or above the semidominator. Cooper-Harvey-
/// Kennedy's predecessor intersection instead climbs one idom chain per
/// predecessor and per round: quadratic when a wide join (one exception
/// handler shared by every statement) sits below a deep chain.
fn semi_nca_idoms(predecessors: &[Vec<usize>]) -> Vec<Option<usize>> {
    const UNVISITED: usize = usize::MAX;
    let n = predecessors.len();
    if n == 0 {
        return Vec::new();
    }
    let mut successors = vec![Vec::new(); n];
    for (node, preds) in predecessors.iter().enumerate() {
        for &pred in preds {
            successors[pred].push(node);
        }
    }
    // Depth-first preorder with spanning-tree parents. A node is numbered when
    // it is popped, from its most recent scheduling, so the tree is a true
    // depth-first tree: every tree ancestor precedes its descendants and every
    // non-tree edge into a node comes from a later-numbered node or an
    // ancestor.
    let mut number = vec![UNVISITED; n];
    let mut vertex = Vec::with_capacity(n);
    let mut parent = Vec::with_capacity(n);
    let mut stack = vec![(0usize, 0usize)];
    while let Some((node, from)) = stack.pop() {
        if number[node] != UNVISITED {
            continue;
        }
        let index = vertex.len();
        number[node] = index;
        vertex.push(node);
        parent.push(from);
        stack.extend(
            successors[node]
                .iter()
                .rev()
                .filter(|&&next| number[next] == UNVISITED)
                .map(|&next| (next, index)),
        );
    }
    let count = vertex.len();
    // Preorder-indexed semidominators, eval labels and link-eval ancestors.
    let mut semi: Vec<usize> = (0..count).collect();
    let mut label: Vec<usize> = (0..count).collect();
    let mut ancestor = parent.clone();
    let mut path = Vec::new();
    for w in (1..count).rev() {
        // The spanning-tree parent is a predecessor numbered before `w`.
        semi[w] = parent[w];
        for &pred in &predecessors[vertex[w]] {
            let v = number[pred];
            if v == UNVISITED {
                continue;
            }
            // Nodes numbered after `w` are linked into the forest.
            let u = eval(v, w + 1, &mut ancestor, &mut label, &semi, &mut path);
            semi[w] = semi[w].min(semi[u]);
        }
    }
    // Ascending preorder: every candidate's own idom is already final.
    let mut idom = parent;
    for w in 1..count {
        let mut candidate = idom[w];
        while candidate > semi[w] {
            candidate = idom[candidate];
        }
        idom[w] = candidate;
    }
    let mut idoms = vec![None; n];
    for w in 1..count {
        idoms[vertex[w]] = Some(vertex[idom[w]]);
    }
    idoms
}

/// The node of least semidominator on the forest path from `v` up to, but not
/// including, its first ancestor numbered below `last_linked`. Compresses the
/// path so later queries skip it; `path` is reused scratch space.
fn eval(
    v: usize,
    last_linked: usize,
    ancestor: &mut [usize],
    label: &mut [usize],
    semi: &[usize],
    path: &mut Vec<usize>,
) -> usize {
    if ancestor[v] < last_linked {
        return label[v];
    }
    let mut top = v;
    loop {
        path.push(top);
        top = ancestor[top];
        if ancestor[top] < last_linked {
            break;
        }
    }
    // `top` is the highest linked node; its ancestor is the forest root.
    let mut above = top;
    let mut above_label = label[top];
    while let Some(node) = path.pop() {
        ancestor[node] = ancestor[above];
        if semi[above_label] < semi[label[node]] {
            label[node] = above_label;
        } else {
            above_label = label[node];
        }
        above = node;
    }
    label[v]
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

    /// The immediate dominator strictly dominates its node, and every other
    /// strict dominator of the node dominates it.
    fn assert_idoms_are_nearest(graph: &[Vec<usize>], tree: &IndexedDominance, context: &str) {
        for node in 0..graph.len() {
            match tree.immediate_dominator(node) {
                Some(idom) => {
                    assert_ne!(idom, node, "{context}: node {node}");
                    assert!(tree.dominates(idom, node), "{context}: node {node}");
                    for other in 0..graph.len() {
                        if other != node && tree.dominates(other, node) {
                            assert!(tree.dominates(other, idom), "{context}: node {node}");
                        }
                    }
                }
                None => assert!(node == 0 || !tree.is_reachable(node), "{context}: {node}"),
            }
        }
    }

    #[test]
    fn semi_nca_matches_path_search_on_random_graphs() {
        // Deterministic xorshift; no external randomness in a proof.
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = |bound: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % bound as u64) as usize
        };
        for case in 0..3_000 {
            let nodes = 2 + next(23);
            let mut graph: Vec<Vec<usize>> = (0..nodes)
                .map(|_| (0..next(4)).map(|_| 0).collect())
                .collect();
            for edges in &mut graph {
                for target in edges.iter_mut() {
                    *target = next(nodes);
                }
            }
            // Some cases add a wide join below a chain, the shape that made
            // predecessor intersection quadratic.
            if case % 5 == 0 {
                let join = next(nodes);
                for (node, edges) in graph.iter_mut().enumerate() {
                    if node + 1 < nodes {
                        edges.push(node + 1);
                    }
                    edges.push(join);
                }
            }
            let tree = IndexedDominance::compute(&graph, 0);
            let context = format!("case {case}: {graph:?}");
            for usage in 0..nodes {
                let reachable = reachable_without(&graph, 0, None, usage);
                assert_eq!(tree.is_reachable(usage), reachable, "{context}");
                for definition in 0..nodes {
                    assert_eq!(
                        tree.dominates(definition, usage),
                        reachable && !reachable_without(&graph, 0, Some(definition), usage),
                        "{context}: {definition} dom {usage}"
                    );
                }
            }
            assert_idoms_are_nearest(&graph, &tree, &context);
        }
    }

    #[test]
    fn wide_join_below_a_deep_chain_has_exact_idoms() {
        // One exception handler shared by every statement of a long body.
        const CHAIN: usize = 50_000;
        let pad = CHAIN;
        let graph: Vec<Vec<usize>> = (0..=CHAIN)
            .map(|node| match node {
                0 => vec![1],
                node if node == pad => vec![],
                node if node + 1 < CHAIN => vec![pad, node + 1],
                _ => vec![pad],
            })
            .collect();
        let tree = IndexedDominance::compute(&graph, 0);
        for node in 1..CHAIN {
            assert_eq!(tree.immediate_dominator(node), Some(node - 1));
        }
        assert_eq!(tree.immediate_dominator(pad), Some(1));
        assert!(tree.dominates(1, CHAIN - 1) && !tree.dominates(2, pad));
    }
}
