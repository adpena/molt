use crate::tir::blocks::BlockId;
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    ExceptionRegionNestingRole, opcode_exception_region_nesting_role_table,
};

use super::super::PassStats;
use super::super::effects::op_may_throw;
use super::super::reachability::metadata_preserving_reachable_blocks;
use super::classify::op_is_side_effecting;
use super::uses::build_use_counts;

/// Remove dead operations (and unreachable blocks) from `func`.
///
/// An operation is dead when:
///   - all of its result values have use-count 0, AND
///   - its opcode is not side-effecting.
///
/// When `func.has_exception_handling` is set, ops inside try regions that
/// may throw are conservatively kept alive (they could transfer control to
/// an exception handler whose side effects must be preserved).
///
/// Returns statistics about the changes made.
pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "dce",
        ..Default::default()
    };

    let has_eh = func.has_exception_handling;

    // --- Phase 1: remove unreachable blocks ---
    let reachable = metadata_preserving_reachable_blocks(func);
    let unreachable: Vec<BlockId> = func
        .blocks
        .keys()
        .copied()
        .filter(|id| !reachable.contains(id))
        .collect();
    for id in &unreachable {
        func.blocks.remove(id);
        stats.ops_removed += 1; // count the block removal as one unit
    }

    // --- Phase 2: iterative dead-op removal ---

    for _round in 0..10 {
        let mut uses = build_use_counts(func);

        // Collect block ids to iterate (avoids borrow issues).
        let block_ids: Vec<BlockId> = func.blocks.keys().copied().collect();

        // Note: removed_this_round is cumulative across all blocks in this round.
        // The retain is harmless on blocks with no removals.
        let mut removed_this_round = 0usize;

        for bid in &block_ids {
            let block = match func.blocks.get_mut(bid) {
                Some(b) => b,
                None => continue,
            };

            // Track try-region nesting depth within this block.
            // Ops between TryStart..TryEnd are "inside a try region".
            let mut try_depth = Vec::with_capacity(block.ops.len());
            let mut depth: u32 = 0;
            for op in &block.ops {
                match opcode_exception_region_nesting_role_table(op.opcode) {
                    ExceptionRegionNestingRole::Enter => {
                        try_depth.push(depth);
                        depth += 1;
                    }
                    ExceptionRegionNestingRole::Exit => {
                        depth = depth.saturating_sub(1);
                        try_depth.push(depth);
                    }
                    ExceptionRegionNestingRole::None => {
                        try_depth.push(depth);
                    }
                }
            }

            // Walk ops in reverse order so that cascading removals within
            // a single pass are applied greedily.
            let mut to_keep: Vec<bool> = vec![true; block.ops.len()];

            for i in (0..block.ops.len()).rev() {
                let op = &block.ops[i];
                if op_is_side_effecting(op) {
                    continue;
                }

                // When inside a try region, conservatively keep ops that
                // may throw: they represent implicit edges to the handler.
                if has_eh && try_depth[i] > 0 && op_may_throw(op) {
                    continue;
                }

                // Check whether every result is dead.
                let all_dead = op
                    .results
                    .iter()
                    .all(|v| uses.get(v).copied().unwrap_or(0) == 0);

                if all_dead {
                    // Mark for removal and release operand uses so that
                    // upstream ops in this same block may become dead too.
                    to_keep[i] = false;
                    for v in &op.operands {
                        let count = uses.entry(*v).or_insert(0);
                        if *count > 0 {
                            *count -= 1;
                        }
                    }
                    removed_this_round += 1;
                }
            }

            if removed_this_round > 0 {
                // Drain ops that were marked dead.
                let mut keep_iter = to_keep.iter();
                block.ops.retain(|_| *keep_iter.next().unwrap());
            }
        }

        stats.ops_removed += removed_this_round;

        if removed_this_round == 0 {
            break; // fixpoint reached
        }
    }

    stats
}
