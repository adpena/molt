//! Executable SimpleIR definition availability, including invocation custody.
use super::{IndexedDominance, is_simple_exception_observation_kind};
use crate::ir::OpIR;
use crate::tir::cfg::CFG;
use crate::tir::simple_def_use::SimpleDefinitionSite;

#[derive(Debug, Clone, Copy)]
pub enum SimpleProgramPoint {
    Before(usize),
    After(usize),
}

impl SimpleProgramPoint {
    fn node(self) -> usize {
        match self {
            Self::Before(op) => 1 + 2 * op,
            Self::After(op) => 2 + 2 * op,
        }
    }
    pub fn operation(self) -> usize {
        match self {
            Self::Before(op) | Self::After(op) => op,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SimpleExecutionDominance {
    dominance: IndexedDominance,
    operation_count: usize,
}

impl SimpleExecutionDominance {
    pub fn compute(cfg: &CFG, ops: &[OpIR]) -> Self {
        let mut successors = vec![Vec::new(); 1 + 2 * ops.len()];
        if !ops.is_empty() {
            successors[0].push(SimpleProgramPoint::Before(0).node());
        }
        for index in 0..ops.len() {
            successors[SimpleProgramPoint::Before(index).node()]
                .push(SimpleProgramPoint::After(index).node());
        }
        for block in &cfg.blocks {
            for index in block.start_op..block.end_op.saturating_sub(1) {
                successors[SimpleProgramPoint::After(index).node()]
                    .push(SimpleProgramPoint::Before(index + 1).node());
            }
            if block.start_op == block.end_op {
                continue;
            }
            let tail = SimpleProgramPoint::After(block.end_op - 1).node();
            for &target in &cfg.successors[block.id] {
                successors[tail]
                    .push(SimpleProgramPoint::Before(cfg.blocks[target].start_op).node());
            }
        }
        let labels = CFG::label_positions(ops);
        for (index, op) in ops.iter().enumerate() {
            if is_simple_exception_observation_kind(&op.kind)
                && let Some(target) = op.value.and_then(|label| labels.get(&label)).copied()
            {
                // The result is unavailable on an exceptional observation edge.
                successors[SimpleProgramPoint::Before(index).node()]
                    .push(SimpleProgramPoint::Before(target).node());
            }
        }
        for &(from, to, _) in &cfg.state_resume_edges {
            let source = (cfg.blocks[from].start_op..cfg.blocks[from].end_op)
                .find(|&index| ops[index].kind == "state_switch")
                .expect("resume edge must leave its canonical state dispatch");
            successors[SimpleProgramPoint::Before(source).node()]
                .push(SimpleProgramPoint::Before(cfg.blocks[to].start_op).node());
        }
        Self {
            dominance: IndexedDominance::compute(&successors, 0),
            operation_count: ops.len(),
        }
    }

    pub fn definition_available(
        &self,
        definition: SimpleDefinitionSite,
        usage: SimpleProgramPoint,
    ) -> bool {
        let definition = match definition {
            SimpleDefinitionSite::Invocation => 0,
            SimpleDefinitionSite::Operation(index) => SimpleProgramPoint::After(index).node(),
        };
        self.dominance.dominates(definition, usage.node())
    }

    /// Whether execution can reach operation `op`.
    pub fn operation_is_reachable(&self, op: usize) -> bool {
        op < self.operation_count
            && self
                .dominance
                .is_reachable(SimpleProgramPoint::Before(op).node())
    }

    /// Whether every execution path to operation `target` executes `dominator`
    /// first; an operation dominates itself. The operation tree projects the
    /// point tree onto before-operation points: an after-point's only
    /// predecessor is its own before-point, so operation `dominator` is an
    /// ancestor of `target` exactly when its before-point dominates the
    /// target's. The answer is two interval comparisons, never a walk.
    pub fn operation_dominates(&self, dominator: usize, target: usize) -> bool {
        dominator == target
            || (dominator < self.operation_count
                && target < self.operation_count
                && self.dominance.dominates(
                    SimpleProgramPoint::Before(dominator).node(),
                    SimpleProgramPoint::Before(target).node(),
                ))
    }

    /// The nearest operation that dominates `op`, for diagnostics. The first
    /// operation and unreachable operations have none.
    pub fn operation_immediate_dominator(&self, op: usize) -> Option<usize> {
        if op >= self.operation_count {
            return None;
        }
        // A before-point's immediate dominator belongs to another operation,
        // or is the invocation node.
        self.dominance
            .immediate_dominator(SimpleProgramPoint::Before(op).node())
            .filter(|&node| node != 0)
            .map(|node| (node - 1) / 2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn op(kind: &str, value: Option<i64>) -> OpIR {
        OpIR {
            kind: kind.into(),
            value,
            ..OpIR::default()
        }
    }
    #[test]
    fn registration_retains_structure_but_only_observation_exposes_exceptional_result_gap() {
        for kind in ["try_start", "check_exception", "async_work_poll"] {
            let ops = vec![
                op(kind, Some(7)),
                op("jump", Some(7)),
                op("label", Some(7)),
                op("ret_void", None),
            ];
            let cfg = CFG::build(&ops);
            let points = cfg.execution_points(&ops);
            assert!(points.definition_available(
                SimpleDefinitionSite::Invocation,
                SimpleProgramPoint::Before(2)
            ));
            assert_eq!(
                points.definition_available(
                    SimpleDefinitionSite::Operation(0),
                    SimpleProgramPoint::Before(2)
                ),
                kind == "try_start",
                "{kind}"
            );
            assert!(!points.definition_available(
                SimpleDefinitionSite::Operation(0),
                SimpleProgramPoint::Before(0)
            ));
            assert!(points.definition_available(
                SimpleDefinitionSite::Operation(0),
                SimpleProgramPoint::After(0)
            ));
        }
    }
    /// The operation tree as the immediate-dominator chain defines it: walk
    /// from `target` through nearest dominating operations.
    fn chain_dominates(points: &SimpleExecutionDominance, dominator: usize, target: usize) -> bool {
        let mut current = target;
        loop {
            if current == dominator {
                return true;
            }
            match points.operation_immediate_dominator(current) {
                Some(parent) => current = parent,
                None => return false,
            }
        }
    }

    #[test]
    fn operation_intervals_match_the_immediate_dominator_chain() {
        let shapes = [
            // Loop with an early exit and a forward initializer.
            vec![
                op("jump", Some(9)),
                op("label", Some(7)),
                op("ret_void", None),
                op("label", Some(9)),
                op("const", None),
                op("jump", Some(7)),
            ],
            // Observations that leave for a shared handler, and a dead tail.
            vec![
                op("const", None),
                op("check_exception", Some(5)),
                op("const", None),
                op("check_exception", Some(5)),
                op("ret_void", None),
                op("label", Some(5)),
                op("ret_void", None),
                op("const", None),
            ],
            // A registration and a self-entering loop.
            vec![
                op("try_start", Some(3)),
                op("label", Some(2)),
                op("const", None),
                op("jump", Some(2)),
                op("label", Some(3)),
                op("ret_void", None),
            ],
        ];
        for ops in shapes {
            let points = CFG::build(&ops).execution_points(&ops);
            for target in 0..ops.len() {
                // The first operation is always reachable; any other is
                // reachable exactly when it has a dominating operation.
                assert_eq!(
                    points.operation_is_reachable(target),
                    target == 0 || points.operation_immediate_dominator(target).is_some(),
                    "{ops:?}: reachability of {target}"
                );
                for dominator in 0..ops.len() {
                    assert_eq!(
                        points.operation_dominates(dominator, target),
                        chain_dominates(&points, dominator, target),
                        "{ops:?}: {dominator} dom {target}"
                    );
                }
            }
            assert!(!points.operation_is_reachable(ops.len()));
            assert!(!points.operation_dominates(0, ops.len()));
        }
    }

    #[test]
    fn invocation_and_forward_initializer_do_not_become_loop_carried_definitions() {
        let ops = vec![
            op("jump", Some(9)),
            op("label", Some(7)),
            op("ret_void", None),
            op("label", Some(9)),
            op("const", None),
            op("jump", Some(7)),
        ];
        let points = CFG::build(&ops).execution_points(&ops);
        assert!(points.definition_available(
            SimpleDefinitionSite::Operation(4),
            SimpleProgramPoint::Before(1)
        ));
        assert!(!points.definition_available(
            SimpleDefinitionSite::Operation(4),
            SimpleProgramPoint::Before(0)
        ));
        let ops = vec![op("label", Some(7)), op("const", None), op("jump", Some(7))];
        let points = CFG::build(&ops).execution_points(&ops);
        assert!(!points.definition_available(
            SimpleDefinitionSite::Operation(1),
            SimpleProgramPoint::Before(0)
        ));
        assert!(points.definition_available(
            SimpleDefinitionSite::Invocation,
            SimpleProgramPoint::Before(0)
        ));
    }
}
