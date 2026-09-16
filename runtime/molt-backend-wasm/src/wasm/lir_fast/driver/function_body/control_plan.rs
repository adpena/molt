//! Reducible CFG stackification. Loop identity comes from shared dominance,
//! never block numbering. Each forward label ends immediately before its
//! destination; each continue label starts at its natural-loop header.

use super::super::super::lir_control::LirControlLabel;
use molt_tir::tir::blocks::BlockId;
use molt_tir::tir::dominators::{self, CfgEdgePolicy};
use molt_tir::tir::function::TirFunction;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug)]
pub(super) struct Scope {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) label: LirControlLabel,
}

pub(super) struct ControlPlan {
    pub(super) order: Vec<BlockId>,
    pub(super) scopes: Vec<Scope>,
}

impl ControlPlan {
    pub(super) fn new(graph: &TirFunction, rpo: &[BlockId]) -> Self {
        if rpo.len() == 1 {
            let mut has_edge = false;
            graph.blocks[&rpo[0]]
                .terminator
                .for_each_edge(|_, _| has_edge = true);
            if !has_edge {
                return Self {
                    order: rpo.to_vec(),
                    scopes: vec![],
                };
            }
        }
        let reject = |detail: &str| -> ! {
            panic!("unsupported LIR WASM CFG in '{}': {detail}", graph.name)
        };
        let rpo_index: HashMap<_, _> = rpo.iter().enumerate().map(|(i, &id)| (id, i)).collect();
        let mut backward_edges = Vec::new();
        for &source in rpo {
            graph.blocks[&source].terminator.for_each_edge(|target, _| {
                if rpo_index[&target] <= rpo_index[&source] {
                    backward_edges.push((source, target));
                }
            });
        }
        // RPO already exposes all possible loop headers. DAGs need no
        // predecessor map, dominators, or second graph traversal here.
        let loops: HashMap<BlockId, HashSet<BlockId>> = if backward_edges.is_empty() {
            HashMap::new()
        } else {
            let predecessors =
                dominators::build_pred_map_with(graph, CfgEdgePolicy::TerminatorOnly);
            let idoms =
                dominators::compute_idoms_with(graph, &predecessors, CfgEdgePolicy::TerminatorOnly);
            let mut headers = HashSet::new();
            for (source, target) in backward_edges {
                if !dominators::dominates(target, source, &idoms) {
                    reject("irreducible cycle: backward edge does not target a dominating header");
                }
                headers.insert(target);
            }
            headers
                .into_iter()
                .map(|header| {
                    (
                        header,
                        dominators::collect_loop_blocks(graph, &predecessors, &idoms, header),
                    )
                })
                .collect()
        };
        for (&header, members) in &loops {
            for (&other, other_members) in &loops {
                if header != other
                    && !members.is_disjoint(other_members)
                    && !members.is_subset(other_members)
                    && !other_members.is_subset(members)
                {
                    reject("irreducible overlapping natural loops");
                }
            }
        }
        let parents: HashMap<_, _> = loops
            .iter()
            .map(|(&header, members)| {
                let parent = loops
                    .iter()
                    .filter(|(_, outer)| outer.len() > members.len() && members.is_subset(outer))
                    .min_by_key(|(other, outer)| (outer.len(), **other))
                    .map(|(&other, _)| other);
                (header, parent)
            })
            .collect();
        let order = order_regions(rpo, &loops, &parents);
        assert_eq!(
            order.len(),
            rpo.len(),
            "natural-loop layout lost a reachable block"
        );
        let index: HashMap<_, _> = order.iter().enumerate().map(|(i, &id)| (id, i)).collect();
        let mut scopes = Vec::new();
        for (&header, members) in &loops {
            let start = index[&header];
            let end = members.iter().map(|id| index[id]).max().unwrap() + 1;
            assert_eq!(
                end - start,
                members.len(),
                "natural-loop layout is not contiguous"
            );
            scopes.push(Scope {
                start,
                end,
                label: LirControlLabel::LoopContinue(header),
            });
        }
        let mut forwards: HashMap<BlockId, usize> = HashMap::new();
        for (source_index, source) in order.iter().enumerate() {
            graph.blocks[source].terminator.for_each_edge(|target, _| {
                if source_index < index[&target] {
                    forwards
                        .entry(target)
                        .and_modify(|start| *start = (*start).min(source_index))
                        .or_insert(source_index);
                } else if !loops
                    .get(&target)
                    .is_some_and(|members| members.contains(source))
                {
                    reject("irreducible edge after natural-loop layout");
                }
            });
        }
        scopes.extend(forwards.into_iter().map(|(target, start)| Scope {
            start,
            end: index[&target],
            label: LirControlLabel::ForwardExit(target),
        }));
        // Process strictly earlier-ending intervals first. Their already
        // widened starts give the transitive closure in one sweep. A range-min
        // tree records starts over strict interiors, so touching/equal-end
        // intervals do not spuriously move semantic loop headers.
        scopes.sort_by_key(|scope| scope.end);
        let leaves = order.len().next_power_of_two();
        let mut starts = vec![usize::MAX; 2 * leaves];
        let mut first = 0;
        while first < scopes.len() {
            let end = scopes[first].end;
            let last = first + scopes[first..].partition_point(|scope| scope.end == end);
            for scope in &mut scopes[first..last] {
                let mut start = scope.start;
                let mut node = leaves + start;
                while node != 0 {
                    start = start.min(starts[node]);
                    node /= 2;
                }
                if start != scope.start && matches!(scope.label, LirControlLabel::LoopContinue(_)) {
                    reject("irreducible entry crosses a natural-loop scope");
                }
                scope.start = start;
            }
            for scope in &scopes[first..last] {
                let mut left = leaves + scope.start + 1;
                let mut right = leaves + scope.end;
                while left < right {
                    if left % 2 == 1 {
                        starts[left] = starts[left].min(scope.start);
                        left += 1;
                    }
                    if right % 2 == 1 {
                        right -= 1;
                        starts[right] = starts[right].min(scope.start);
                    }
                    left /= 2;
                    right /= 2;
                }
            }
            first = last;
        }
        scopes.sort_by_key(|scope| {
            let (is_loop, target) = match scope.label {
                LirControlLabel::ForwardExit(target) => (false, target),
                LirControlLabel::LoopContinue(target) => (true, target),
                LirControlLabel::Selection => unreachable!("selection is not a CFG scope"),
            };
            (scope.start, std::cmp::Reverse(scope.end), is_loop, target)
        });
        Self { order, scopes }
    }
}

