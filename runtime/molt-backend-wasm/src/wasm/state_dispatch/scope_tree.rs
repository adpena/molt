//! Bounded-depth geometry of the physical block scopes (HF-546).
//!
//! A forward transfer in structured WASM needs an enclosing label that ends
//! right before its destination. One `block` per physical block nests as deep
//! as the function has blocks: a 16,334-block module body reached Binaryen's
//! recursive stack writer at 3,182 nested `if`s after `-Oz` and overflowed a
//! 544 KiB macOS worker stack. A function of at most [`FLAT_SCOPE_LIMIT`]
//! blocks keeps one scope per block, which costs no routing. A larger one
//! lays its scopes out as a radix tree: each node holds at most
//! `2^SCOPE_FANOUT_LOG2` child scopes, so it nests about `levels * fanout`
//! deep, not one level per block.
//!
//! A transfer between leaves of one level-1 node keeps its direct `br`. A
//! transfer into another node stores the destination's digits below the
//! common node in the routing local and branches to the end of the
//! destination child's scope; each node it enters selects its child with a
//! `br_table` over one radix digit. The design and its sources are in
//! docs/spec/areas/wasm/WASM_OPTIMIZATION_PLAN.md section 1.7.

/// Most blocks a function may keep in the flat one-scope-per-block layout:
/// at most 512 nested scopes, a quarter of the 2,128 levels that overflowed
/// Binaryen's writer, and 1/32 of WABT's 16,384-label reader cap.
pub(super) const FLAT_SCOPE_LIMIT: usize = 512;
/// Log2 of the child-scope fan-out of one tree node: 128 children. Three
/// levels (387 nested scopes) cover every function body the JS API admits.
pub(super) const SCOPE_FANOUT_LOG2: u32 = 7;

#[cfg(not(test))]
fn scope_shape() -> (usize, u32) {
    (FLAT_SCOPE_LIMIT, SCOPE_FANOUT_LOG2)
}

#[cfg(test)]
thread_local! {
    static TEST_SCOPE_SHAPE: std::cell::Cell<(usize, u32)> =
        const { std::cell::Cell::new((FLAT_SCOPE_LIMIT, SCOPE_FANOUT_LOG2)) };
}

#[cfg(test)]
fn scope_shape() -> (usize, u32) {
    TEST_SCOPE_SHAPE.with(std::cell::Cell::get)
}

/// Plan the scopes of every function compiled inside `body` with a flat
/// limit and fan-out of `2^log2`, so small fixtures reach the multi-level
/// routing of huge production functions. Emission is single-threaded.
#[cfg(test)]
pub(in crate::wasm) fn with_test_scope_fanout_log2<R>(log2: u32, body: impl FnOnce() -> R) -> R {
    assert!((1..=SCOPE_FANOUT_LOG2).contains(&log2));
    let previous = TEST_SCOPE_SHAPE.with(|cell| cell.replace((1 << log2, log2)));
    let result = body();
    TEST_SCOPE_SHAPE.with(|cell| cell.set(previous));
    result
}

/// Radix-tree layout of `block_count` physical blocks. Level 1 nodes hold
/// blocks; the root is the single node at level `levels`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ScopeTree {
    block_count: usize,
    levels: u32,
    fanout_log2: u32,
}

/// The `br` that leaves `source` toward a later block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ForwardScopeExit {
    pub(super) label_depth: u32,
    /// When the lowest common node lies above level 1, the destination's
    /// ancestors select it from the routing local: this key holds the
    /// destination's digits below the common node.
    pub(super) route_key: Option<u32>,
}

impl ScopeTree {
    pub(super) fn new(block_count: usize) -> Self {
        let (flat_limit, fanout_log2) = scope_shape();
        Self::with_shape(block_count, flat_limit, fanout_log2)
    }

    fn with_shape(block_count: usize, flat_limit: usize, fanout_log2: u32) -> Self {
        if block_count <= flat_limit {
            // One node holds every block; its radix covers the whole function.
            return Self {
                block_count,
                levels: 1,
                fanout_log2: block_count.max(2).next_power_of_two().trailing_zeros(),
            };
        }
        let mut levels = 1;
        while block_count > 1usize << (fanout_log2 * levels) {
            levels += 1;
        }
        Self {
            block_count,
            levels,
            fanout_log2,
        }
    }

