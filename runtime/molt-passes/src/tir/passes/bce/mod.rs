//! Bounds Check Elimination (BCE) pass for TIR - Tier-0 substrate **S6** consumer.
//!
//! Annotates `Index` / `StoreIndex` operations that are provably
//! bounds-check-safe by adding a `"bce_safe"` attribute (`AttrValue::Bool(true)`).
//! Backend codegen tests for this attribute and emits a straight-line element
//! access with **no bounds check** (see `native_backend::function_compiler` -
//! the `bce_safe` fast paths). A false `bce_safe` is therefore a *silent
//! out-of-bounds memory access*, not a panic, so the proof obligation is
//! absolute.
//!
//! ## Sole proof source: the value-range analysis (S6)
//!
//! All range/length reasoning lives in the [`ValueRange`](super::value_range)
//! analysis (built on [`ScalarEvolution`](super::scev)). This pass is a thin
//! consumer: for each indexing op it asks
//! [`ValueRangeResult::proves_index_in_bounds`] (numeric `0 <= i < len`) and
//! [`ValueRangeResult::proves_index_lt_len_symbolically`] (the
//! `while i < len(c): c[i]` shape where the length is a non-constant SSA value).
//! Both queries are **conservative over-approximations**: they return `true`
//! only when safety is *proven*, and `false` on any uncertainty.
//!
//! This replaces the former in-pass `RangeFact` / `GuardFact` / `KnownLength` /
//! `AddConst` lattices (deleted): range facts, induction-variable ranges,
//! container lengths and guard narrowing are now the value-range analysis's
//! single responsibility, shared with every other range consumer.
//!
//! The loop structure still comes from the S1 [`LoopForest`] analysis (used
//! transitively inside the value-range computation), so this pass - like LICM -
//! reasons over the one sound natural-loop definition (structurally hardening
//! the old ad-hoc loop-body scan, gap-analysis item C1).

use crate::tir::analysis::AnalysisManager;
use crate::tir::blocks::BlockId;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode};

use super::PassStats;
use super::value_range::ValueRange;

/// Bounds Check Elimination pass.
///
/// Marks `Index` / `StoreIndex` ops whose index is provably in
/// `[0, len(container))` via the value-range analysis. Returns [`PassStats`]
/// counting how many ops were annotated (`values_changed`).
pub fn run(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats {
    let mut stats = PassStats {
        name: "bce",
        ..Default::default()
    };

    // The value-range analysis owns all range/length reasoning (constants,
    // induction-variable ranges from SCEV, container lengths, and edge-sensitive
    // guard narrowing). Clone it so we can take `&mut func.blocks` below; the
    // analysis is a pure function of the (here unchanged) function.
    let vr = am.get::<ValueRange>(func).clone();

    let block_ids: Vec<BlockId> = func.blocks.keys().copied().collect();

    for bid in &block_ids {
        let Some(block) = func.blocks.get_mut(bid) else {
            continue;
        };
        for op in block.ops.iter_mut() {
            if op.opcode != OpCode::Index && op.opcode != OpCode::StoreIndex {
                continue;
            }
            // Idempotent: never re-mark (and never *un*-mark): a previously
            // proven-safe op stays safe.
            if op.attrs.contains_key("bce_safe") {
                continue;
            }
            let Some(&container) = op.operands.first() else {
                continue;
            };
            let Some(&index) = op.operands.get(1) else {
                continue;
            };

            // BCE-only conservative proof: carrier facts are deliberately
            // excluded so a full-range raw int can never elide bounds checks.
            let proven = vr.proves_index_in_bounds_conservatively(*bid, container, index);

            if proven {
                op.attrs
                    .insert("bce_safe".to_string(), AttrValue::Bool(true));
                stats.values_changed += 1;
            }
        }
    }

    stats
}

#[cfg(test)]
mod tests;
