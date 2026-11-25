use molt_ir::deterministic_hash::{HashMap, HashSet};

use super::super::super::blocks::Terminator;
use super::super::super::function::TirFunction;
use super::super::super::op_kinds_generated::opcode_generator_fusion_poll_role_table;
use super::super::super::ops::{Dialect, OpCode, TirOp};
use super::super::super::types::TirType;
use super::super::super::values::ValueId;
use super::super::ownership_lattice_min::Replacements;
use super::clone::{FusedFrame, clone_and_rewrite_poll};
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
    split_polls: bool,
) -> bool {
    // The poll the splice clones, its body's polls on cold service paths
    // (`frame_states`).
    let prepared = super::frame_states::prepare_poll(poll, split_polls);
    let poll = &prepared;
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
    // The consumer's loop header, its iteration's block and the empty chain
    // to the done test retire; the header's edges are rewired.
    let old_header = candidate.loop_header.unwrap_or(candidate.cond_block);
    let retired: HashSet<_> = std::iter::once(candidate.cond_block)
        .chain(candidate.loop_header)
        .chain(candidate.chain.iter().copied())
        .collect();
    let retired_loops = candidate.loop_header.into_iter().collect();
    caller
        .validate_block_retirement(
            &retired,
            &retired_loops,
            &HashSet::from_iter([old_header]),
            false, // wire_fused_loop rewires edges but does not replace caller.entry_block.
        )
        .ok()?;
    // A separate header holds no operations and enters the iteration directly,
    // and the iteration's done flag drives the loop alone.
    if let Some(header) = candidate.loop_header
        && header != candidate.cond_block
    {
        let block = caller.blocks.get(&header)?;
        if !block.ops.is_empty()
            || !matches!(
                &block.terminator,
                Terminator::Branch { target, args } if *target == candidate.cond_block && args.is_empty()
            )
        {
            return None;
        }
    }
    if !caller.blocks.get(&candidate.cond_block)?.args.is_empty()
        && candidate
            .loop_header
            .is_some_and(|header| header != candidate.cond_block)
    {
        return None;
    }

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

    // --- Promote the frame's user slots to SSA values of the poll's own control
    //     flow, the first `arity` slots holding the `AllocTask` arguments. A
    //     slot whose reads cannot be answered soundly bails the whole splice. A
    //     frame state may name any argument a block entry holds. ---
    let (_, payload) = crate::tir::ops::alloc_task_operands(
        &caller.blocks[&candidate.alloc_block].ops[candidate.alloc_idx].operands,
    );
    let arity = payload.len();
    let mut plan = plan_slots(poll, arity)?;
    let state_arguments = plan.state_arguments();
    plan.arguments.extend(state_arguments);
    Some(plan)
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
        let (_, args) = crate::tir::ops::alloc_task_operands(
            &caller.blocks[&candidate.alloc_block].ops[candidate.alloc_idx].operands,
        );
        plan.arguments
            .iter()
            .map(|&position| (position, args[position]))
            .collect()
    };
    let mut arguments: HashMap<usize, ValueId> = HashMap::default();
    let mut preheader_init_ops: Vec<TirOp> = Vec::new();
    let identity = caller.fresh_value();
    caller.value_types.insert(identity, TirType::I64);
    preheader_init_ops.push(TirOp::new(OpCode::SplicedFrameSlot, vec![], vec![identity]));
    for (position, argument) in bound {
        let slot = caller.fresh_value();
        let ty = caller
            .value_types
            .get(&argument)
            .cloned()
            .unwrap_or(TirType::DynBox);
        caller.value_types.insert(slot, ty);
        preheader_init_ops.push(TirOp {
            origin: None,
            runtime: None,
            callee: None,
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

    // --- Clone + rewrite the poll body into the caller. The consumer's latch
    //     poll stays: the consumer body runs once per fused iteration, so the
    //     fused loop services asynchronous work there, in the consumer's own
    //     frame, with no bracket (spliced_frames.md 5.2). A poll of the
    //     generator body services work in the generator's frame. ---
    // The clone bails only on a malformed poll. The caller visible to the pass
    // remains untouched because this stage is discarded.
    let frame = FusedFrame {
        function: candidate.function,
        identity,
        handler: candidate.handler,
        handler_args: candidate.handler_args.clone(),
    };
    let clone = clone_and_rewrite_poll(poll, caller, plan, &arguments, &mut owners, &frame)?;

    // --- Wire the fused loop. ---
    if !wire_fused_loop(
        caller,
        candidate,
        &clone,
        preheader_init_ops,
        &mut owners,
        identity,
    ) {
        return None;
    }

    // SSA-validity is an invariant of the splice, not a hope: a malformed splice
    // panics here rather than silently corrupting the program (mirrors the E1
    // inliner). The `run_pipeline` re-run the driver performs verifies again.
    if let Err(errors) = super::super::super::verify::verify_function(caller) {
        panic!(
            "[generator_fusion] verification failed after splicing poll #{} into '{}': {:?}",
            candidate.poll.0, caller.name, errors
        );
    }
    Some(owners)
}
