use crate::tir::function::TirFunction;
use crate::tir::values::ValueId;

use super::super::PassStats;
use super::edge::{prune_exception_payloads, prune_terminator_payloads, retain_positions_not_in};
use super::liveness::dead_arg_positions;

/// Remove block-argument lanes whose SSA value is unused inside the target
/// block, updating all edge payloads that bind the target's remaining args.
///
/// The pass iterates to a fixed point because pruning one block's arg can make a
/// predecessor block arg dead when that value was only forwarded through the
/// removed lane.
pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "block_arg_prune",
        ..Default::default()
    };

    loop {
        let prune = dead_arg_positions(func);
        if prune.is_empty() {
            break;
        }

        let mut removed_block_args = 0usize;
        for (bid, dead_positions) in &prune {
            let Some(block) = func.blocks.get_mut(bid) else {
                continue;
            };
            let dead_values: Vec<ValueId> = dead_positions
                .iter()
                .filter_map(|idx| block.args.get(*idx).map(|arg| arg.id))
                .collect();
            removed_block_args += retain_positions_not_in(&mut block.args, dead_positions);
            for value in dead_values {
                func.value_types.remove(&value);
            }
        }

        let mut removed_edge_args = 0usize;
        for block in func.blocks.values_mut() {
            removed_edge_args += prune_terminator_payloads(&mut block.terminator, &prune);
        }
        removed_edge_args += prune_exception_payloads(func, &prune);

        stats.values_changed += removed_block_args + removed_edge_args;
    }

    stats
}
