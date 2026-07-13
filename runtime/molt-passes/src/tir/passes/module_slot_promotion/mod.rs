//! Module-slot promotion — scalar promotion (mem2reg) of module-dict slots
//! across natural loops (the bench_sum 16× root cause; design:
//! `docs/design/foundation/10_module-global-loop-promotion.md`).
//!
//! Module-level Python keeps every loop-carried variable in the module dict:
//! each iteration pays `ModuleGetAttr` + `ModuleSetAttr` (+ the boxed value
//! round-trip) per variable — ~200× the cost of the register-carried local the
//! optimizer already handles 4.6× faster than CPython. This pass rewrites each
//! qualifying loop so promoted slots are carried as **header block arguments**
//! (SSA phis): in-loop reads use the carried value, in-loop writes redefine it,
//! every loop exit stores the final value back once, and every in-loop
//! `CheckException` whose slot state is dirty is routed through a
//! **compensation block** that stores the values live at that program point
//! before continuing to the original handler — an exception observer sees
//! exactly the as-if-stored-per-iteration state (deoptimization-state
//! discipline). After promotion the carried value is an ordinary SSA loop phi,
//! so the existing value-range / `RawI64Safe` machinery applies unchanged —
//! promotion turns module-level loops into the function-local shape the rest of
//! the optimizer is already good at.
//!
//! ## Soundness gates (each refusal is conservative-correct: the loop simply
//! keeps its per-iteration dict traffic)
//!
//! * **Concurrent observers**: CPython permits another *thread* to observe
//!   module globals mid-loop. [`module_has_concurrency_markers`] scans the
//!   whole module for threading reachability (`molt_thread_*` intrinsic name
//!   strings, `threading`/`_thread` imports) and the pass is a module-wide
//!   no-op when any is found. Fail-closed.
//! * **Other dict observers in the loop**: any op whose
//!   [`MemRegion`](super::alias_analysis::MemRegion) may alias
//!   [`MemRegion::ModuleDict`](super::alias_analysis::MemRegion::ModuleDict)
//!   (opaque calls are `GenericHeap` and alias everything) disqualifies the
//!   loop — EXCEPT const-named module get/set ops on the same module object:
//!   module dicts are plain dicts, so ops on *distinct constant keys* are
//!   disjoint (key-precise refinement of the oracle's coarse `ModuleDict`
//!   region).
//! * **Dynamic module access**: a module op with a non-constant name (or a
//!   `ModuleDelGlobal*` / `ModuleGetGlobal`) may touch any ATTR slot → the
//!   containing FUNCTION is skipped entirely. (`ModuleCache*` ops operate on
//!   the separate `sys.modules` registry, not the attr dict — not wildcards.)
//! * **Entry availability (no speculation)**: a slot is promoted only when its
//!   value is available at loop entry without hoisting a may-raise load past
//!   the loop guard: either the **preheader block** ends with a barrier-free
//!   suffix containing a get/set of the slot (its SSA value seeds the phi), or
//!   the slot's first in-loop access is in the **header block** (the header
//!   executes on every entry, so a preheader load raises exactly where the
//!   first iteration's access would have).
//! * **Linear loop body**: every loop block has at most one in-loop successor
//!   (no internal joins), so renaming needs no internal phis — the only join
//!   is the header phi this pass inserts. (Loops with internal control flow
//!   are a later phase; refusal is a perf bail, never a miscompile.)
//! * **No outside uses**: no in-loop module-op result may be used outside the
//!   loop (LCSSA would be needed; refused instead).
//! * **State machines**: functions containing generator/async ops are skipped.

use crate::tir::dominators::{CfgEdgePolicy, build_pred_map_with, compute_idoms_with};
use crate::tir::function::{TirFunction, TirModule};
use crate::tir::op_kinds_generated::{ModuleSlotAccessRole, opcode_module_slot_access_role_table};

use super::alias_analysis::AliasAnalysisResult;

mod gates;
mod loops;
mod promote;
mod terminators;

#[cfg(test)]
mod tests;

use gates::{
    const_str_defs, is_state_machine_op, is_wildcard_module_op, module_has_concurrency_markers,
    single_module_root,
};
use loops::discover_loops;
use promote::promote_loop;

