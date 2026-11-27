//! TIR function inliner driver.
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
//! caller (the entire point of inlining - constant-folding the callee's return
//! through the caller's uses, eliminating the call boundary).
//!
//! ## What this arc (phases a + b) does and does NOT do
//!
//! * **(a) clone + remap primitives** - [`clone_function_body_with_fresh_ids`]
//!   produces a disjoint-SSA copy of a callee body inside the caller, with every
//!   `ValueId` / `BlockId` / terminator target / block argument remapped through
//!   the caller's `fresh_value` / `fresh_block` counters. The callee's parameter
//!   values bind *directly* to the values the splice supplies (each argument,
//!   or its owned binding alias), so the cloned entry block carries no
//!   arguments. All loop metadata
//!   (`label_id_map` + `loop_roles` + `loop_pairs` + `loop_break_kinds` +
//!   `loop_cond_blocks`) transfers with remapped keys.
//! * **(b) simple splice + module wiring** - [`splice_call_site`] splits the
//!   caller block at the `Call`, branches the first half into the cloned entry,
//!   rewrites each callee `Return` into a branch to the continuation block (which
//!   binds the returned value to the original call-result `ValueId`), and deletes
//!   the `Call`. [`run_inliner`] drives this across the module.
//!
//! Phase c (this arc) extends inlining to **observation-only** callees:
//! functions that carry `CheckException` propagation ops but no real exception
//! HANDLER region (no `try`/`except` `TryStart`/`TryEnd`, no generator/async
//! `StateBlock`). Normal exits route to the continuation block `B_cont`; void
//! exception exits reached only through `CheckException` either branch directly
//! to the caller's post-call handler (when the continuation starts with that
//! observation) or use a representation-matched dead placeholder for the
//! value-carrying continuation. The clone remaps the callee's per-function
//! exception labels to fresh caller ids, so merged exception edges cannot collide
//! with caller labels.
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
//! 1. **SSA** - the splice is structurally SSA-preserving: the continuation
//!    block is reachable *only* through the cloned callee's exits, every one of
//!    which is dominated by the cloned entry, and the call-result value is
//!    redefined as the continuation block's single argument. Every splice is
//!    followed by a `verify_function` assertion (in tests) and the
//!    [`run_pipeline`](crate::tir::passes::run_pipeline) re-run (which itself
//!    verifies). A splice that produced invalid SSA *panics*; it never silently
//!    corrupts.
//! 2. **REFCOUNT** - the call's reference contract: each parameter's declared
//!    custody (`Borrowed` or `Transferred`, design 20 §1.6), the callee's frame
//!    clear at every exit, and one owned result. The splice places no
//!    `IncRef`/`DecRef` of its own. Its activation (`activation.rs`) binds each
//!    parameter the activation owns through an owned `binding_alias`, ends each
//!    exit with the frame clear DropInsertion plans for the callee's `Return`
//!    (as `DelBoundary`s), and captures a returned frame binding first; the
//!    caller's DropInsertion places all of them. A caller that does
//!    `IncRef(arg)` immediately before the `Call` hands the callee a reference
//!    that no custody names, so [`splice_call_site`] refuses any site with an
//!    `IncRef` of one of the call's argument values in the <=2 ops immediately
//!    preceding the `Call` (the [`call_site_has_arg_incref`] guard).
//! 3. **LOOP METADATA** - LICM / BCE / the structured-loop back-conversion read
//!    `loop_roles` *and* `loop_pairs` *and* `loop_break_kinds` *and*
//!    `loop_cond_blocks`. Transferring only `loop_roles` (the obvious one) would
//!    leave the merged loop half-described and mis-optimized. The clone transfers
//!    **all four** maps (plus `label_id_map`) with every key remapped to the
//!    fresh block ids.

use molt_ir::backend_environment::{self as backend_env, BackendEnv};
use molt_ir::deterministic_hash::{HashMap, HashSet};

use crate::tir::call_facts::{InlineEligibility, InlineWhyNot};
use crate::tir::call_graph::CallGraph;
use crate::tir::function::{TirFunction, TirModule};
use crate::tir::passes::async_work_poll;
use crate::tir::passes::check_exception_elim::classify::RaiseFacts;
use crate::tir::passes::ip_summary::ModuleSummaries;
use crate::tir::target_info::TargetInfo;

use super::activation::Activations;
use super::call_sites::InlineSiteUses;
use super::eligibility::{
    callee_op_count, classify_with_wrapper, is_inline_safe, is_numeric_raw_lane_return_type,
};
use super::splice::splice_call_site;

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
    pub changed_functions: Vec<molt_ir::FunctionId>,
}

#[derive(Clone, Copy)]
enum InlineBudget {
    Ordinary,
    NeedsBenefit { numeric: bool, within_hot_cap: bool },
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

