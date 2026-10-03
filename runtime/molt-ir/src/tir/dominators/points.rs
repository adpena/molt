//! Definition availability across mid-block exception transfers.
//!
//! Project lightweight graph segments at exception observations without cloning
//! operations or rewriting IR. Block dominance alone cannot distinguish a value
//! defined before a check from one defined below it. Restricting dominance to
//! normal edges instead loses definitions created inside handlers.
//!
//! The segment dominator tree is immutable once computed. One depth-first walk
//! numbers it, so a query compares two intervals instead of walking the tree:
//! a segment dominates exactly the segments whose intervals its own contains.

use std::collections::HashMap;

use super::{
    IndexedDominance, exception_edge_binds_handler_arguments, exception_label_to_block,
    is_exception_transfer_edge,
};
use crate::tir::blocks::BlockId;
use crate::tir::function::TirFunction;
use crate::tir::ops::AttrValue;

pub struct ProgramPointDominance {
    // (first operation position in segment, dense graph node)
    segments: HashMap<BlockId, Vec<(usize, usize)>>,
    dominance: IndexedDominance,
}

impl ProgramPointDominance {
    /// Conservative analysis graph, including region registrations.
    pub fn compute(func: &TirFunction) -> Self {
        Self::compute_with_edges(func, is_exception_transfer_edge)
    }

    /// Actual execution paths: observations leave at their operation; region
    /// registrations retain a handler but never execute it. SSA verification
    /// must include exceptional entries without manufacturing a registration
    /// entry that skips the protected body's definitions.
    pub fn compute_executable(func: &TirFunction) -> Self {
        Self::compute_with_edges(func, exception_edge_binds_handler_arguments)
    }

    fn compute_with_edges(
        func: &TirFunction,
        transfers: fn(crate::tir::ops::OpCode) -> bool,
    ) -> Self {
        let labels = exception_label_to_block(func);
        let mut blocks: Vec<_> = func.blocks.keys().copied().collect();
        blocks.sort_unstable();
        let mut segments = HashMap::with_capacity(blocks.len());
        let mut successors = Vec::<Vec<usize>>::new();
        let mut exceptional = Vec::new();
        for &bid in &blocks {
            let mut points = vec![(0, successors.len())];
            successors.push(Vec::new());
            for (index, op) in func.blocks[&bid].ops.iter().enumerate() {
                if transfers(op.opcode)
                    && let Some(AttrValue::Int(label)) = op.attrs.get("value")
                    && let Some(&target) = labels.get(label)
                {
                    let from = points.last().unwrap().1;
                    exceptional.push((from, target));
                    let next = successors.len();
                    successors[from].push(next);
                    successors.push(Vec::new());
                    points.push((index + 1, next));
                }
            }
            segments.insert(bid, points);
        }
        for (from, target) in exceptional {
            if let Some(points) = segments.get(&target) {
                successors[from].push(points[0].1);
            }
        }
        for &bid in &blocks {
            let from = segments[&bid].last().unwrap().1;
            for target in func.blocks[&bid].terminator.successors() {
                if let Some(points) = segments.get(&target) {
                    successors[from].push(points[0].1);
                }
            }
        }
        let entry = segments
            .get(&func.entry_block)
            .map_or(successors.len(), |points| points[0].1);
        Self {
            segments,
            dominance: IndexedDominance::compute(&successors, entry),
        }
    }

    fn node_before(&self, block: BlockId, position: usize) -> Option<usize> {
        let points = self.segments.get(&block)?;
        let index = points.partition_point(|&(start, _)| start <= position) - 1;
        Some(points[index].1).filter(|&node| self.dominance.is_reachable(node))
    }

    /// Whether execution can enter this block under the graph's edge policy.
    pub fn is_reachable(&self, block: BlockId) -> bool {
        self.node_before(block, 0).is_some()
    }

