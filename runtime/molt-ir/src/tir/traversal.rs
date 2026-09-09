//! Iterative postorder authority for dense pre-SSA and sparse TIR graphs.

/// `visit` admits an existing, not-yet-visited node and appends its successors
/// in semantic order, excluding targets already visited at scheduling time.
/// Returning false excludes a missing or already-seen node. Unseen siblings
/// remain unmarked until entry so cross-edges preserve depth-first order.
/// One scratch successor buffer is reused; graph depth never uses call stack.
pub(crate) fn reverse_postorder_by<Node: Copy>(
    roots: impl IntoIterator<Item = Node>,
    capacity: usize,
    mut visit: impl FnMut(Node, &mut Vec<Node>) -> bool,
) -> Vec<Node> {
    let mut order = Vec::with_capacity(capacity);
    let mut stack = Vec::with_capacity(capacity);
    let mut successors = Vec::new();
    stack.extend(roots.into_iter().map(|root| (root, false)));
    stack.reverse();
    while let Some((node, exiting)) = stack.pop() {
        if exiting {
            order.push(node);
            continue;
        }
        successors.clear();
        if !visit(node, &mut successors) {
            continue;
        }
        stack.push((node, true));
        stack.extend(successors.iter().rev().map(|&next| (next, false)));
    }
    order.reverse();
    order
}

/// Dense CFG/SSA adapter. Structural validation owns invalid-edge diagnostics;
/// analysis never treats an out-of-domain index as a graph node.
pub(crate) fn indexed_reverse_postorder(successors: &[Vec<usize>], entry: usize) -> Vec<usize> {
    let mut visited = vec![false; successors.len()];
    reverse_postorder_by([entry], successors.len(), |node, output| {
        let Some(seen) = visited.get_mut(node) else {
            return false;
        };
        if std::mem::replace(seen, true) {
            return false;
        }
        output.extend(
            successors[node]
                .iter()
                .copied()
                .filter(|&next| visited.get(next).is_some_and(|seen| !seen)),
        );
        true
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dense_domain_handles_empty_dangling_duplicate_and_cyclic_edges() {
        assert!(indexed_reverse_postorder(&[], 0).is_empty());
        assert!(indexed_reverse_postorder(&[vec![]], 1).is_empty());
        assert_eq!(indexed_reverse_postorder(&[vec![99]], 0), vec![0]);
        assert_eq!(
            indexed_reverse_postorder(&[vec![0, 1, 1], vec![0]], 0),
            vec![0, 1]
        );
    }

    #[test]
    fn dense_backedges_and_pending_sibling_crossedges_preserve_dfs_order() {
        let successors: Vec<Vec<usize>> = (0..128)
            .map(|node| std::iter::once(node + 1).chain(0..node).collect())
            .collect();
        assert_eq!(
            indexed_reverse_postorder(&successors, 0),
            (0..128).collect::<Vec<_>>()
        );
        // Node 2 is pending when reached through node 1. Marking all siblings
        // during scheduling instead of entry would incorrectly put 2 before 1.
        assert_eq!(
            indexed_reverse_postorder(&[vec![1, 2], vec![2], vec![]], 0),
            vec![0, 1, 2]
        );
    }
}
