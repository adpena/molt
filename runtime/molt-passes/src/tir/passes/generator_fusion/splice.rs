use std::collections::{BTreeSet, HashSet};

use super::super::super::blocks::BlockId;
use super::super::super::function::TirFunction;
use super::super::super::op_kinds_generated::opcode_generator_fusion_poll_role_table;
use super::super::super::ops::{OpCode, TirOp};
use super::super::super::types::TirType;
use super::super::super::values::ValueId;
use super::clone::{
    LocalInit, clone_and_rewrite_poll, const_int_op, const_none_op, local_slot_init_const,
};
use super::wire::wire_fused_loop;
use super::{FusionCandidate, FusionStats, GEN_CONTROL_BYTES, SlotInfo, attr_value_int};

#[derive(Clone, Copy)]
enum SlotInitPlan {
    CallerValue(ValueId),
    Int(i64),
    None,
}

#[derive(Clone, Copy)]
struct PlannedSlot {
    offset: i64,
    init: SlotInitPlan,
}

/// Collect the set of USER frame-slot offsets (`>= GEN_CONTROL_BYTES`) the poll
/// body accesses via `ClosureLoad`/`ClosureStore`, in ascending order.
fn collect_user_frame_slots(poll: &TirFunction) -> Vec<i64> {
    let mut slots = BTreeSet::new();
    for block in poll.blocks.values() {
        for op in &block.ops {
            if matches!(op.opcode, OpCode::ClosureLoad | OpCode::ClosureStore)
                && let Some(off) = attr_value_int(op)
                && off >= GEN_CONTROL_BYTES
            {
                slots.insert(off);
            }
        }
    }
    slots.into_iter().collect()
}

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
// slots become loop-carried phis (param slots seeded from the `AllocTask` args,
// local slots from the poll's entry-block init stores).
//
// Multi-yield-SITE generators (sequential `yield a; yield b; ...`) need a
// return-dispatch over yield-delimited segments — doc-26 Phase-1 Finding #1 —
// and bail soundly here (the generator stays Tier D: a correct heap frame).

/// Apply the fusion splice for `candidate`. Returns `true` iff the caller was
/// mutated; `false` on a conservative bail (caller left byte-identical).
pub(in crate::tir::passes::generator_fusion) fn apply_fusion(
    caller: &mut TirFunction,
    poll: &TirFunction,
    candidate: &FusionCandidate,
    stats: &mut FusionStats,
) -> bool {
    let Some(planned_slots) = preflight_fusion(caller, poll, candidate) else {
        return false;
    };

    // Cloning and wiring allocate ids and rewrite CFG, type, label, and loop
    // metadata before every structural surprise can be ruled out. Perform that
    // whole mutable phase on an owned staging function. A conservative bailout
    // drops the stage; the caller (including its allocation counters) remains
    // byte-identical. Successful fusion commits with one move, preserving the
    // exact deterministic id sequence produced by the former in-place path.
    let mut staged = caller.clone();
    if !apply_fusion_staged(&mut staged, poll, candidate, &planned_slots) {
        return false;
    }

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
    true
}

/// Complete every read-only eligibility and slot-initialization check before
/// allocating the transaction's staging copy. Returning `None` cannot mutate
/// either the caller or the fusion statistics.
fn preflight_fusion(
    caller: &TirFunction,
    poll: &TirFunction,
    candidate: &FusionCandidate,
) -> Option<Vec<PlannedSlot>> {
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

    // --- Resolve the AllocTask args (the generator's parameter values, caller
    //     space) so param slots can be seeded. ---
    let alloc_args: Vec<ValueId> = caller.blocks[&candidate.alloc_block].ops[candidate.alloc_idx]
        .operands
        .clone();

    // --- Plan each user slot: offset + caller-space init value. A slot whose
    //     init cannot be resolved soundly bails the whole splice. ---
    let user_slots = collect_user_frame_slots(poll);
    let mut planned_slots = Vec::with_capacity(user_slots.len());
    for &offset in &user_slots {
        // Param slot? offset == GEN_CONTROL_BYTES + 8*i, i < alloc_args.len().
        let rel = offset - GEN_CONTROL_BYTES;
        if rel % 8 != 0 {
            return None; // non-8-aligned slot — unexpected shape, bail.
        }
        let idx = (rel / 8) as usize;
        if idx < alloc_args.len() {
            // Parameter slot: init = the AllocTask arg (already a caller value).
            planned_slots.push(PlannedSlot {
                offset,
                init: SlotInitPlan::CallerValue(alloc_args[idx]),
            });
            continue;
        }
        // Local slot: prove its initializer now, but materialize it only inside
        // the transaction. Phase 1 supports const/None initialization.
        let init = match local_slot_init_const(poll, offset) {
            Some(LocalInit::Int(v)) => SlotInitPlan::Int(v),
            Some(LocalInit::None_) => SlotInitPlan::None,
            None => return None, // non-trivial local init — bail (Tier D).
        };
        planned_slots.push(PlannedSlot { offset, init });
    }

    Some(planned_slots)
}

/// Execute the allocation and CFG-rewrite phase against an unobservable staging
/// function. `false` discards every mutation made here.
fn apply_fusion_staged(
    caller: &mut TirFunction,
    poll: &TirFunction,
    candidate: &FusionCandidate,
    planned_slots: &[PlannedSlot],
) -> bool {
    let mut slot_infos: Vec<SlotInfo> = Vec::with_capacity(planned_slots.len());
    // Materialize local init values in the staged caller. These ops are moved to
    // the cloned preheader after cloning so they dominate the loop-header phis.
    let mut preheader_init_ops: Vec<TirOp> = Vec::new();
    for planned in planned_slots {
        let init_caller_val = match planned.init {
            SlotInitPlan::CallerValue(value) => value,
            SlotInitPlan::Int(value) => {
                let fresh = caller.fresh_value();
                caller.value_types.insert(fresh, TirType::I64);
                preheader_init_ops.push(const_int_op(fresh, value));
                fresh
            }
            SlotInitPlan::None => {
                let fresh = caller.fresh_value();
                caller.value_types.insert(fresh, TirType::None);
                preheader_init_ops.push(const_none_op(fresh));
                fresh
            }
        };
        slot_infos.push(SlotInfo {
            offset: planned.offset,
            init_caller_val,
        });
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
    let Some(clone) = clone_and_rewrite_poll(poll, caller, &slot_infos) else {
        // The clone bailed (e.g. an unpromotable slot store pattern). The caller
        // visible to the pass remains untouched because this stage is discarded.
        return false;
    };

    // --- Wire the fused loop. ---
    if !wire_fused_loop(caller, candidate, &clone, &slot_infos, preheader_init_ops) {
        return false;
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
    true
}
