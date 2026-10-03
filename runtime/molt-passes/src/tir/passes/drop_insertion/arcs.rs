use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::dominators::exception_edge_binds_handler_arguments;
use crate::tir::ops::AttrValue;
use crate::tir::values::ValueId;

/// A stable identifier for ONE outgoing arc of a terminator, so the mixed-
/// ownership-phi retain can retarget exactly that arc when splitting a critical
/// edge (two arcs to the same block with different args must be distinguishable).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum ArcDescriptor {
    /// The single arc of an unconditional `Branch`.
    Branch,
    /// The `then` arc of a `CondBranch`.
    CondThen,
    /// The `else` arc of a `CondBranch`.
    CondElse,
    /// The case arc at `cases[index]` of a `Switch`.
    SwitchCase(usize),
    /// The `default` arc of a `Switch`.
    SwitchDefault,
}

/// One outgoing arc of a block's terminator: which target it goes to, the args it
/// forwards, and a [`ArcDescriptor`] that pins it for retargeting.
pub(super) struct Arc {
    pub(super) descriptor: ArcDescriptor,
    pub(super) target: BlockId,
    pub(super) args: Vec<ValueId>,
}

impl Arc {
    /// A self-loop arc whose source block is also its target (the latch IS the
    /// header) — treated as ambiguous for IncRef placement, since a
    /// before-terminator IncRef on such an arc would sit on the in-block
    /// straight-line path that the body's drops also traverse. Splitting isolates
    /// the retain onto the edge. `pred` is the block the arc originates from.
    pub(super) fn is_self_loop_into_own_phi(&self, pred: BlockId) -> bool {
        self.target == pred
    }
}

/// Enumerate every outgoing arc of `term` with its forwarding args and descriptor.
pub(super) fn terminator_arcs(term: &Terminator) -> Vec<Arc> {
    match term {
        Terminator::Branch { target, args } => vec![Arc {
            descriptor: ArcDescriptor::Branch,
            target: *target,
            args: args.clone(),
        }],
        Terminator::CondBranch {
            then_block,
            then_args,
            else_block,
            else_args,
            ..
        } => vec![
            Arc {
                descriptor: ArcDescriptor::CondThen,
                target: *then_block,
                args: then_args.clone(),
            },
            Arc {
                descriptor: ArcDescriptor::CondElse,
                target: *else_block,
                args: else_args.clone(),
            },
        ],
        Terminator::Switch {
            cases,
            default,
            default_args,
            ..
        } => {
            let mut out: Vec<Arc> = cases
                .iter()
                .enumerate()
                .map(|(i, (_, b, args))| Arc {
                    descriptor: ArcDescriptor::SwitchCase(i),
                    target: *b,
                    args: args.clone(),
                })
                .collect();
            out.push(Arc {
                descriptor: ArcDescriptor::SwitchDefault,
                target: *default,
                args: default_args.clone(),
            });
            out
        }
        // Resume and initial-entry edges share ordinary edge-exact ownership.
        Terminator::StateDispatch {
            cases,
            default,
            default_args,
        } => {
            let mut out: Vec<Arc> = cases
                .iter()
                .enumerate()
                .map(|(i, (_, b, args))| Arc {
                    descriptor: ArcDescriptor::SwitchCase(i),
                    target: *b,
                    args: args.clone(),
                })
                .collect();
            out.push(Arc {
                descriptor: ArcDescriptor::SwitchDefault,
                target: *default,
                args: default_args.clone(),
            });
            out
        }
        Terminator::Return { .. } | Terminator::Unreachable => vec![],
    }
}

