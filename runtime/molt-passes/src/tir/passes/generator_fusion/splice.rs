use std::collections::BTreeSet;

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
        return false;
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
            return false;
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
    let mut slot_infos: Vec<SlotInfo> = Vec::with_capacity(user_slots.len());
    // Pre-materialize init values in the caller. We append const/copy ops into
    // the AllocTask block before the AllocTask (so they dominate the loop).
    let mut preheader_init_ops: Vec<TirOp> = Vec::new();
    for &offset in &user_slots {
        // Param slot? offset == GEN_CONTROL_BYTES + 8*i, i < alloc_args.len().
        let rel = offset - GEN_CONTROL_BYTES;
        if rel % 8 != 0 {
            return false; // non-8-aligned slot — unexpected shape, bail.
        }
        let idx = (rel / 8) as usize;
        if idx < alloc_args.len() {
            // Parameter slot: init = the AllocTask arg (already a caller value).
            slot_infos.push(SlotInfo {
                offset,
                init_caller_val: alloc_args[idx],
            });
            continue;
        }
        // Local slot: init from the poll entry-block init store, materialized as
        // a caller const. We only support a const/None init in Phase 1 (the
        // common `i = 0` / unbound-local case); a non-const local init bails.
        let init_val = match local_slot_init_const(poll, offset) {
            Some(LocalInit::Int(v)) => {
                let nv = caller.fresh_value();
                caller.value_types.insert(nv, TirType::I64);
                preheader_init_ops.push(const_int_op(nv, v));
                nv
            }
            Some(LocalInit::None_) => {
                let nv = caller.fresh_value();
                caller.value_types.insert(nv, TirType::None);
                preheader_init_ops.push(const_none_op(nv));
                nv
            }
            None => return false, // non-trivial local init — bail (Tier D).
        };
        slot_infos.push(SlotInfo {
            offset,
            init_caller_val: init_val,
        });
    }

    // --- Clone + rewrite the poll body into the caller. ---
    let Some(clone) = clone_and_rewrite_poll(poll, caller, &slot_infos) else {
        // The clone bailed (e.g. an unpromotable slot store pattern). Any fresh
        // ids / preheader ops we minted are inert (never inserted into a block),
        // so the caller is still byte-identical.
        return false;
    };

    // --- Wire the fused loop. ---
    if !wire_fused_loop(caller, candidate, &clone, &slot_infos, preheader_init_ops) {
        return false;
    }

    stats.frames_elided += 1;
    stats.yield_sites_spliced += 1;

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
