//! TIR **function inliner** — the Tier-2 engine keystone (E1, phases a + b).
//!
//! This is a *module* transform (it splices one function's body into another),
//! not a per-function [`TirPass`](crate::tir::pass_manager). It runs inside
//! [`run_module_pipeline`](crate::tir::module_phase::run_module_pipeline) after
//! the call graph + summaries are built, walks the call graph **bottom-up over
//! the SCC condensation** (every callee is finalized before its callers), and at
//! each statically-resolved, in-budget, exception-free, non-recursive,
//! non-generator call site replaces the `Call` op with a fresh-id clone of the
//! callee's body. After a function has had one or more callees inlined, the
//! per-function S1 [`run_pipeline`](crate::tir::passes::run_pipeline) re-runs on
//! the merged function so the inlined code is optimized *jointly* with the
//! caller (the entire point of inlining — constant-folding the callee's return
//! through the caller's uses, eliminating the call boundary).
//!
//! ## What this arc (phases a + b) does and does NOT do
//!
//! * **(a) clone + remap primitives** — [`clone_function_body_with_fresh_ids`]
//!   produces a disjoint-SSA copy of a callee body inside the caller, with every
//!   `ValueId` / `BlockId` / terminator target / block argument remapped through
//!   the caller's `fresh_value` / `fresh_block` counters. The callee's parameter
//!   values bind *directly* to the call's argument values (no copy ops), so the
//!   cloned entry block carries no arguments. All loop metadata
//!   (`label_id_map` + `loop_roles` + `loop_pairs` + `loop_break_kinds` +
//!   `loop_cond_blocks`) transfers with remapped keys.
//! * **(b) simple splice + module wiring** — [`splice_call_site`] splits the
//!   caller block at the `Call`, branches the first half into the cloned entry,
//!   rewrites each callee `Return` into a branch to the continuation block (which
//!   binds the returned value to the original call-result `ValueId`), and deletes
//!   the `Call`. [`run_inliner`] drives this across the module.
//!
//! Phase c (this arc) extends inlining to **observation-only** callees:
//! functions that carry `CheckException` propagation ops but no real exception
//! HANDLER region (no `try`/`except` `TryStart`/`TryEnd`, no generator/async
//! `StateBlock`). Every callee exit — the normal `Return` AND the exception-exit
//! `Return` (the `ret_void` reached only via `CheckException` edges) — is routed
//! to the continuation block `B_cont`, whose first op is the caller's own
//! post-call `CheckException`; that re-observes the pending flag and routes to
//! the caller's handler exactly as the un-inlined call/return/check sequence did.
//! The clone remaps the callee's per-function exception labels to fresh caller
//! ids (no namespace collision) and pads a void exception-exit's branch into the
//! value-carrying continuation with a representation-matched dead placeholder.
//!
//! Phases d (cost / multi-site / fixed-point) and e (retire the SimpleIR inliner)
//! are SEPARATE later arcs. [`is_inlineable`] still conservatively refuses any
//! callee with a true exception HANDLER region ([`TirFunction::has_exception_handlers`]),
//! any recursive-SCC member, any callee over the cost-model op budget, and any
//! callee containing a generator/async op. Refusing handler-bearing callees is
//! *conservative-correct*, not interim: it never miscompiles, it only forgoes an
//! optimization a later handler-aware arc unlocks.
//!
//! ## The three correctness invariants (each a miscompile if violated)
//!
//! 1. **SSA** — the splice is structurally SSA-preserving: the continuation
//!    block is reachable *only* through the cloned callee's exits, every one of
//!    which is dominated by the cloned entry, and the call-result value is
//!    redefined as the continuation block's single argument. Every splice is
//!    followed by a `verify_function` assertion (in tests) and the
//!    [`run_pipeline`](crate::tir::passes::run_pipeline) re-run (which itself
//!    verifies). A splice that produced invalid SSA *panics*; it never silently
//!    corrupts.
//! 2. **REFCOUNT** — the calling convention is **+0 borrowed** parameters /
//!    **+1 owned** return. The splice adds and removes *zero* `IncRef`/`DecRef`
//!    ops, so the callee body's reference-count balance is preserved verbatim.
//!    The one caller-side hazard: a caller that does `IncRef(arg)` immediately
//!    before the `Call` (handing the callee an owned, not borrowed, argument)
//!    would, post-inline, leak that extra reference because the callee body
//!    consumes a *borrowed* parameter. [`splice_call_site`] therefore refuses any
//!    site with an `IncRef` of one of the call's argument values in the ≤2 ops
//!    immediately preceding the `Call` (the [`call_site_has_arg_incref`] guard).
//! 3. **LOOP METADATA** — LICM / BCE / the structured-loop back-conversion read
//!    `loop_roles` *and* `loop_pairs` *and* `loop_break_kinds` *and*
//!    `loop_cond_blocks`. Transferring only `loop_roles` (the obvious one) would
//!    leave the merged loop half-described and mis-optimized. The clone transfers
//!    **all four** maps (plus `label_id_map`) with every key remapped to the
//!    fresh block ids.

