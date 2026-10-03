use std::collections::{HashMap, HashSet};

use super::super::super::blocks::BlockId;
use super::super::super::function::TirFunction;
use super::super::super::op_kinds_generated::opcode_generator_fusion_poll_role_table;
use super::super::super::ops::{Dialect, OpCode, TirOp};
use super::super::super::types::TirType;
use super::super::super::values::ValueId;
use super::super::ownership_lattice_min::Replacements;
use super::clone::clone_and_rewrite_poll;
use super::slots::{SlotPlan, plan_slots};
use super::wire::wire_fused_loop;
use super::{FusionCandidate, FusionStats};

// ===========================================================================
// The splice (single-yield-site — the Tier-B keystone)
// ===========================================================================
//
// Phase 1 splices the structurally-cleanest class that covers the perf keystone
// (`bench_generator_iter`) and the os.walk inner loop: **single-yield-site
// generators** — exactly one `StateYield` in the poll body. This is the
// `while <cond>: yield <expr>; <step>` shape (a yield inside the generator's own
// loop) and the bare `def g(): ...; yield <expr>` shape. The generator's own
// control flow becomes the fused loop; the single yield binds the element to the
// consumer's for-target and runs the consumer body inline; the frame's user
// slots become SSA values of that control flow (`slots.rs`), a parameter slot
// starting as the frame's reference to its `AllocTask` argument.
//
// Multi-yield-SITE generators (sequential `yield a; yield b; ...`) need a
// return-dispatch over yield-delimited segments — doc-26 Phase-1 Finding #1 —
// and bail soundly here (the generator stays Tier D: a correct heap frame).

/// Apply the fusion splice for `candidate`. Returns `true` iff the caller was
/// mutated; `false` on a conservative bail (caller left byte-identical). A fused
/// caller is left type-refined, and each promoted read and parameter slot keeps
/// the reference the elided frame's load or slot held.
pub(in crate::tir::passes::generator_fusion) fn apply_fusion(
    caller: &mut TirFunction,
    poll: &TirFunction,
    candidate: &FusionCandidate,
    stats: &mut FusionStats,
) -> bool {
    let Some(plan) = preflight_fusion(caller, poll, candidate) else {
        return false;
    };

    // Cloning and wiring allocate ids and rewrite CFG, type, label, and loop
    // metadata before every structural surprise can be ruled out. Perform that
    // whole mutable phase on an owned staging function. A conservative bailout
    // drops the stage; the caller (including its allocation counters) remains
    // byte-identical. Successful fusion commits with one move, preserving the
    // exact deterministic id sequence produced by the former in-place path.
    let mut staged = caller.clone();
    let Some(owners) = apply_fusion_staged(&mut staged, poll, candidate, &plan) else {
        return false;
    };

    // Keep the commit itself infallible: even debug overflow must be detected
    // before replacing the caller so a panic cannot expose half-committed state.
    let next_frames_elided = stats
        .frames_elided
        .checked_add(1)
        .expect("generator-fusion frame statistic overflow");
    let next_yield_sites_spliced = stats
        .yield_sites_spliced
        .checked_add(1)
        .expect("generator-fusion yield statistic overflow");
    *caller = staged;
    stats.frames_elided = next_frames_elided;
    stats.yield_sites_spliced = next_yield_sites_spliced;
    // The owners are decided on refined facts, so a slot whose refined range
    // proves it a raw integer holds no reference and its reads stay plain
    // copies that the value facts see through.
    super::super::super::type_refine::refine_types(caller);
    owners.finish(caller, None);
    true
}

