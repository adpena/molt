use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::values::ValueId;

/// Collect (target_block, arg_values) edges from a terminator.
pub(super) fn collect_branch_edges(block: &TirBlock) -> Vec<(BlockId, Vec<ValueId>)> {
    match &block.terminator {
        Terminator::Branch { target, args } => {
            vec![(*target, args.clone())]
        }
        Terminator::CondBranch {
            then_block,
            then_args,
            else_block,
            else_args,
            ..
        } => {
            vec![
                (*then_block, then_args.clone()),
                (*else_block, else_args.clone()),
            ]
        }
        Terminator::Switch {
            cases,
            default,
            default_args,
            ..
        }
        | Terminator::StateDispatch {
            cases,
            default,
            default_args,
            ..
        } => {
            let mut edges: Vec<(BlockId, Vec<ValueId>)> = cases
                .iter()
                .map(|(_, target, args)| (*target, args.clone()))
                .collect();
            edges.push((*default, default_args.clone()));
            edges
        }
        Terminator::Return { .. } | Terminator::Unreachable => vec![],
    }
}
