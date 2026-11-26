//! Sparse SSA relations with one edge arena and no per-value allocation.

use std::hash::Hash;
use std::num::NonZeroUsize;

use molt_ir::deterministic_hash::HashMap;

pub(super) struct ValueEdges<N, E> {
    heads: HashMap<N, Option<NonZeroUsize>>,
    edges: Vec<Edge<E>>,
}

struct Edge<E> {
    value: E,
    next: Option<NonZeroUsize>,
}

impl<N, E> Default for ValueEdges<N, E> {
    fn default() -> Self {
        Self {
            heads: HashMap::default(),
            edges: Vec::new(),
        }
    }
}

impl<N: Eq + Hash, E> ValueEdges<N, E> {
    pub(super) fn add_node(&mut self, node: N) {
        self.heads.entry(node).or_default();
    }

    pub(super) fn contains(&self, node: &N) -> bool {
        self.heads.contains_key(node)
    }

    pub(super) fn add(&mut self, from: N, value: E) {
        let head = self.heads.entry(from).or_default();
        self.edges.push(Edge { value, next: *head });
        *head = NonZeroUsize::new(self.edges.len());
    }

    pub(super) fn from(&self, node: &N) -> impl Iterator<Item = &E> {
        self.follow(self.heads.get(node).copied().flatten())
    }

    pub(super) fn take_from(&mut self, node: &N) -> impl Iterator<Item = &E> {
        let head = self.heads.remove(node).flatten();
        self.follow(head)
    }

    fn follow(&self, mut head: Option<NonZeroUsize>) -> impl Iterator<Item = &E> {
        std::iter::from_fn(move || {
            let edge = &self.edges[head?.get() - 1];
            head = edge.next;
            Some(&edge.value)
        })
    }
}
