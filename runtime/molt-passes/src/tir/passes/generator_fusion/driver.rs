use molt_ir::deterministic_hash::HashMap;

use super::super::super::call_graph::CallGraph;
use super::super::super::function::{TirFunction, TirModule};
use super::super::super::target_info::TargetInfo;
use super::{
    FusionStats, PollRefusal, apply_fusion, collect_fusion_candidates, poll_fusion_refusal,
};

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
            let refusal = poll_fusion_refusal(*id, f, call_graph);
            if let Some(refusal) = refusal
                && refusal != PollRefusal::NotAGenerator
            {
                stats.refused_polls.push((f.name.clone(), refusal));
            }
            let fusable = refusal.is_none();
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
        // A reduction kernel over a generator this caller allocates declines
        // (`decline`): fold it, so the loop's `IterNext` is the iterator's one
        // consumer.
        let declined = super::decline::decline_reduction_kernels(&mut module.functions[caller_idx]);
        let mut spliced_any = false;
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
                spliced_any = true;
                if !stats.changed_functions.contains(&caller_id) {
                    stats.changed_functions.push(caller_id);
                }
                // Re-optimize the merged caller jointly (SCCP folds the dead
                // throw-check, LICM/escape/BCE clean up the fused loop)
                // through the module phase's one re-entry contract. The
                // splice placed the fused loop's polls itself (the poll
                // body's own, the consumer latch's retired), so placement
                // does not run again.
                let _ = super::super::reoptimize(caller, tti);
            } else {
                // The candidate could not be spliced (a conservative
                // mid-analysis bail). Stop processing this caller to avoid an
                // infinite re-collect loop on the same un-spliceable site.
                break;
            }
        }
        if declined && !spliced_any {
            // The fold alone changed the caller: re-optimize it as a splice does.
            stats.changed_functions.push(caller_id);
            let caller = &mut module.functions[caller_idx];
            let _ = super::super::run_pipeline(caller, tti);
            super::super::super::type_refine::refine_types(caller);
        }
    }

    stats
}