    let index_of = module.body_index();
    let hot_cap = tti.hot_inline_op_limit();
    let mut diagnostics =
        (backend_env::var(BackendEnv::MOLT_INLINE_STATS).as_deref() == Ok("1")).then(Vec::new);
    let final_tir = tti.lowering.final_lowering == crate::tir::target_info::FinalLowering::FinalTir;
    let mut wrappers = HashMap::default();
    let mut pending_wrappers = HashSet::default();

    let budget_for =
        |id, body: &TirFunction, wrapper: Option<&super::packed_wrapper::PackedWrapper>| {
            match classify_with_wrapper(id, body, call_graph, summaries, tti, wrapper) {
                InlineEligibility::Eligible => Some(InlineBudget::Ordinary),
                InlineEligibility::WhyNot(InlineWhyNot::OverBudget)
                    if wrapper.is_some() || is_inline_safe(id, body, call_graph) =>
                {
                    Some(InlineBudget::NeedsBenefit {
                        numeric: is_numeric_raw_lane_return_type(&body.return_type),
                        within_hot_cap: callee_op_count(id, body, summaries) <= hot_cap,
                    })
                }
                _ => None,
            }
        };
    let mut inlinable: HashMap<_, _> = module
        .functions
        .iter()
        .zip(module.ids.iter().copied())
        .enumerate()
        .filter(|(_, (body, id))| {
            !call_graph.callers(*id).is_empty() && !non_inlinable.contains(&body.name)
        })
        .filter_map(|(idx, (body, id))| {
            if final_tir
                && body.execution_context == molt_ir::ExecutionContextPolicy::Local
                && !call_graph.in_static_cycle(id)
                && call_graph.callees(id).len() >= 2
                && body
                    .exact_param_types
                    .iter()
                    .any(|ty| matches!(ty, crate::tir::types::TirType::Class(_)))
            {
                pending_wrappers.insert(id);
            }
            Some((id, (idx, budget_for(id, body, None)?)))
        })
        .collect();
    if inlinable.is_empty() && pending_wrappers.is_empty() && diagnostics.is_none() {
        return stats;
    }

    // Prepared activations serve every caller: a callee is final before any
    // caller splices it.
    let mut activations = Activations::default();