/// Retarget exactly the arc named by `desc` to `new_target`, and CLEAR that arc's
/// forwarded args (the inserted edge-split block now supplies them via its own
/// `Branch`). Used to splice a critical-edge-split block onto one arc.
pub(super) fn retarget_arc(term: &mut Terminator, desc: &ArcDescriptor, new_target: BlockId) {
    match (term, desc) {
        (Terminator::Branch { target, args }, ArcDescriptor::Branch) => {
            *target = new_target;
            args.clear();
        }
        (
            Terminator::CondBranch {
                then_block,
                then_args,
                ..
            },
            ArcDescriptor::CondThen,
        ) => {
            *then_block = new_target;
            then_args.clear();
        }
        (
            Terminator::CondBranch {
                else_block,
                else_args,
                ..
            },
            ArcDescriptor::CondElse,
        ) => {
            *else_block = new_target;
            else_args.clear();
        }
        (Terminator::Switch { cases, .. }, ArcDescriptor::SwitchCase(i)) => {
            let (_, target, args) = cases
                .get_mut(*i)
                .expect("DropInsertion cannot retarget an absent case arc");
            *target = new_target;
            args.clear();
        }
        (
            Terminator::Switch {
                default,
                default_args,
                ..
            },
            ArcDescriptor::SwitchDefault,
        ) => {
            *default = new_target;
            default_args.clear();
        }
        // Resume dispatch uses the same edge-exact ownership as other switches.
        (Terminator::StateDispatch { cases, .. }, ArcDescriptor::SwitchCase(i)) => {
            let (_, target, args) = cases
                .get_mut(*i)
                .expect("DropInsertion cannot retarget an absent case arc");
            *target = new_target;
            args.clear();
        }
        (
            Terminator::StateDispatch {
                default,
                default_args,
                ..
            },
            ArcDescriptor::SwitchDefault,
        ) => {
            *default = new_target;
            default_args.clear();
        }
        // The descriptor came from this terminator, and nothing rewrites the
        // terminator before its split is applied. Losing a split drops the
        // retains and releases planned for that arc: an owned phi then holds a
        // borrowed reference, or the arc's owner leaks. Never emit code after
        // that invariant fails.
        (term, desc) => panic!("DropInsertion arc {desc:?} does not match terminator {term:?}"),
    }
}

/// A critical-edge split to materialize: insert a fresh block holding `retains`
/// IncRefs + a `Branch(target, args)`, and retarget `pred`'s `arc` to it.
pub(super) struct EdgeSplit {
    pub(super) pred: BlockId,
    pub(super) arc: ArcDescriptor,
    pub(super) target: BlockId,
    pub(super) args: Vec<ValueId>,
    pub(super) retains: Vec<ValueId>,
    pub(super) releases: Vec<ValueId>,
}

/// Plan RC operations on one arc. Every plan for the same arc shares one split
/// block, so each must name the arc's own target and payload.
pub(super) fn push_edge_split(
    splits: &mut Vec<EdgeSplit>,
    pred: BlockId,
    arc: ArcDescriptor,
    target: BlockId,
    args: Vec<ValueId>,
    retains: Vec<ValueId>,
    releases: Vec<ValueId>,
) {
    if let Some(existing) = splits
        .iter_mut()
        .find(|split| split.pred == pred && split.arc == arc)
    {
        assert_eq!(
            existing.target, target,
            "DropInsertion split target changed"
        );
        assert_eq!(existing.args, args, "DropInsertion split payload changed");
        existing.retains.extend(retains);
        existing.releases.extend(releases);
        return;
    }
    splits.push(EdgeSplit {
        pred,
        arc,
        target,
        args,
        retains,
        releases,
    });
}

pub(super) struct ExceptionArc {
    pub(super) op_index: usize,
    pub(super) target: BlockId,
    pub(super) args: Vec<ValueId>,
}

pub(super) fn exception_arcs_for_block(
    label_to_block: &std::collections::HashMap<i64, BlockId>,
    block: &TirBlock,
) -> Vec<ExceptionArc> {
    block
        .ops
        .iter()
        .enumerate()
        .filter_map(|(op_index, op)| {
            if !crate::tir::dominators::is_exception_transfer_edge(op.opcode) {
                return None;
            }
            let target_label = match op.attrs.get("value") {
                Some(AttrValue::Int(label)) => *label,
                _ => return None,
            };
            let target = *label_to_block.get(&target_label)?;
            Some(ExceptionArc {
                op_index,
                target,
                args: op.operands.clone(),
            })
        })
        .collect()
}

