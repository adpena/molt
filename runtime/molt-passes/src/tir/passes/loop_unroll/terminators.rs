//! Pure CFG-terminator utilities shared by the loop-unroll transform.
//!
//! These are representation-only helpers over [`Terminator`]: they read or
//! rewrite the value references and successor edges of a terminator without any
//! knowledge of the unroll cost model or loop recognition. Keeping them in a
//! leaf module isolates the terminator match arms from the transform logic in
//! [`super`].

use std::collections::HashMap;

use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::values::ValueId;

/// Every `ValueId` referenced by a terminator (condition + all branch args).
pub(super) fn terminator_value_refs(term: &Terminator) -> Vec<ValueId> {
    let mut refs = Vec::new();
    match term {
        Terminator::Branch { args, .. } => refs.extend(args.iter().copied()),
        Terminator::CondBranch {
            cond,
            then_args,
            else_args,
            ..
        } => {
            refs.push(*cond);
            refs.extend(then_args.iter().copied());
            refs.extend(else_args.iter().copied());
        }
        Terminator::Switch {
            value,
            cases,
            default_args,
            ..
        } => {
            refs.push(*value);
            for (_, _, args) in cases {
                refs.extend(args.iter().copied());
            }
            refs.extend(default_args.iter().copied());
        }
        // `StateDispatch` has no condition value; only its per-edge args.
        Terminator::StateDispatch {
            cases,
            default_args,
            ..
        } => {
            for (_, _, args) in cases {
                refs.extend(args.iter().copied());
            }
            refs.extend(default_args.iter().copied());
        }
        Terminator::Return { values } => refs.extend(values.iter().copied()),
        Terminator::Unreachable => {}
    }
    refs
}

/// Replace every value reference in `term` (condition + all branch args) that
/// appears as a key in `subst` with its mapped value.
pub(super) fn substitute_terminator_values(
    term: &mut Terminator,
    subst: &HashMap<ValueId, ValueId>,
) {
    fn remap(v: &mut ValueId, subst: &HashMap<ValueId, ValueId>) {
        if let Some(&nv) = subst.get(v) {
            *v = nv;
        }
    }
    match term {
        Terminator::Branch { args, .. } => {
            for v in args {
                remap(v, subst);
            }
        }
        Terminator::CondBranch {
            cond,
            then_args,
            else_args,
            ..
        } => {
            remap(cond, subst);
            for v in then_args.iter_mut().chain(else_args.iter_mut()) {
                remap(v, subst);
            }
        }
        Terminator::Switch {
            value,
            cases,
            default_args,
            ..
        } => {
            remap(value, subst);
            for (_, _, args) in cases.iter_mut() {
                for v in args {
                    remap(v, subst);
                }
            }
            for v in default_args {
                remap(v, subst);
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
                    remap(v, subst);
                }
            }
            for v in default_args {
                remap(v, subst);
            }
        }
        Terminator::Return { values } => {
            for v in values {
                remap(v, subst);
            }
        }
        Terminator::Unreachable => {}
    }
}

/// Extract the args passed to `header` from a predecessor terminator.
pub(super) fn header_args_from(term: &Terminator, header: BlockId) -> Option<&[ValueId]> {
    match term {
        Terminator::Branch { target, args } if *target == header => Some(args.as_slice()),
        Terminator::CondBranch {
            then_block,
            then_args,
            else_block,
            else_args,
            ..
        } => {
            if *then_block == header {
                Some(then_args.as_slice())
            } else if *else_block == header {
                Some(else_args.as_slice())
            } else {
                None
            }
        }
        Terminator::Switch {
            cases,
            default,
            default_args,
            ..
        } => {
            if *default == header {
                Some(default_args.as_slice())
            } else {
                cases.iter().find_map(|(_, b, args)| {
                    if *b == header {
                        Some(args.as_slice())
                    } else {
                        None
                    }
                })
            }
        }
        _ => None,
    }
}

/// Returns `true` if the terminator references `target` as any successor.
pub(super) fn branches_to(term: &Terminator, target: BlockId) -> bool {
    match term {
        Terminator::Branch { target: t, .. } => *t == target,
        Terminator::CondBranch {
            then_block,
            else_block,
            ..
        } => *then_block == target || *else_block == target,
        Terminator::Switch { cases, default, .. } => {
            *default == target || cases.iter().any(|(_, b, _)| *b == target)
        }
        _ => false,
    }
}

/// Replace every successor reference to `from` with `to` in `term`. The landing
/// block has zero block arguments by construction, so we MUST also drop any
/// argument list that was being forwarded to `from` to keep TIR verification
/// (block-arg arity match) sound.
pub(super) fn redirect_terminator(term: &mut Terminator, from: BlockId, to: BlockId) {
    match term {
        Terminator::Branch { target, args } if *target == from => {
            *target = to;
            args.clear();
        }
        Terminator::CondBranch {
            then_block,
            then_args,
            else_block,
            else_args,
            ..
        } => {
            if *then_block == from {
                *then_block = to;
                then_args.clear();
            }
            if *else_block == from {
                *else_block = to;
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
                if *b == from {
                    *b = to;
                    args.clear();
                }
            }
            if *default == from {
                *default = to;
                default_args.clear();
            }
        }
        _ => {}
    }
}
