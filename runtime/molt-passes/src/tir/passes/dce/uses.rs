use std::collections::HashMap;

use crate::tir::blocks::Terminator;
use crate::tir::function::TirFunction;
use crate::tir::values::ValueId;

/// Increment the use-count of every ValueId mentioned in a terminator.
fn count_terminator_uses(term: &Terminator, uses: &mut HashMap<ValueId, usize>) {
    match term {
        Terminator::Branch { args, .. } => {
            for v in args {
                *uses.entry(*v).or_insert(0) += 1;
            }
        }
        Terminator::CondBranch {
            cond,
            then_args,
            else_args,
            ..
        } => {
            *uses.entry(*cond).or_insert(0) += 1;
            for v in then_args {
                *uses.entry(*v).or_insert(0) += 1;
            }
            for v in else_args {
                *uses.entry(*v).or_insert(0) += 1;
            }
        }
        Terminator::Switch {
            value,
            cases,
            default_args,
            ..
        } => {
            *uses.entry(*value).or_insert(0) += 1;
            for (_, _, args) in cases {
                for v in args {
                    *uses.entry(*v).or_insert(0) += 1;
                }
            }
            for v in default_args {
                *uses.entry(*v).or_insert(0) += 1;
            }
        }
        // `StateDispatch` has no condition value (the saved state is read from
        // the frame header at codegen time); only its per-edge args are uses.
        Terminator::StateDispatch {
            cases,
            default_args,
            ..
        } => {
            for (_, _, args) in cases {
                for v in args {
                    *uses.entry(*v).or_insert(0) += 1;
                }
            }
            for v in default_args {
                *uses.entry(*v).or_insert(0) += 1;
            }
        }
        Terminator::Return { values } => {
            for v in values {
                *uses.entry(*v).or_insert(0) += 1;
            }
        }
        Terminator::Unreachable => {}
    }
}

/// Build a full use-count map from all ops and terminators in the function.
pub(super) fn build_use_counts(func: &TirFunction) -> HashMap<ValueId, usize> {
    let mut uses: HashMap<ValueId, usize> = HashMap::new();
    for block in func.blocks.values() {
        for op in &block.ops {
            for v in &op.operands {
                *uses.entry(*v).or_insert(0) += 1;
            }
        }
        count_terminator_uses(&block.terminator, &mut uses);
    }
    uses
}
