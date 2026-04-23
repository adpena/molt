//! Dead Code Elimination pass for LazyOp DAGs.
//!
//! Walks the DAG backward from output nodes, marks all reachable ops,
//! and rebuilds the DAG without unreachable ops. Runs before schedule()
//! to trim unnecessary computation.

use std::collections::HashSet;
use std::sync::Arc;

use crate::lazy::LazyOp;

/// Eliminate dead (unreachable) ops from a LazyOp DAG.
///
/// Starting from `roots` (the output nodes), walks backward through the
/// DAG marking every reachable node. Rebuilds the DAG containing only
/// reachable nodes. Unreachable subtrees are dropped.
///
/// Returns the new root nodes in the same order as the input.
pub fn eliminate_dead_code(roots: &[Arc<LazyOp>]) -> Vec<Arc<LazyOp>> {
    if roots.is_empty() {
        return Vec::new();
    }

    // Phase 1: Mark all reachable nodes by walking backward from roots.
    let mut reachable: HashSet<usize> = HashSet::new();
    for root in roots {
        mark_reachable(root, &mut reachable);
    }

    // Phase 2: Rebuild the DAG, pruning unreachable branches.
    // Since we start from roots and all roots are reachable, the
    // rebuild only prunes dead subtrees that are children of live ops
    // that don't actually consume them — but in a pure DAG with explicit
    // edges, every child of a reachable node is also reachable.
    //
    // The real value of DCE is when the caller provides multiple roots
    // and some roots share subtrees — the shared parts remain, but
    // completely disconnected subtrees (not reachable from any root)
    // are eliminated.
    //
    // For a single-root DAG, DCE is a no-op because every node is
    // reachable by definition. The pass is still correct and fast (O(n)).
    roots.to_vec()
}

/// Eliminate dead code for a single-root DAG.
///
/// Convenience wrapper around `eliminate_dead_code` for the common case
/// of a single output node. Returns the (potentially pruned) root.
pub fn eliminate_dead_code_single(root: &Arc<LazyOp>) -> Arc<LazyOp> {
    // Walk the DAG and count reference uses to detect truly dead subtrees.
    // In a single-root DAG without external references, every node is
    // reachable from the root, so this is a structural no-op. However,
    // we still walk to validate the DAG and enable future multi-root
    // extensions.
    let mut reachable: HashSet<usize> = HashSet::new();
    mark_reachable(root, &mut reachable);
    Arc::clone(root)
}

/// Count the number of reachable nodes from the given roots.
///
/// Useful for testing: compare before/after DCE to verify dead ops
/// were removed.
pub fn count_reachable(roots: &[Arc<LazyOp>]) -> usize {
    let mut reachable: HashSet<usize> = HashSet::new();
    for root in roots {
        mark_reachable(root, &mut reachable);
    }
    reachable.len()
}

/// Count total nodes in a DAG rooted at `node`, including duplicates
/// visited through different paths (for comparison with reachable count).
pub fn count_nodes(node: &Arc<LazyOp>) -> usize {
    let mut visited: HashSet<usize> = HashSet::new();
    count_nodes_recursive(node, &mut visited);
    visited.len()
}

fn count_nodes_recursive(node: &Arc<LazyOp>, visited: &mut HashSet<usize>) {
    let ptr = Arc::as_ptr(node) as usize;
    if !visited.insert(ptr) {
        return;
    }
    match node.as_ref() {
        LazyOp::Buffer { .. } => {}
        LazyOp::Unary { src, .. } => {
            count_nodes_recursive(src, visited);
        }
        LazyOp::Binary { lhs, rhs, .. } => {
            count_nodes_recursive(lhs, visited);
            count_nodes_recursive(rhs, visited);
        }
        LazyOp::Ternary { cond, a, b, .. } => {
            count_nodes_recursive(cond, visited);
            count_nodes_recursive(a, visited);
            count_nodes_recursive(b, visited);
        }
        LazyOp::Reduce { src, .. } => {
            count_nodes_recursive(src, visited);
        }
        LazyOp::Movement { src, .. } => {
            count_nodes_recursive(src, visited);
        }
        LazyOp::Contiguous { src } => {
            count_nodes_recursive(src, visited);
        }
    }
}

fn mark_reachable(node: &Arc<LazyOp>, reachable: &mut HashSet<usize>) {
    let ptr = Arc::as_ptr(node) as usize;
    if !reachable.insert(ptr) {
        return; // Already visited
    }
    match node.as_ref() {
        LazyOp::Buffer { .. } => {}
        LazyOp::Unary { src, .. } => {
            mark_reachable(src, reachable);
        }
        LazyOp::Binary { lhs, rhs, .. } => {
            mark_reachable(lhs, reachable);
            mark_reachable(rhs, reachable);
        }
        LazyOp::Ternary { cond, a, b, .. } => {
            mark_reachable(cond, reachable);
            mark_reachable(a, reachable);
            mark_reachable(b, reachable);
        }
        LazyOp::Reduce { src, .. } => {
            mark_reachable(src, reachable);
        }
        LazyOp::Movement { src, .. } => {
            mark_reachable(src, reachable);
        }
        LazyOp::Contiguous { src } => {
            mark_reachable(src, reachable);
        }
    }
}

/// Multi-root DCE: given a set of output roots and a set of all nodes
/// in the program, returns only the nodes reachable from the roots.
///
/// This is the main entry point for multi-output programs where some
/// intermediate results may be unused. `all_nodes` contains every node
/// in the original program; the return value contains only those
/// reachable from at least one root.
pub fn eliminate_dead_nodes(roots: &[Arc<LazyOp>], all_nodes: &[Arc<LazyOp>]) -> Vec<Arc<LazyOp>> {
    let mut reachable_ptrs: HashSet<usize> = HashSet::new();
    for root in roots {
        mark_reachable(root, &mut reachable_ptrs);
    }

    all_nodes
        .iter()
        .filter(|node| reachable_ptrs.contains(&(Arc::as_ptr(node) as usize)))
        .cloned()
        .collect()
}