    /// Whether an SSA definition is available immediately before an operation.
    /// `None` identifies a block argument; `usize::MAX` as the use position
    /// queries the block's terminator boundary.
    pub fn definition_available(
        &self,
        definition_block: BlockId,
        definition_op: Option<usize>,
        use_block: BlockId,
        use_op: usize,
    ) -> bool {
        let definition_position = definition_op.map_or(0, |index| index + 1);
        if definition_block == use_block && definition_position > use_op {
            return false;
        }
        let Some(definition) = self.node_before(definition_block, definition_position) else {
            return false;
        };
        let Some(usage) = self.node_before(use_block, use_op) else {
            return false;
        };
        self.dominance.dominates(definition, usage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::{Terminator, TirBlock};
    use crate::tir::ops::{Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;

    #[test]
    fn exceptional_positions_reject_skipped_definitions_and_admit_handler_definitions() {
        let mut func = TirFunction::new(
            "point_availability".into(),
            vec![],
            TirType::None,
            crate::FunctionReturnAbi::Void,
        );
        let handler = func.fresh_block();
        let continuation = func.fresh_block();
        let disconnected = func.fresh_block();
        func.label_id_map.insert(handler.0, 7);
        let operation = |opcode, label: Option<i64>| TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: vec![],
            results: vec![],
            attrs: label.map_or_else(Default::default, |label| {
                [("value".into(), AttrValue::Int(label))]
                    .into_iter()
                    .collect()
            }),
            source_span: None,
        };
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = vec![
            operation(OpCode::CheckException, Some(7)),
            operation(OpCode::Call, None),
            operation(OpCode::CheckException, Some(7)),
        ];
        entry.terminator = Terminator::Return { values: vec![] };
        func.blocks.insert(
            handler,
            TirBlock {
                id: handler,
                args: vec![],
                ops: vec![operation(OpCode::Call, None)],
                terminator: Terminator::Branch {
                    target: continuation,
                    args: vec![],
                },
            },
        );
        for id in [continuation, disconnected] {
            func.blocks.insert(
                id,
                TirBlock {
                    id,
                    args: vec![],
                    ops: vec![],
                    terminator: Terminator::Return { values: vec![] },
                },
            );
        }
        let dominance = ProgramPointDominance::compute(&func);
        assert!(!dominance.definition_available(func.entry_block, Some(1), handler, 0));
        assert!(!dominance.definition_available(func.entry_block, Some(1), func.entry_block, 0));
        assert!(dominance.definition_available(func.entry_block, Some(1), func.entry_block, 2));
        assert!(dominance.definition_available(handler, Some(0), continuation, 0));
        assert!(dominance.definition_available(handler, None, handler, 0));
        assert!(!dominance.definition_available(disconnected, None, disconnected, 0));
    }

    fn point_op(opcode: OpCode, label: Option<i64>) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: vec![],
            results: vec![],
            attrs: label.map_or_else(Default::default, |label| {
                [("value".into(), AttrValue::Int(label))]
                    .into_iter()
                    .collect()
            }),
            source_span: None,
        }
    }

    /// The program points `(block, position)` of a function as an explicit
    /// graph, built without segments or a dominator tree. An operation moves to
    /// the next position, an exception-transfer operation at `index` also
    /// leaves from position `index` for its target's entry, and the terminator
    /// leaves from the block's last position.
    struct PointGraph {
        successors: HashMap<(BlockId, usize), Vec<(BlockId, usize)>>,
        entry: (BlockId, usize),
    }

    impl PointGraph {
        fn of(func: &TirFunction, executable: bool) -> Self {
            let labels = exception_label_to_block(func);
            let mut successors: HashMap<(BlockId, usize), Vec<(BlockId, usize)>> = HashMap::new();
            for (&block, body) in &func.blocks {
                for (index, op) in body.ops.iter().enumerate() {
                    let next = successors.entry((block, index)).or_default();
                    next.push((block, index + 1));
                    if (if executable {
                        exception_edge_binds_handler_arguments(op.opcode)
                    } else {
                        is_exception_transfer_edge(op.opcode)
                    }) && let Some(AttrValue::Int(label)) = op.attrs.get("value")
                        && let Some(&target) = labels.get(label)
                        && func.blocks.contains_key(&target)
                    {
                        next.push((target, 0));
                    }
                }
                let exit = successors.entry((block, body.ops.len())).or_default();
                for target in body.terminator.successors() {
                    if func.blocks.contains_key(&target) {
                        exit.push((target, 0));
                    }
                }
            }
            Self {
                successors,
                entry: (func.entry_block, 0),
            }
        }

