use std::collections::HashMap;

use crate::tir::blocks::Terminator;
use crate::tir::values::ValueId;

/// Collect all ValueIds used in a terminator.
pub(super) fn terminator_values(term: &Terminator) -> Vec<ValueId> {
    match term {
        Terminator::Branch { args, .. } => args.clone(),
        Terminator::CondBranch {
            cond,
            then_args,
            else_args,
            ..
        } => {
            let mut v = vec![*cond];
            v.extend_from_slice(then_args);
            v.extend_from_slice(else_args);
            v
        }
        Terminator::Switch {
            value,
            cases,
            default_args,
            ..
        } => {
            let mut v = vec![*value];
            for (_, _, args) in cases {
                v.extend_from_slice(args);
            }
            v.extend_from_slice(default_args);
            v
        }
        Terminator::StateDispatch {
            cases,
            default_args,
            ..
        } => {
            let mut v = Vec::new();
            for (_, _, args) in cases {
                v.extend_from_slice(args);
            }
            v.extend_from_slice(default_args);
            v
        }
        Terminator::Return { values } => values.clone(),
        Terminator::Unreachable => vec![],
    }
}

/// Replace ValueIds in a terminator according to the replacement map.
pub(super) fn replace_in_terminator(
    term: &mut Terminator,
    replacements: &HashMap<ValueId, ValueId>,
) {
    let replace = |v: &mut ValueId| {
        if let Some(&r) = replacements.get(v) {
            *v = r;
        }
    };

    match term {
        Terminator::Branch { args, .. } => {
            for a in args {
                replace(a);
            }
        }
        Terminator::CondBranch {
            cond,
            then_args,
            else_args,
            ..
        } => {
            replace(cond);
            for a in then_args {
                replace(a);
            }
            for a in else_args {
                replace(a);
            }
        }
        Terminator::Switch {
            value,
            cases,
            default_args,
            ..
        } => {
            replace(value);
            for (_, _, args) in cases {
                for a in args {
                    replace(a);
                }
            }
            for a in default_args {
                replace(a);
            }
        }
        Terminator::StateDispatch {
            cases,
            default_args,
            ..
        } => {
            for (_, _, args) in cases {
                for a in args {
                    replace(a);
                }
            }
            for a in default_args {
                replace(a);
            }
        }
        Terminator::Return { values } => {
            for v in values {
                replace(v);
            }
        }
        Terminator::Unreachable => {}
    }
}