/// Collapse each immediate child loop to a unit while preserving canonical
/// RPO between units. Dominating headers are first, including function entry.
fn order_regions(
    rpo: &[BlockId],
    loops: &HashMap<BlockId, HashSet<BlockId>>,
    parents: &HashMap<BlockId, Option<BlockId>>,
) -> Vec<BlockId> {
    enum Unit {
        Block(BlockId),
        Region(BlockId),
    }
    let mut regions: HashMap<Option<BlockId>, Vec<Unit>> = HashMap::new();
    for &block in rpo {
        if let Some(&parent) = parents.get(&block) {
            regions.entry(parent).or_default().push(Unit::Region(block));
        }
        let owner = loops
            .iter()
            .filter(|(_, members)| members.contains(&block))
            .min_by_key(|(header, members)| (members.len(), **header))
            .map(|(&header, _)| header);
        regions.entry(owner).or_default().push(Unit::Block(block));
    }
    let mut pending: Vec<_> = regions
        .remove(&None)
        .unwrap_or_default()
        .into_iter()
        .rev()
        .collect();
    let mut output = Vec::with_capacity(rpo.len());
    while let Some(unit) = pending.pop() {
        match unit {
            Unit::Block(block) => output.push(block),
            Unit::Region(header) => pending.extend(
                regions
                    .remove(&Some(header))
                    .expect("natural-loop region missing")
                    .into_iter()
                    .rev(),
            ),
        }
    }
    output
}
