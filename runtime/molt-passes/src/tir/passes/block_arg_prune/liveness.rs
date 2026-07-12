use std::collections::{BTreeMap, HashMap, HashSet};

use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::dominators;
use crate::tir::function::TirFunction;
use crate::tir::values::ValueId;

use super::edge::exception_transfer_target;

fn real_uses_in_terminator(term: &Terminator, uses: &mut HashSet<ValueId>) {
    match term {
        Terminator::Branch { .. } => {}
        Terminator::CondBranch { cond, .. } => {
            uses.insert(*cond);
        }
        Terminator::Switch { value, .. } => {
            uses.insert(*value);
        }
        Terminator::StateDispatch { .. } => {}
        Terminator::Return { values } => {
            uses.extend(values.iter().copied());
        }
        Terminator::Unreachable => {}
    }
}

fn block_arg_ids(func: &TirFunction) -> HashSet<ValueId> {
    func.blocks
        .values()
        .flat_map(|block| block.args.iter().map(|arg| arg.id))
        .collect()
}

fn real_used_values(func: &TirFunction) -> HashSet<ValueId> {
    let mut uses = HashSet::new();
    for block in func.blocks.values() {
        for op in &block.ops {
            if !dominators::is_exception_transfer_edge(op.opcode) {
                uses.extend(op.operands.iter().copied());
            }
        }
        real_uses_in_terminator(&block.terminator, &mut uses);
    }
    uses
}

fn target_arg_is_live(
    func: &TirFunction,
    live: &HashSet<ValueId>,
    target: BlockId,
    arg_index: usize,
) -> bool {
    func.blocks
        .get(&target)
        .and_then(|block| block.args.get(arg_index))
        .is_some_and(|arg| live.contains(&arg.id))
}

fn propagate_edge_payload_liveness(
    func: &TirFunction,
    live: &mut HashSet<ValueId>,
    target: BlockId,
    args: &[ValueId],
) -> bool {
    let mut changed = false;
    for (idx, &arg) in args.iter().enumerate() {
        if target_arg_is_live(func, live, target, idx) {
            changed |= live.insert(arg);
        }
    }
    changed
}

fn propagate_terminator_liveness(
    func: &TirFunction,
    live: &mut HashSet<ValueId>,
    term: &Terminator,
) -> bool {
    match term {
        Terminator::Branch { target, args } => {
            propagate_edge_payload_liveness(func, live, *target, args)
        }
        Terminator::CondBranch {
            then_block,
            then_args,
            else_block,
            else_args,
            ..
        } => {
            propagate_edge_payload_liveness(func, live, *then_block, then_args)
                | propagate_edge_payload_liveness(func, live, *else_block, else_args)
        }
        Terminator::Switch {
            cases,
            default,
            default_args,
            ..
        } => {
            let mut changed = propagate_edge_payload_liveness(func, live, *default, default_args);
            for (_, target, args) in cases {
                changed |= propagate_edge_payload_liveness(func, live, *target, args);
            }
            changed
        }
        Terminator::StateDispatch {
            cases,
            default,
            default_args,
            ..
        } => {
            let mut changed = propagate_edge_payload_liveness(func, live, *default, default_args);
            for (_, target, args) in cases {
                changed |= propagate_edge_payload_liveness(func, live, *target, args);
            }
            changed
        }
        Terminator::Return { .. } | Terminator::Unreachable => false,
    }
}

fn propagate_exception_liveness(
    func: &TirFunction,
    live: &mut HashSet<ValueId>,
    label_to_block: &HashMap<i64, BlockId>,
) -> bool {
    let mut changed = false;
    for block in func.blocks.values() {
        for op in &block.ops {
            let Some(target) = exception_transfer_target(op.opcode, &op.attrs, label_to_block)
            else {
                continue;
            };
            changed |= propagate_edge_payload_liveness(func, live, target, &op.operands);
        }
    }
    changed
}

fn live_values(func: &TirFunction) -> HashSet<ValueId> {
    let block_args = block_arg_ids(func);
    let mut live: HashSet<ValueId> = real_used_values(func)
        .into_iter()
        .filter(|value| block_args.contains(value))
        .collect();
    let label_to_block = dominators::exception_label_to_block(func);

    loop {
        let mut changed = false;
        for block in func.blocks.values() {
            changed |= propagate_terminator_liveness(func, &mut live, &block.terminator);
        }
        changed |= propagate_exception_liveness(func, &mut live, &label_to_block);
        if !changed {
            return live;
        }
    }
}

pub(super) fn dead_arg_positions(func: &TirFunction) -> BTreeMap<BlockId, Vec<usize>> {
    let live = live_values(func);
    let mut prune = BTreeMap::new();
    for (&bid, block) in &func.blocks {
        if bid == func.entry_block || block.args.is_empty() {
            continue;
        }
        let dead: Vec<usize> = block
            .args
            .iter()
            .enumerate()
            .filter_map(|(idx, arg)| (!live.contains(&arg.id)).then_some(idx))
            .collect();
        if !dead.is_empty() {
            prune.insert(bid, dead);
        }
    }
    prune
}
