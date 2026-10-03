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

    /// Project before-operation nodes to the legacy public operation-index
    /// view. Both existing lifecycle and loop-preheader consumers use this same
    /// execution graph; only hidden result and invocation nodes are removed.
    pub fn operation_dominators(&self) -> Vec<Option<usize>> {
        (0..self.operation_count)
            .map(|op| {
                let mut parent = self
                    .dominance
                    .immediate_dominator(SimpleProgramPoint::Before(op).node());
                while let Some(node) = parent {
                    if node == 0 {
                        return None;
                    }
                    let parent_op = (node - 1) / 2;
                    if parent_op != op {
                        return Some(parent_op);
                    }
                    parent = self.dominance.immediate_dominator(node);
                }
                None
            })
            .collect()
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
