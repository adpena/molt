//! Deforestation / iterator-fusion sub-pass.
//!
//! Fuses generator/iterator chains that feed a fusable builtin consumer
//! (`sum`/`any`/`all`/`min`/`max`/`list`/`len`/`set`/`tuple`/`sorted`/
//! `reversed`) into single loops, eliminating the intermediate data
//! structures. See the module-level docs on [`super`].

use std::collections::HashMap;

use super::super::PassStats;
use crate::tir::blocks::BlockId;
use crate::tir::function::TirFunction;
use crate::tir::ops::OpCode;
use crate::tir::values::ValueId;

mod chains;
mod collections;
mod model;
mod reductions;

use chains::find_fusable_chains;
use collections::{fuse_len, fuse_list, fuse_reversed, fuse_set, fuse_sorted, fuse_tuple};
use model::FusableBuiltin;
#[cfg(test)]
pub(super) use model::is_fusable_body;
use reductions::{fuse_any_all, fuse_min_max, fuse_sum};

/// Detect and fuse iterator/generator chains into single loops.
///
/// Patterns detected:
/// 1. `sum(genexpr)` → accumulator loop
/// 2. `list(genexpr)` → preallocated list + append loop
/// 3. `map(f, iter)` → fused apply-in-loop
/// 4. `filter(pred, iter)` → fused guard-in-loop
/// 5. `any(genexpr)` / `all(genexpr)` → early-exit loop
/// 6. `min(genexpr)` / `max(genexpr)` → tracking loop
///
/// Purity requirement: only fuses when the body is provably pure
/// (no side effects, no exceptions beyond what unfused version would raise).
pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "deforestation",
        ..Default::default()
    };

    // Phase 1: Build a map from ValueId → defining op location, and a map from
    // GetIter results to their source iterables.
    let mut def_map: HashMap<ValueId, (BlockId, usize)> = HashMap::new();
    let mut get_iter_sources: HashMap<ValueId, ValueId> = HashMap::new();

    for (&bid, block) in &func.blocks {
        for (i, op) in block.ops.iter().enumerate() {
            for &res in &op.results {
                def_map.insert(res, (bid, i));
            }
            if op.opcode == OpCode::GetIter && !op.operands.is_empty() && !op.results.is_empty() {
                get_iter_sources.insert(op.results[0], op.operands[0]);
            }
        }
    }

    // Phase 2: Find fusable chains. We look for CallBuiltin ops where:
    //   - The builtin name is one of our fusable set
    //   - The single argument comes from a ForIter loop
    //   - The loop body is pure
    let chains = find_fusable_chains(func, &def_map, &get_iter_sources);

    // Phase 3: Apply fusion rewrites.
    for chain in chains {
        match chain.builtin {
            FusableBuiltin::Sum => {
                fuse_sum(func, &chain, &mut stats);
            }
            FusableBuiltin::Any => {
                fuse_any_all(func, &chain, true, &mut stats);
            }
            FusableBuiltin::All => {
                fuse_any_all(func, &chain, false, &mut stats);
            }
            FusableBuiltin::Min => {
                fuse_min_max(func, &chain, true, &mut stats);
            }
            FusableBuiltin::Max => {
                fuse_min_max(func, &chain, false, &mut stats);
            }
            FusableBuiltin::List => {
                fuse_list(func, &chain, &mut stats);
            }
            FusableBuiltin::Len => {
                fuse_len(func, &chain, &mut stats);
            }
            FusableBuiltin::Set => {
                fuse_set(func, &chain, &mut stats);
            }
            FusableBuiltin::Tuple => {
                fuse_tuple(func, &chain, &mut stats);
            }
            FusableBuiltin::Sorted => {
                fuse_sorted(func, &chain, &mut stats);
            }
            FusableBuiltin::Reversed => {
                fuse_reversed(func, &chain, &mut stats);
            }
        }
    }

    stats
}
