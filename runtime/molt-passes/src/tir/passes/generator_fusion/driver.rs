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
    let poll_bodies: HashMap<String, TirFunction> = module
        .functions
        .iter()
        .filter(|f| is_poll_fusable(f, call_graph))
        .map(|f| (f.name.clone(), f.clone()))
        .collect();
    if poll_bodies.is_empty() {
        return stats;
    }

    // Map function name -> index for O(1) caller lookup, owned (drops the borrow
    // on `module.functions` before mutation).
    let index_of: HashMap<String, usize> = module
        .functions
        .iter()
        .enumerate()
        .map(|(i, f)| (f.name.clone(), i))
        .collect();

    // Caller names processed in deterministic order.
    let mut caller_names: Vec<String> = module.functions.iter().map(|f| f.name.clone()).collect();
    caller_names.sort();

    for caller_name in caller_names {
        let Some(&caller_idx) = index_of.get(&caller_name) else {
            continue;
        };
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
            let Some(poll) = poll_bodies.get(&candidate.poll_name) else {
                break;
            };
            let poll_owned = poll.clone();
            let caller = &mut module.functions[caller_idx];
            let spliced = apply_fusion(caller, &poll_owned, &candidate, &mut stats);
            if spliced {
                if !stats.changed_functions.contains(&caller_name) {
                    stats.changed_functions.push(caller_name.clone());
                }
                // Re-optimize the merged caller jointly (SCCP folds the dead
                // throw-check, LICM/escape/BCE clean up the fused loop). Bracket
                // with type refinement on both sides, matching the inliner's
                // refine→pipeline→refine contract so the backends receive a
                // fully-refined body.
                super::super::super::type_refine::refine_types(caller);
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