/// Where a control arc leaves its source block. A terminator arc binds its
/// arguments at the source's exit. An observation's arc binds them at the
/// observation, and only when it raises; the normal continuation keeps every
/// owner the arc would move. A region registration's arc keeps its handler
/// reachable, but control never leaves through it, so it binds nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum ArcSite {
    Terminator(ArcDescriptor),
    /// The observation (`CheckException`) at this operation.
    Exception(usize),
    /// The region registration (`TryStart`) at this operation.
    Registration(usize),
}

impl ArcSite {
    /// The operation the arc leaves at; `usize::MAX` is the terminator
    /// boundary.
    pub(super) fn position(self) -> usize {
        match self {
            ArcSite::Terminator(_) => usize::MAX,
            ArcSite::Exception(op_index) | ArcSite::Registration(op_index) => op_index,
        }
    }
}

/// One canonical control arc: where it leaves, its target, and the values it
/// binds to the target's block arguments, in order.
pub(super) struct ControlArc {
    pub(super) site: ArcSite,
    pub(super) target: BlockId,
    pub(super) args: Vec<ValueId>,
}

/// Every control arc leaving `block`: its exception arcs in operation order,
/// then its terminator arcs. These are the edges that liveness, program-point
/// dominance and exceptional landings read. An exception arc is an
/// observation's when its operation binds the handler's arguments, and a region
/// registration's otherwise.
pub(super) fn control_arcs(
    label_to_block: &std::collections::HashMap<i64, BlockId>,
    block: &TirBlock,
) -> Vec<ControlArc> {
    let exceptional = exception_arcs_for_block(label_to_block, block)
        .into_iter()
        .map(|arc| {
            let site = if exception_edge_binds_handler_arguments(block.ops[arc.op_index].opcode) {
                ArcSite::Exception(arc.op_index)
            } else {
                ArcSite::Registration(arc.op_index)
            };
            ControlArc {
                site,
                target: arc.target,
                args: arc.args,
            }
        });
    let normal = terminator_arcs(&block.terminator)
        .into_iter()
        .map(|arc| ControlArc {
            site: ArcSite::Terminator(arc.descriptor),
            target: arc.target,
            args: arc.args,
        });
    exceptional.chain(normal).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic(expected = "does not match terminator")]
    fn retarget_rejects_mismatched_descriptor() {
        let mut term = Terminator::Return { values: vec![] };
        retarget_arc(&mut term, &ArcDescriptor::Branch, BlockId(1));
    }

    #[test]
    fn retarget_rejects_absent_cases_for_value_and_resume_dispatch() {
        let terminators = [
            Terminator::Switch {
                value: ValueId(0),
                cases: vec![],
                default: BlockId(0),
                default_args: vec![],
            },
            Terminator::StateDispatch {
                cases: vec![],
                default: BlockId(0),
                default_args: vec![],
            },
        ];
        for mut term in terminators {
            let retargeted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                retarget_arc(&mut term, &ArcDescriptor::SwitchCase(0), BlockId(1));
            }));
            assert!(
                retargeted.is_err(),
                "an absent case arc must not be skipped"
            );
        }
    }

    #[test]
    #[should_panic(expected = "DropInsertion split payload changed")]
    fn one_arc_rejects_plans_with_different_payloads() {
        let mut splits = Vec::new();
        let (pred, target) = (BlockId(0), BlockId(1));
        push_edge_split(
            &mut splits,
            pred,
            ArcDescriptor::Branch,
            target,
            vec![ValueId(2)],
            vec![],
            vec![ValueId(3)],
        );
        push_edge_split(
            &mut splits,
            pred,
            ArcDescriptor::Branch,
            target,
            vec![ValueId(4)],
            vec![ValueId(4)],
            vec![],
        );
    }
}