/// Complete every read-only eligibility check and the frame-slot plan before
/// allocating the transaction's staging copy. Returning `None` cannot mutate
/// either the caller or the fusion statistics.
fn preflight_fusion(
    caller: &TirFunction,
    poll: &TirFunction,
    candidate: &FusionCandidate,
) -> Option<SlotPlan> {
    let retired: HashSet<_> = std::iter::once(candidate.cond_block)
        .chain(candidate.loop_header)
        .collect();
    let retired_loops = candidate.loop_header.into_iter().collect();
    caller
        .validate_block_retirement(
            &retired,
            &retired_loops,
            &HashSet::from([candidate.loop_header.unwrap_or(candidate.cond_block)]),
            false, // wire_fused_loop rewires edges but does not replace caller.entry_block.
        )
        .ok()?;

    // --- Phase-1 gate: exactly one yield site. ---
    let yield_count: usize = poll
        .blocks
        .values()
        .flat_map(|b| b.ops.iter())
        .filter(|op| opcode_generator_fusion_poll_role_table(op.opcode).is_required_yield())
        .count();
    if yield_count != 1 {
        // Multi-yield-site (sequential `yield a; yield b; ...`) needs a
        // return-dispatch over yield-delimited segments — doc-26 Phase-1
        // Finding #1. Conservative bail: the generator stays Tier D.
        return None;
    }

    // --- Consumer-carried-state gate. A function-scope consumer threads its own
    //     loop-carried values (e.g. an accumulator `total`) as block ARGUMENTS
    //     on its loop header — the standard SSA loop-phi form. Splicing the
    //     generator's loop in between those edges requires re-threading those
    //     carried values through the fused loop (doc-26 Phase-1 Finding #1,
    //     function-scope extension). Phase 1 handles the consumer whose loop
    //     region carries NO block args (module-scope consumers keep `total` in the
    //     module dict via ModuleGetAttr/SetAttr, so their loop blocks are
    //     arg-less); bail soundly (Tier D) when any block in the consumer loop
    //     region — the cond/body blocks, the loop header, and the continue target
    //     the body branches back to — carries args. ---
    let mut consumer_region: Vec<BlockId> = vec![candidate.cond_block, candidate.body_block];
    if let Some(h) = candidate.loop_header {
        consumer_region.push(h);
    }
    // The block the body loops back to (the continue target) is the carried-phi
    // header in the function-scope shape.
    if let Some(body) = caller.blocks.get(&candidate.body_block) {
        body.terminator
            .for_each_edge(|target, _| consumer_region.push(target));
    }
    for b in consumer_region {
        if caller
            .blocks
            .get(&b)
            .is_some_and(|blk| !blk.args.is_empty())
        {
            return None;
        }
    }

    // --- Promote the frame's user slots to SSA values of the poll's own control
    //     flow, the first `arity` slots holding the `AllocTask` arguments. A
    //     slot whose reads cannot be answered soundly bails the whole splice. ---
    let arity = caller.blocks[&candidate.alloc_block].ops[candidate.alloc_idx]
        .operands
        .len();
    plan_slots(poll, arity)
}

/// Execute the allocation and CFG-rewrite phase against an unobservable staging
/// function, returning the replacements whose owners the committed caller
/// keeps. `None` discards every mutation made here.
fn apply_fusion_staged(
    caller: &mut TirFunction,
    poll: &TirFunction,
    candidate: &FusionCandidate,
    plan: &SlotPlan,
) -> Option<Replacements> {
    let mut owners = Replacements::new(caller);
    // A parameter slot starts as the frame's reference to its argument: a copy
    // that keeps that reference, bound at the top of the cloned preheader so it
    // dominates every read.
    let bound: Vec<(usize, ValueId)> = {
        let args = &caller.blocks[&candidate.alloc_block].ops[candidate.alloc_idx].operands;
        plan.arguments
            .iter()
            .map(|&position| (position, args[position]))
            .collect()
    };
    let mut arguments: HashMap<usize, ValueId> = HashMap::new();
    let mut preheader_init_ops: Vec<TirOp> = Vec::new();
    for (position, argument) in bound {
        let slot = caller.fresh_value();
        let ty = caller
            .value_types
            .get(&argument)
            .cloned()
            .unwrap_or(TirType::DynBox);
        caller.value_types.insert(slot, ty);
        preheader_init_ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![argument],
            results: vec![slot],
            attrs: Default::default(),
            source_span: None,
        });
        owners.record_held(slot);
        arguments.insert(position, slot);
    }

    // --- Clone + rewrite the poll body into the caller. ---
    let obsolete_consumer_latch_polls = super::super::async_work_poll::loop_only_poll_sites(
        caller,
        candidate.loop_header.unwrap_or(candidate.cond_block),
    );
    // Retire roles while the read-only plan's coordinates still name the
    // original staged body. Cloning/wiring may insert ops into caller blocks.
    // A later bailout discards this stage, including these marker changes.
    for (block, index) in obsolete_consumer_latch_polls {
        assert!(
            caller.blocks.get_mut(&block).unwrap().ops[index].clear_async_work_poll(),
            "generator fusion obsolete-latch plan drifted before rewrite"
        );
    }
    // The clone bails only on a malformed poll. The caller visible to the pass
    // remains untouched because this stage is discarded.
    let clone = clone_and_rewrite_poll(poll, caller, plan, &arguments, &mut owners)?;

    // --- Wire the fused loop. ---
    if !wire_fused_loop(caller, candidate, &clone, preheader_init_ops) {
        return None;
    }

    // SSA-validity is an invariant of the splice, not a hope: a malformed splice
    // panics here rather than silently corrupting the program (mirrors the E1
    // inliner). The `run_pipeline` re-run the driver performs verifies again.
    if let Err(errors) = super::super::super::verify::verify_function(caller) {
        panic!(
            "[generator_fusion] verification failed after splicing poll '{}' into '{}': {:?}",
            candidate.poll_name, caller.name, errors
        );
    }
    Some(owners)
}
