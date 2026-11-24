use molt_ir::deterministic_hash::{HashMap, HashSet};

use super::super::super::call_graph::CallGraph;
use super::super::super::function::{TirFunction, TirModule};
use super::super::super::target_info::TargetInfo;
use super::{FusionStats, PollRefusal, apply_fusion, next_fusion_candidate, poll_fusion_refusal};

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
    // Each snapshot is the linearized poll: a yield continues at its resume
    // block (`linearize`).
    let poll_bodies: HashMap<molt_ir::FunctionId, TirFunction> = module
        .functions
        .iter()
        .zip(module.ids.iter().copied())
        .filter_map(|(f, id)| {
            let refusal = poll_fusion_refusal(id, f, call_graph);
            let linear = match refusal {
                None => super::linearize::linearize_poll(f),
                Some(_) => None,
            };
            let refusal = refusal.or(linear.is_none().then_some(PollRefusal::ResumeShape));
            if let Some(refusal) = refusal
                && refusal != PollRefusal::NotAGenerator
            {
                stats.refused_polls.push((f.name.clone(), refusal));
            }
            let linear = linear?;
            if tti.supports_pending_call_eval_breaker_poll() {
                assert!(
                    super::super::async_work_poll::is_materialized(f),
                    "generator fusion requires post-pipeline async-work observations in poll {:?}",
                    f.name
                );
            }
            Some((id, linear))
        })
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
        // (`decline`): fold it, so the loop's `IterNextUnboxed` is the
        // iterator's one consumer, and re-optimize the caller so the folded
        // chunk loop, whose edges would otherwise still join the loop's
        // carried values, is gone before recognition.
        if super::decline::decline_reduction_kernels(&mut module.functions[caller_idx]) {
            stats.changed_functions.push(caller_id);
            let _ = super::super::reoptimize(&mut module.functions[caller_idx], tti);
        }
        // Collect candidates over the current caller body, then splice them one
        // at a time (re-collecting after each splice — a successful splice
        // rewrites the caller's blocks, invalidating prior coordinates).
        let mut declined = HashSet::default();
        loop {
            let candidate = {
                let caller = &module.functions[caller_idx];
                next_fusion_candidate(caller, &poll_bodies, &declined)
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
            let unfused = caller.clone();
            let unfused_stats = stats.clone();
            let spliced = apply_fusion(
                caller,
                &poll_owned,
                &candidate,
                &mut stats,
                tti.supports_pending_call_eval_breaker_poll(),
            );
            if spliced {
                // Re-optimize the merged caller jointly (SCCP folds the dead
                // throw-check, LICM/escape/BCE clean up the fused loop)
                // through the module phase's one re-entry contract. The
                // consumer's latch poll stays the fused loop's poll, so
                // placement does not run again.
                let _ = super::super::reoptimize(caller, tti);
                // The merged caller's facts may prove a bracketed operation
                // callback-free (the generator's multiply once its operand is
                // the loop counter): its frame publication goes, and the loop
                // is optimized again without it (spliced_frames.md 4.4).
                if super::super::spliced_frame::prune_brackets(caller) != 0 {
                    let _ = super::super::reoptimize(caller, tti);
                }
                // A bracket hands Python code a snapshot of the generator's
                // locals. Where the program holds a PEP 667 frame-locals
                // writer (HF-901) that code may rebind them, which the
                // snapshot would drop: keep the generator unfused (5.1).
                if poll_owned.frame_locals_exposed()
                    && super::super::spliced_frame::has_bracket(caller)
                {
                    *caller = unfused;
                    stats = unfused_stats;
                    stats
                        .refused_polls
                        .push((poll_owned.name.clone(), PollRefusal::FrameLocalsWriter));
                    declined.insert(candidate.alloc_val);
                    continue;
                }
                if !stats.changed_functions.contains(&caller_id) {
                    stats.changed_functions.push(caller_id);
                }
            } else {
                // A failed transaction leaves this allocation and every other
                // site's coordinates intact. Other sites may still splice.
                declined.insert(candidate.alloc_val);
            }
        }
    }

    stats
}
