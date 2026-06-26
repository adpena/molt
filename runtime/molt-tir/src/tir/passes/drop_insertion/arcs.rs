use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::function::TirFunction;
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
        // `StateDispatch` mirrors `Switch`'s arc shape (cases + default).  Reuse
        // the `SwitchCase`/`SwitchDefault` descriptors: `drop_insertion` bails on
        // state-machine functions (the `has_state_machine` guard in `run`), so
        // this arm is unreachable in practice, but keeps the arc model total and
        // correct should that guard ever be lifted for `_poll` bodies.
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
            if let Some((_, b, args)) = cases.get_mut(*i) {
                *b = new_target;
                args.clear();
            }
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
        // `StateDispatch` shares the `SwitchCase`/`SwitchDefault` arc descriptors
        // (see `terminator_arcs`).  Unreachable while `drop_insertion` bails on
        // state machines, but kept total for correctness if that guard is lifted.
        (Terminator::StateDispatch { cases, .. }, ArcDescriptor::SwitchCase(i)) => {
            if let Some((_, b, args)) = cases.get_mut(*i) {
                *b = new_target;
                args.clear();
            }
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
        // Descriptor/terminator mismatch is a logic error — the descriptor was
        // produced from THIS terminator by `terminator_arcs` and the terminator is
        // not mutated between enumeration and retarget. Leave unchanged (fail-
        // closed: a missed retarget keeps the original edge — the IncRef block is
        // then unreachable/dead, a leak at worst, never a UAF).
        _ => {}
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
        debug_assert_eq!(existing.target, target);
        debug_assert_eq!(existing.args, args);
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

pub(super) fn exception_arcs_for_block(func: &TirFunction, block: &TirBlock) -> Vec<ExceptionArc> {
    let label_to_block = crate::tir::dominators::exception_label_to_block(func);
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