/// Statistics from one [`run_module_slot_promotion`] invocation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromotionStats {
    /// Functions that had at least one loop promoted.
    pub functions_changed: usize,
    /// Total (loop, slot) promotions performed.
    pub slots_promoted: usize,
    /// In-loop `ModuleGetAttr`/`ModuleSetAttr` ops deleted.
    pub ops_eliminated: usize,
}

pub fn run_module_slot_promotion(module: &mut TirModule) -> (PromotionStats, Vec<String>) {
    let mut stats = PromotionStats::default();
    let mut changed_names = Vec::new();
    let mut dbg = DebugLog::from_env();

    if module_has_concurrency_markers(module) {
        dbg.note("module-wide refusal: concurrency markers (threading/_thread import or molt_thread_* call)");
        dbg.flush(&module.name);
        return (stats, changed_names);
    }

    for func in &mut module.functions {
        let promoted = promote_function(func, &mut stats, &mut dbg);
        if promoted {
            stats.functions_changed += 1;
            changed_names.push(func.name.clone());
        }
    }
    dbg.flush(&module.name);
    (stats, changed_names)
}

/// `MOLT_PROMOTE_DEBUG=1` refusal-reason log, written through the debug-artifact
/// channel (`<artifact-dir>/promotion/<module>.txt`) — backend stderr does not
/// surface through the CLI on successful builds, artifacts do. The instrument
/// that keeps a silently-inert activation diagnosable (the L4 / needs_inlining
/// lesson, institutionalized).
pub(super) struct DebugLog {
    lines: Option<Vec<String>>,
}

impl DebugLog {
    pub(super) fn from_env() -> Self {
        Self {
            lines: (std::env::var("MOLT_PROMOTE_DEBUG").as_deref() == Ok("1")).then(Vec::new),
        }
    }
    pub(super) fn note(&mut self, msg: impl Into<String>) {
        if let Some(lines) = &mut self.lines {
            lines.push(msg.into());
        }
    }
    pub(super) fn flush(&mut self, module_name: &str) {
        if let Some(lines) = &self.lines {
            let sanitized: String = module_name
                .chars()
                .map(|c| {
                    if c.is_alphanumeric() || c == '_' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            let _ = crate::debug_artifacts::write_debug_artifact(
                format!("promotion/{sanitized}.txt"),
                lines.join("\n") + "\n",
            );
        }
    }
}

fn promote_function(
    func: &mut TirFunction,
    stats: &mut PromotionStats,
    dbg: &mut DebugLog,
) -> bool {
    // Cheap pre-filter: nothing to do without module ops.
    if !func.blocks.values().any(|b| {
        b.ops.iter().any(|op| {
            opcode_module_slot_access_role_table(op.opcode) == ModuleSlotAccessRole::KeyedAttr
        })
    }) {
        return false;
    }
    // Skip state machines outright.
    if func
        .blocks
        .values()
        .any(|b| b.ops.iter().any(|op| is_state_machine_op(op.opcode)))
    {
        dbg.note(format!("{}: skip (state-machine ops)", func.name));
        return false;
    }

    let names = const_str_defs(func);
    // Wildcard module access anywhere → skip the function.
    if let Some(op) = func
        .blocks
        .values()
        .find_map(|b| b.ops.iter().find(|op| is_wildcard_module_op(op, &names)))
    {
        dbg.note(format!(
            "{}: skip (wildcard module op {:?})",
            func.name, op.opcode
        ));
        return false;
    }

    let alias = AliasAnalysisResult::compute(func);
    let Some(module_root) = single_module_root(func, &alias) else {
        dbg.note(format!(
            "{}: skip (no single entry-arg module root)",
            func.name
        ));
        return false;
    };

    let pred_map = build_pred_map_with(func, CfgEdgePolicy::TerminatorOnly);
    let idoms = compute_idoms_with(func, &pred_map, CfgEdgePolicy::TerminatorOnly);
    let loops = discover_loops(func, &pred_map, &idoms, dbg);
    if loops.is_empty() {
        return false;
    }

    let mut changed = false;
    for lp in loops {
        changed |= promote_loop(func, &lp, module_root, &names, &alias, stats, dbg);
    }
    changed
}
