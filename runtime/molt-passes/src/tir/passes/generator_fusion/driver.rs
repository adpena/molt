use std::collections::HashMap;

use super::super::super::call_graph::CallGraph;
use super::super::super::function::{TirFunction, TirModule};
use super::super::super::target_info::TargetInfo;
use super::{FusionStats, apply_fusion, collect_fusion_candidates, is_poll_fusable};

/// Run generator fusion over `module`. Returns the elided-frame statistics.
///
pub fn run_generator_fusion(
    module: &mut TirModule,
    call_graph: &CallGraph,
    tti: &TargetInfo,
) -> FusionStats {
    let mut stats = FusionStats::default();

    // Snapshot every fusable poll body up front (owned clones), keyed by name —
    // the splice reads the poll body while holding `&mut` on the caller, and the
    // borrow checker cannot prove disjointness through the module vector.
    let poll_bodies: HashMap<molt_ir::FunctionId, TirFunction> = module
        .functions
        .iter()
        .zip(module.ids.iter().copied())
        .filter(|(f, id)| {
            let fusable = is_poll_fusable(*id, f, call_graph);
            if fusable && tti.supports_pending_call_eval_breaker_poll() {
                assert!(
                    super::super::async_work_poll::is_materialized(f),
                    "generator fusion requires post-pipeline async-work observations in poll {:?}",
                    f.name
                );
            }
            fusable
        })
        .map(|(f, id)| (id, f.clone()))
        .collect();
    if poll_bodies.is_empty() {
        return stats;
    }

    // Callers in deterministic (id) order.
    let mut callers: Vec<(molt_ir::FunctionId, usize)> =
        module.ids.iter().copied().zip(0..).collect();
    callers.sort_unstable();

    for (caller_id, caller_idx) in callers {
        let caller_name = module.functions[caller_idx].name.clone();
        // Collect candidates over the current caller body, then splice them one
        // at a time (re-collecting after each splice — a successful splice
        // rewrites the caller's blocks, invalidating prior coordinates).
        loop {
            let candidate = {
                let caller = &module.functions[caller_idx];
                collect_fusion_candidates(caller, &poll_bodies, call_graph)
                    .into_iter()
                    .next()
            };
            let Some(candidate) = candidate else { break };
            if tti.supports_pending_call_eval_breaker_poll() {
                assert!(
                    super::super::async_work_poll::is_materialized(&module.functions[caller_idx]),
                    "generator fusion requires post-pipeline async-work observations in caller {:?}",
                    caller_name
                );
            }
            let Some(poll) = poll_bodies.get(&candidate.poll) else {
                break;
            };
            let poll_owned = poll.clone();
            let caller = &mut module.functions[caller_idx];
            let spliced = apply_fusion(caller, &poll_owned, &candidate, &mut stats);
            if spliced {
                if !stats.changed_functions.contains(&caller_id) {
                    stats.changed_functions.push(caller_id);
                }
                // Re-optimize the merged caller jointly (SCCP folds the dead
                // throw-check, LICM/escape/BCE clean up the fused loop).
                // `apply_fusion` leaves the caller type-refined, and refinement
                // follows the pipeline again: the inliner's
                // refine→pipeline→refine contract, so the backends receive a
                // fully-refined body.
                let _ = super::super::run_pipeline(caller, tti);
                super::super::super::type_refine::refine_types(caller);
            } else {
                // The candidate could not be spliced (a conservative
                // mid-analysis bail). Stop processing this caller to avoid an
                // infinite re-collect loop on the same un-spliceable site.
                break;
            }
        }
    }

    stats
}