use std::collections::{HashMap, HashSet};

use super::super::call_graph::CallGraph;
use super::super::function::TirModule;
use super::super::target_info::TargetInfo;
use super::ip_summary::ModuleSummaries;

mod call_sites;
mod clone_body;
mod eligibility;
mod splice;
#[cfg(test)]
mod tests;

use self::call_sites::collect_call_sites;
pub use self::eligibility::{classify_inline_eligibility, is_inlineable};
use self::eligibility::{is_inline_safe, split_field_enabled_callees};
use self::splice::splice_call_site;

/// Statistics from one [`run_inliner`] invocation over a module.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InlinerStats {
    /// Number of call sites successfully inlined (a `Call` replaced by the
    /// callee body).
    pub sites_inlined: usize,
    /// Number of caller functions that had at least one site inlined (and were
    /// therefore re-optimized by the per-function pipeline).
    pub functions_changed: usize,
    /// Names of the caller functions that had at least one site inlined (and so
    /// whose body now differs from its pre-inline form). Production codegen
    /// back-converts ONLY these functions' TIR to SimpleIR, leaving every
    /// unchanged function byte-identical (no second TIR roundtrip).
    pub changed_functions: Vec<String>,
}

/// Run the inliner over `module` in **bottom-up SCC order** (callees finalized
/// before callers). After a function has one or more sites inlined, re-run the
/// per-function pipeline on the merged function so the inlined body is optimized
/// jointly with the caller.
///
/// `call_graph` and `summaries` describe the module *before* this pass; the
/// driver ([`run_module_pipeline`](crate::tir::module_phase::run_module_pipeline))
/// rebuilds both afterward.
pub fn run_inliner(
    module: &mut TirModule,
    call_graph: &CallGraph,
    summaries: &ModuleSummaries,
    tti: &TargetInfo,
    non_inlinable: &HashSet<String>,
) -> InlinerStats {
    let mut stats = InlinerStats::default();

    // The set of callee names that are inlinable (computed once from the
    // pre-pass call graph/summaries). Bodies stay module-owned and are borrowed
    // live at the splice site, so bottom-up callee changes are visible to callers
    // without cloning a second body authority.
    let defined: Vec<String> = module.functions.iter().map(|f| f.name.clone()).collect();

    // Callees that an in-budget inline misses but whose inlining unlocks the
    // split-field deforestation (a caller hands them a non-escaping
    // `string_split_field` result). These are admitted on the safety gate alone
    // (over-budget but sound) — the targeted enabling the baton specifies.
    let split_enabled = split_field_enabled_callees(module, &defined);

    // Map function name -> index in the module vector for O(1) lookup.
    let index_of: HashMap<String, usize> = module
        .functions
        .iter()
        .enumerate()
        .map(|(i, f)| (f.name.clone(), i))
        .collect();

    // Record inlinable callees by module index, not by cloned body. The module
    // owns each body exactly once; call-site splicing borrows the caller mutably
    // and the callee immutably via `split_at_mut` below. That preserves the
    // bottom-up contract without a whole-module body snapshot.
    let inlinable_indices: HashMap<String, usize> = module
        .functions
        .iter()
        .enumerate()
        // A callee whose canonical definition is linked externally (e.g. a
        // shared-stdlib-partition symbol that the native/wasm driver will
        // externalize into `stdlib_shared.o`) has external linkage: this module
        // does not own its body, so splicing a private copy at the call site is
        // unsound (it drops the external reference and forks the definition).
        // Refused unconditionally — the `Call` survives as an external reference.
        .filter(|(_, f)| !non_inlinable.contains(&f.name))
        .filter(|(_, f)| {
            is_inlineable(f, call_graph, summaries, tti)
                || (split_enabled.contains(&f.name) && is_inline_safe(f, call_graph))
        })
        .map(|(idx, f)| (f.name.clone(), idx))
        .collect();

    if inlinable_indices.is_empty() {
        return stats;
    }

    // Walk bottom-up over the SCC condensation: callees before callers.
    for scc in call_graph.bottom_up_order() {
        for caller_name in scc {
            let Some(&caller_idx) = index_of.get(&caller_name) else {
                continue;
            };

            // Collect this caller's inlinable call sites ONCE, then splice them
            // in **reverse** order (descending block id, then descending op
            // index). `collect_call_sites` yields ascending order, so `.rev()`
            // gives the splice-safe order: a splice at `(B, i)` keeps the
            // pre-call half at the *same* block id `B` with ops `0..i`, so every
            // not-yet-processed site at `(B, j<i)` or in an earlier block keeps
            // its `(block, op_index)` identity. Processing highest-index-first
            // therefore never invalidates a pending site's coordinates — no
            // re-collection needed.
            //
            // A refused site (refcount guard / shape mismatch) is simply skipped
            // (its `Call` survives, conservative-correct) and does NOT block the
            // remaining inlinable sites in the same caller.
            let mut changed_this_fn = false;
            let sites = {
                let caller = &module.functions[caller_idx];
                collect_call_sites(caller, &defined)
            };
            for site in sites.into_iter().rev() {
                if site.callee == caller_name {
                    continue; // self-call (recursive) — never inline.
                }
                let Some(&callee_idx) = inlinable_indices.get(&site.callee) else {
                    continue;
                };
                if callee_idx == caller_idx {
                    continue;
                }
                let (caller, callee) = if caller_idx < callee_idx {
                    let (left, right) = module.functions.split_at_mut(callee_idx);
                    (&mut left[caller_idx], &right[0])
                } else {
                    let (left, right) = module.functions.split_at_mut(caller_idx);
                    (&mut right[0], &left[callee_idx])
                };
                let callee_has_exception_handling = callee.has_exception_handling;
                let did_inline = splice_call_site(caller, callee, &site);
                if did_inline {
                    stats.sites_inlined += 1;
                    changed_this_fn = true;
                    // Propagate the callee's exception-handling flag. An
                    // OBSERVATION-only callee carries `has_exception_handling`
                    // (its `CheckException` ops set it); inlining its body imports
                    // those ops, so the merged caller must be flagged too — the
                    // conservative downstream passes (SCCP try-region, DCE) read
                    // this flag. (The caller is usually already flagged, since it
                    // has its own post-call `CheckException`, but a caller with no
                    // exception ops of its own would otherwise be left unflagged.)
                    if callee_has_exception_handling {
                        caller.has_exception_handling = true;
                    }
                }
            }

            if changed_this_fn {
                stats.functions_changed += 1;
                stats.changed_functions.push(caller_name.clone());
                // Re-run the per-function pipeline on the merged caller so the
                // inlined body is optimized jointly. A fresh PassManager (no
                // stale AnalysisManager cache) — run_pipeline builds one anew.
                // Bracket with type refinement on BOTH sides (refine → pipeline →
                // refine), matching every backend's per-function lift contract, so
                // `run_module_pipeline` returns every changed body *fully
                // type-refined*. The LLVM/WASM/native lowerers and the post-inline
                // representation-fact rebuild all depend on this invariant: an
                // unrefined merged body would floor its values to `DynBox` and emit
                // boxed dispatch on exactly the hot inlined paths.
                let caller = &mut module.functions[caller_idx];
                super::super::type_refine::refine_types(caller);
                let _ = super::run_pipeline(caller, tti);
                super::super::type_refine::refine_types(caller);
            }
        }
    }

    stats
}