        /// Whether a path from the entry reaches `point` without passing
        /// `avoid`.
        fn reaches(&self, point: (BlockId, usize), avoid: Option<(BlockId, usize)>) -> bool {
            if avoid == Some(self.entry) {
                return false;
            }
            let mut seen = std::collections::HashSet::from([self.entry]);
            let mut pending = vec![self.entry];
            while let Some(current) = pending.pop() {
                if current == point {
                    return true;
                }
                for &next in self.successors.get(&current).into_iter().flatten() {
                    if Some(next) != avoid && seen.insert(next) {
                        pending.push(next);
                    }
                }
            }
            false
        }

        /// Dominance by its definition: `usage` is reachable, and every path to
        /// it from the entry passes `definition`.
        fn dominates(&self, definition: (BlockId, usize), usage: (BlockId, usize)) -> bool {
            self.reaches(usage, None)
                && (definition == usage || !self.reaches(usage, Some(definition)))
        }
    }

    /// A small function drawn from a fixed pseudo-random stream: observations
    /// and registrations that may target any block, including their own,
    /// branches, loops, and blocks that nothing reaches.
    fn generated(seed: u64) -> TirFunction {
        let mut state = seed;
        let mut next = |bound: usize| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) as usize % bound
        };
        let mut func = TirFunction::new(
            format!("generated_{seed}"),
            vec![],
            TirType::None,
            crate::FunctionReturnAbi::Void,
        );
        let count = 2 + next(5);
        let mut blocks = vec![func.entry_block];
        blocks.extend((1..count).map(|_| func.fresh_block()));
        for (index, block) in blocks.iter().enumerate() {
            func.label_id_map.insert(block.0, 100 + index as i64);
        }
        let condition = func.fresh_value();
        for &block in &blocks {
            let ops = (0..next(4))
                .map(|_| match next(4) {
                    0 => point_op(OpCode::CheckException, Some(100 + next(count) as i64)),
                    1 => point_op(OpCode::TryStart, Some(100 + next(count) as i64)),
                    _ => point_op(OpCode::Call, None),
                })
                .collect();
            let terminator = match next(3) {
                0 => Terminator::Return { values: vec![] },
                1 => Terminator::Branch {
                    target: blocks[next(count)],
                    args: vec![],
                },
                _ => Terminator::CondBranch {
                    cond: condition,
                    then_block: blocks[next(count)],
                    then_args: vec![],
                    else_block: blocks[next(count)],
                    else_args: vec![],
                },
            };
            func.blocks.insert(
                block,
                TirBlock {
                    id: block,
                    args: vec![],
                    ops,
                    terminator,
                },
            );
        }
        func
    }

    /// The interval projection answers every definition-before-use query as the
    /// path definition of dominance does, across exceptional segments, region
    /// registrations, self-transfers, loops and unreachable blocks.
    #[test]
    fn interval_dominance_matches_every_path_through_program_points() {
        for seed in 0..96 {
            let func = generated(seed);
            for executable in [false, true] {
                let dominance = if executable {
                    ProgramPointDominance::compute_executable(&func)
                } else {
                    ProgramPointDominance::compute(&func)
                };
                let graph = PointGraph::of(&func, executable);
                let mut blocks: Vec<_> = func.blocks.keys().copied().collect();
                blocks.sort_unstable();
                for &definition_block in &blocks {
                    let operations = func.blocks[&definition_block].ops.len();
                    for definition_op in std::iter::once(None).chain((0..operations).map(Some)) {
                        let defined =
                            (definition_block, definition_op.map_or(0, |index| index + 1));
                        for &use_block in &blocks {
                            let last = func.blocks[&use_block].ops.len();
                            for use_op in (0..=last).chain([usize::MAX]) {
                                assert_eq!(
                                    dominance.definition_available(
                                        definition_block,
                                        definition_op,
                                        use_block,
                                        use_op,
                                    ),
                                    graph.dominates(defined, (use_block, use_op.min(last))),
                                    "seed {seed}: {defined:?} before {use_block:?}/{use_op}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}