    /// Child scopes per node.
    pub(super) fn fanout(self) -> usize {
        1 << self.fanout_log2
    }

    /// The node at `level - 1` that is child `child` of node `node`.
    pub(super) fn child_node(self, node: usize, child: usize) -> usize {
        (node << self.fanout_log2) | child
    }

    pub(super) fn levels(self) -> u32 {
        self.levels
    }

    /// The index of `block`'s ancestor node at `level`.
    pub(super) fn node_of(self, block: usize, level: u32) -> usize {
        block >> (self.fanout_log2 * level)
    }

    /// The first block of node `node` at `level`.
    pub(super) fn node_start(self, level: u32, node: usize) -> usize {
        node << (self.fanout_log2 * level)
    }

    /// Child scopes of node `node` at `level` (level 1 children are blocks).
    pub(super) fn child_count(self, level: u32, node: usize) -> usize {
        let start = self.node_start(level, node);
        let end = self.block_count.min(self.node_start(level, node + 1));
        let child_shift = self.fanout_log2 * (level - 1);
        (end - start).div_ceil(1 << child_shift)
    }

    /// `block`'s child index within its ancestor node at `level`.
    pub(super) fn digit(self, block: usize, level: u32) -> usize {
        (block >> (self.fanout_log2 * (level - 1))) & (self.fanout() - 1)
    }

    /// Bit shift that exposes the digit selected at `level`.
    pub(super) fn digit_shift(self, level: u32) -> u32 {
        self.fanout_log2 * (level - 1)
    }

    /// Labels of this tree enclosing the body of `block`: at every level, the
    /// child scopes that end after it.
    pub(super) fn enclosing_labels(self, block: usize) -> u32 {
        self.enclosing_labels_below(block, self.levels + 1)
    }

    fn enclosing_labels_below(self, block: usize, level_limit: u32) -> u32 {
        (1..level_limit)
            .map(|level| {
                let children = self.child_count(level, self.node_of(block, level));
                (children - 1 - self.digit(block, level)) as u32
            })
            .sum()
    }

    /// The lowest level whose node contains both blocks.
    fn common_level(self, source: usize, target: usize) -> u32 {
        (1..=self.levels)
            .find(|&level| self.node_of(source, level) == self.node_of(target, level))
            .expect("both blocks belong to the root scope")
    }

    /// The exit from `source` that continues at the start of `target`'s
    /// child scope in their lowest common node.
    pub(super) fn forward_exit(self, source: usize, target: usize) -> ForwardScopeExit {
        assert!(source < target && target < self.block_count);
        let level = self.common_level(source, target);
        let below = self.enclosing_labels_below(source, level);
        let across = self.digit(target, level) - self.digit(source, level) - 1;
        ForwardScopeExit {
            label_depth: below + across as u32,
            route_key: (level > 1).then(|| (target & ((1 << self.digit_shift(level)) - 1)) as u32),
        }
    }

    /// Nodes below `level` that a transfer entering `target` passes through
    /// and that cannot fall into their first child: `(level, node)` pairs.
    pub(super) fn selecting_nodes(
        self,
        target: usize,
        level: u32,
    ) -> impl Iterator<Item = (u32, usize)> {
        (1..level)
            .filter(move |&below| self.digit(target, below) != 0)
            .map(move |below| (below, self.node_of(target, below)))
    }