    // Walk bottom-up over the SCC condensation: callees before callers.
    for scc in call_graph.bottom_up_order() {
        for &caller_id in scc {
            let Some(caller_idx) = index_of.get(caller_id.index()).copied().flatten() else {
                continue;
            };

            // The call graph indexed this caller before the traversal.
            // Its body is still unchanged. Reverse the indexed order so a
            // splice at `(B, i)` keeps the
            // pre-call half at the *same* block id `B` with ops `0..i`, so every
            // not-yet-processed site at `(B, j<i)` or in an earlier block keeps
            // its `(block, op_index)` identity. Processing highest-index-first
            // therefore never invalidates a pending site's coordinates - no
            // re-collection needed.
            //
            // A refused site (refcount guard / shape mismatch) is simply skipped
            // (its `Call` survives, conservative-correct) and does NOT block the
            // remaining inlinable sites in the same caller.
            let callees = call_graph.callees(caller_id);
            // The proof carries operation positions and exceptional transfers
            // in the wrapper and its dependencies. Bottom-up order makes those
            // bodies final before their first caller reads the proof.
            for &id in callees {
                if !pending_wrappers.remove(&id) {
                    continue;
                }
                let idx = index_of[id.index()].expect("composed wrapper body");
                let body = &module.functions[idx];
                let Some(proof) = super::packed_wrapper::summarize(id, body, |callee| {
                    index_of
                        .get(callee.index())
                        .copied()
                        .flatten()
                        .map(|index| &module.functions[index])
                }) else {
                    continue;
                };
                if let Some(budget) = budget_for(id, body, Some(&proof)) {
                    inlinable.insert(id, (idx, budget));
                    wrappers.insert(id, proof);
                }
            }

            if callees.is_empty()
                || (diagnostics.is_none() && !callees.iter().any(|id| inlinable.contains_key(id)))
            {
                continue;
            }
            let caller = &module.functions[caller_idx];
            let sites = call_graph.static_call_sites(caller_id);
            let mut uses = None;
            let admitted: Vec<_> = sites
                .iter()
                .map(|site| {
                    let budget = inlinable.get(&site.callee).map(|(_, budget)| *budget);
                    let op = &caller.blocks[&site.block].ops[site.op_index];
                    let admitted = match budget {
                        Some(InlineBudget::Ordinary) => true,
                        Some(InlineBudget::NeedsBenefit { numeric, within_hot_cap }) => {
                            let uses = uses.get_or_insert_with(|| InlineSiteUses::of(caller));
                            let body = |id| inlinable.get(&id).map(|&(idx, _)| &module.functions[idx]);
                            uses.receives_split_field(op)
                                || (within_hot_cap && (numeric && uses.numeric_consumer(op).is_some()
                                    || uses.receives_noncaptured_allocation(op, caller, &mut activations.capture, body)))
                        }
                        None => false,
                    };
                    if let Some(details) = &mut diagnostics
                        && let Some((result, consumer)) = uses
                            .get_or_insert_with(|| InlineSiteUses::of(caller))
                            .numeric_consumer(op)
                        && let Some(callee_idx) = index_of.get(site.callee.index()).copied().flatten()
                    {
                        let callee = &module.functions[callee_idx];
                        let ops = callee_op_count(site.callee, callee, summaries);
                        let return_type = &callee.return_type;
                        let numeric_return = is_numeric_raw_lane_return_type(return_type);
                        details.push(format!(
                            "{}:{} result={result:?} return={return_type:?} numeric_return={numeric_return} consumer={consumer:?} ops={ops} hot_cap={hot_cap} admitted={admitted}",
                            caller.name, callee.name
                        ));
                    }
                    admitted
                })
                .collect();
            if !admitted.iter().any(|&admitted| admitted) {
                continue;
            }

            // Observe every original call before splicing. Once a site is
            // spliced, its original poll may move to the continuation.
            let predecessors = crate::tir::dominators::build_pred_map(caller);
            let raise = RaiseFacts::of(caller);
            let site_polls: Vec<_> = sites
                .iter()
                .zip(&admitted)
                .map(|(site, &admitted)| {
                    if admitted {
                        async_work_poll::call_return_poll(
                            caller,
                            site.block,
                            site.op_index,
                            &predecessors,
                            &raise,
                        )
                    } else {
                        None
                    }
                })
                .collect();
            let mut changed_this_fn = false;
            activations.enter_caller(sites.iter().zip(&admitted).filter_map(
                |(site, &admitted)| (admitted && site.callee != caller_id).then_some(site),
            ));
            let mut inlined_call_polls = Vec::new();
            for ((site, poll), admitted) in
                sites.iter().copied().zip(site_polls).zip(admitted).rev()
            {
                if !admitted || site.callee == caller_id {
                    continue; // self-call (recursive) - never inline.
                }
                let Some(&(callee_idx, _)) = inlinable.get(&site.callee) else {
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
                let splice = if let Some(summary) = wrappers.get(&site.callee) {
                    let continuation = super::packed_wrapper_resume::splice(
                        caller,
                        callee,
                        &site,
                        summary,
                        &mut activations,
                    );
                    if continuation.is_some() && diagnostics.is_some() {
                        eprintln!("[packed-wrapper splice] {} -> {}", caller.name, callee.name);
                    }
                    continuation.map(|block| (block, true))
                } else {
                    splice_call_site(caller, callee, &site, &mut activations)
                        .map(|block| (block, false))
                };
                if let Some((continuation, preserve_poll)) = splice {
                    stats.sites_inlined += 1;
                    if !preserve_poll && let Some((poll_block, poll_index)) = poll {
                        inlined_call_polls.push(
                            if poll_block == site.block && poll_index > site.op_index {
                                (continuation, poll_index - site.op_index - 1)
                            } else {
                                (poll_block, poll_index)
                            },
                        );
                    }
                    changed_this_fn = true;
                    // Propagate the callee's exception-handling flag. An
                    // OBSERVATION-only callee carries `has_exception_handling`
                    // (its `CheckException` ops set it); inlining its body imports
                    // those ops, so the merged caller must be flagged too - the
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
                async_work_poll::retire_inlined_call_polls(
                    &mut module.functions[caller_idx],
                    &inlined_call_polls,
                );
                stats.functions_changed += 1;
                stats.changed_functions.push(caller_id);
                // Re-optimize the merged caller so the inlined body is
                // optimized jointly, through the module phase's one re-entry
                // contract (refine -> re-entry pipeline -> refine): the
                // lowerers and the post-inline representation-fact rebuild
                // need every changed body fully refined, and the caller's
                // async-work polls were placed on its first entry.
                let _ = super::super::reoptimize(&mut module.functions[caller_idx], tti);
            }
        }
    }

    if let Some(mut details) = diagnostics {
        details.sort_unstable();
        if details.is_empty() {
            details.push("no static direct call results feed numeric consumers".to_string());
        }
        let line = format!(
            "[E1] module '{}': numeric raw-lane inline candidates {}\n",
            module.name,
            details.join(", ")
        );
        eprint!("{line}");
        let _ = crate::debug_artifacts::append_debug_artifact(
            "tir/inliner_numeric_raw_lane_candidates.txt",
            line,
        );
    }

    stats
}
