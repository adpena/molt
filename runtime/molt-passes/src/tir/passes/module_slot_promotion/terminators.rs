//! Pure CFG-terminator utilities shared across the module-slot-promotion
//! transform: successor enumeration, value-use scans, block-arg appends, edge
//! argument extraction, and edge retargeting. Representation-only helpers over
//! [`Terminator`] with no promotion policy of their own.

use std::collections::HashSet;

use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::values::ValueId;

pub(super) fn terminator_successors(term: &Terminator) -> Vec<BlockId> {
    match term {
        Terminator::Branch { target, .. } => vec![*target],
        Terminator::CondBranch {
            then_block,
            else_block,
            ..
        } => vec![*then_block, *else_block],
        Terminator::Switch { cases, default, .. }
        | Terminator::StateDispatch { cases, default, .. } => {
            let mut v: Vec<BlockId> = cases.iter().map(|(_, b, _)| *b).collect();
            v.push(*default);
            v
        }
        Terminator::Return { .. } | Terminator::Unreachable => vec![],
    }
}

pub(super) fn terminator_uses(term: &Terminator, set: &HashSet<ValueId>) -> bool {
    match term {
        Terminator::Branch { args, .. } => args.iter().any(|v| set.contains(v)),
        Terminator::CondBranch {
            cond,
            then_args,
            else_args,
            ..
        } => {
            set.contains(cond)
                || then_args.iter().any(|v| set.contains(v))
                || else_args.iter().any(|v| set.contains(v))
        }
        Terminator::Switch {
            value,
            cases,
            default_args,
            ..
        } => {
            set.contains(value)
                || default_args.iter().any(|v| set.contains(v))
                || cases
                    .iter()
                    .any(|(_, _, args)| args.iter().any(|v| set.contains(v)))
        }
        // `StateDispatch` has no condition value; only its per-edge args.
        Terminator::StateDispatch {
            cases,
            default_args,
            ..
        } => {
            default_args.iter().any(|v| set.contains(v))
                || cases
                    .iter()
                    .any(|(_, _, args)| args.iter().any(|v| set.contains(v)))
        }
        Terminator::Return { values } => values.iter().any(|v| set.contains(v)),
        Terminator::Unreachable => false,
    }
}

/// Retarget every appearance of `header` in `term` to `args`-augmented form:
/// append `extra` to the arg list of each edge into the header.
pub(super) fn append_args_on_edges_to(term: &mut Terminator, header: BlockId, extra: &[ValueId]) {
    match term {
        Terminator::Branch { target, args } if *target == header => {
            args.extend_from_slice(extra);
        }
        Terminator::CondBranch {
            then_block,
            then_args,
            else_block,
            else_args,
            ..
        } => {
            if *then_block == header {
                then_args.extend_from_slice(extra);
            }
            if *else_block == header {
                else_args.extend_from_slice(extra);
            }
        }
        Terminator::Switch {
            cases,
            default,
            default_args,
            ..
        } => {
            for (_, b, args) in cases.iter_mut() {
                if *b == header {
                    args.extend_from_slice(extra);
                }
            }
            if *default == header {
                default_args.extend_from_slice(extra);
            }
        }
        _ => {}
    }
}


pub(super) fn rewrite_terminator_values(term: &mut Terminator, f: &dyn Fn(ValueId) -> ValueId) {
    match term {
        Terminator::Branch { args, .. } => {
            for v in args {
                *v = f(*v);
            }
        }
        Terminator::CondBranch {
            cond,
            then_args,
            else_args,
            ..
        } => {
            *cond = f(*cond);
            for v in then_args.iter_mut().chain(else_args.iter_mut()) {
                *v = f(*v);
            }
        }
        Terminator::Switch {
            value,
            cases,
            default_args,
            ..
        } => {
            *value = f(*value);
            for (_, _, args) in cases.iter_mut() {
                for v in args {
                    *v = f(*v);
                }
            }
            for v in default_args {
                *v = f(*v);
            }
        }
        // `StateDispatch` has no condition value; only its per-edge args.
        Terminator::StateDispatch {
            cases,
            default_args,
            ..
        } => {
            for (_, _, args) in cases.iter_mut() {
                for v in args {
                    *v = f(*v);
                }
            }
            for v in default_args {
                *v = f(*v);
            }
        }
        Terminator::Return { values } => {
            for v in values {
                *v = f(*v);
            }
        }
        Terminator::Unreachable => {}
    }
}

/// The args `term` passes on its edge to `to` (first matching edge).
pub(super) fn edge_args(term: &Terminator, to: BlockId) -> Vec<ValueId> {
    match term {
        Terminator::Branch { target, args } if *target == to => args.clone(),
        Terminator::CondBranch {
            then_block,
            then_args,
            else_block,
            else_args,
            ..
        } => {
            if *then_block == to {
                then_args.clone()
            } else if *else_block == to {
                else_args.clone()
            } else {
                vec![]
            }
        }
        Terminator::Switch {
            cases,
            default,
            default_args,
            ..
        } => cases
            .iter()
            .find(|(_, b, _)| *b == to)
            .map(|(_, _, a)| a.clone())
            .unwrap_or_else(|| {
                if *default == to {
                    default_args.clone()
                } else {
                    vec![]
                }
            }),
        _ => vec![],
    }
}

/// Retarget every edge in `term` from `old` to `new`, clearing the edge args
/// (the new edge block forwards the originals itself).
pub(super) fn retarget_edge(term: &mut Terminator, old: BlockId, new: BlockId) {
    match term {
        Terminator::Branch { target, args } if *target == old => {
            *target = new;
            args.clear();
        }
        Terminator::CondBranch {
            then_block,
            then_args,
            else_block,
            else_args,
            ..
        } => {
            if *then_block == old {
                *then_block = new;
                then_args.clear();
            }
            if *else_block == old {
                *else_block = new;
                else_args.clear();
            }
        }
        Terminator::Switch {
            cases,
            default,
            default_args,
            ..
        } => {
            for (_, b, args) in cases.iter_mut() {
                if *b == old {
                    *b = new;
                    args.clear();
                }
            }
            if *default == old {
                *default = new;
                default_args.clear();
            }
        }
        _ => {}
    }
}