    /// Nodes a forward transfer from `source` to `target` enters below their
    /// lowest common node.
    pub(super) fn routed_entry_nodes(
        self,
        source: usize,
        target: usize,
    ) -> impl Iterator<Item = (u32, usize)> {
        self.selecting_nodes(target, self.common_level(source, target))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One emitted label: the first block of the child scope that follows its
    /// `end`, and whether that child is a node (entered through selection)
    /// rather than a single block.
    type Label = (usize, bool);

    /// Independent oracle: lay the tree out as WASM would emit it (opening
    /// one block per child scope, closing one before every later child) and
    /// record each block's enclosing-label stack and the deepest nesting.
    fn emitted_geometry(tree: ScopeTree) -> (Vec<Vec<Label>>, usize) {
        fn walk(
            tree: ScopeTree,
            level: u32,
            node: usize,
            open: &mut Vec<Label>,
            stacks: &mut Vec<Vec<Label>>,
            max_depth: &mut usize,
        ) {
            let children = tree.child_count(level, node);
            let child_label = |child: usize| -> Label {
                if level == 1 {
                    (tree.node_start(level, node) + child, false)
                } else {
                    (
                        tree.node_start(level - 1, tree.child_node(node, child)),
                        true,
                    )
                }
            };
            for child in (0..children).rev() {
                open.push(child_label(child));
            }
            *max_depth = (*max_depth).max(open.len());
            open.pop();
            for child in 0..children {
                if level == 1 {
                    stacks[child_label(child).0] = open.clone();
                } else {
                    walk(
                        tree,
                        level - 1,
                        tree.child_node(node, child),
                        open,
                        stacks,
                        max_depth,
                    );
                }
                if child + 1 < children {
                    open.pop();
                }
            }
        }
        let mut stacks = vec![Vec::new(); tree.block_count];
        let mut open = Vec::new();
        let mut max_depth = 0;
        walk(tree, tree.levels, 0, &mut open, &mut stacks, &mut max_depth);
        (stacks, max_depth)
    }

    #[test]
    fn scope_tree_matches_emitted_label_geometry() {
        let cases = [
            (
                FLAT_SCOPE_LIMIT,
                SCOPE_FANOUT_LOG2,
                [1, 2, 127, 300, 512, 513, 16_384, 16_385, 40_000],
            ),
            (4, 2, [1, 3, 4, 5, 16, 17, 64, 65, 1_000]),
        ];
        for (flat_limit, fanout_log2, block_counts) in cases {
            for block_count in block_counts {
                check_geometry(ScopeTree::with_shape(block_count, flat_limit, fanout_log2));
            }
        }
    }

    fn check_geometry(tree: ScopeTree) {
        let block_count = tree.block_count;
        let fanout = tree.fanout();
        let (stacks, max_depth) = emitted_geometry(tree);
        assert!(
            max_depth <= tree.levels() as usize * fanout,
            "{block_count} blocks nest {max_depth} deep"
        );
        assert!(
            max_depth <= FLAT_SCOPE_LIMIT,
            "{block_count} blocks nest {max_depth} deep"
        );
        for (block, stack) in stacks.iter().enumerate() {
            assert_eq!(tree.enclosing_labels(block) as usize, stack.len());
        }
        let mut probes = vec![0, 1, fanout - 1, fanout, fanout + 1];
        probes.extend([block_count / 3, block_count / 2, block_count - 1]);
        probes.extend((1..tree.levels()).map(|level| tree.node_start(level, 1)));
        for &source in &probes {
            for &target in &probes {
                if source >= target || target >= block_count {
                    continue;
                }
                let exit = tree.forward_exit(source, target);
                let stack = &stacks[source];
                // The only correct landing is the enclosing scope that
                // starts last at or before the target.
                let expected = *stack
                    .iter()
                    .filter(|(start, _)| *start <= target)
                    .max_by_key(|(start, _)| *start)
                    .expect("some enclosing scope precedes every later block");
                let landed = stack[stack.len() - 1 - exit.label_depth as usize];
                assert_eq!(landed, expected, "{source}->{target}");
                assert_eq!(exit.route_key.is_some(), landed.1, "{source}->{target}");
                if let Some(key) = exit.route_key {
                    // The selectors below the landing scope see the target's
                    // offset within that scope's subtree.
                    assert_eq!(key as usize, target - landed.0, "{source}->{target}");
                }
            }
        }
    }

    #[test]
    fn scope_tree_levels_bound_engine_sized_functions() {
        let levels = |blocks| ScopeTree::new(blocks).levels();
        assert_eq!(levels(FLAT_SCOPE_LIMIT), 1);
        assert_eq!(levels(FLAT_SCOPE_LIMIT + 1), 2);
        assert_eq!(levels(128 * 128), 2);
        assert_eq!(levels(128 * 128 + 1), 3);
        // The JS API caps a function body at 7,654,321 bytes; a block needs
        // its `block` opener, its `end` and one instruction, four bytes.
        assert_eq!(levels(7_654_321 / 4), 3);
    }
}
