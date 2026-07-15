//! The module-slot-promotion transform: per-loop legality classification
//! ([`promote_loop`]) and the SSA rewrite ([`apply_promotion`]) that carries
//! promoted slots as header block-args, inserts loop-exit store-backs, and
//! routes dirty-state `CheckException` observers through compensation blocks.

use std::collections::{HashMap, HashSet};

use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::dominators::{CfgEdgePolicy, build_pred_map_with, terminator_successors};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{ModuleSlotAccessRole, opcode_module_slot_access_role_table};
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::values::{TirValue, ValueId};

use super::super::alias_analysis::{AliasAnalysisResult, MemRegion};
use super::DebugLog;
use super::PromotionStats;
use super::gates::is_marker_passthrough;
use super::loops::LoopInfo;
use super::terminators::{
    append_args_on_edges_to, edge_args, retarget_edge, rewrite_terminator_values, terminator_uses,
};

/// A const-named module-attr access inside (or before) a loop.
#[derive(Debug, Clone)]
struct SlotAccess {
    block: BlockId,
    op_index: usize,
    /// True for `ModuleSetAttr` (operands `[module, name, value]`), false for
    /// `ModuleGetAttr` (operands `[module, name]`, one result).
    is_set: bool,
    /// The stored value (set) or the loaded result (get).
    value: ValueId,
}

/// The per-(loop, slot) classification gathered before any mutation.
struct LoopPlan {
    /// Promoted slot names in deterministic order.
    slots: Vec<String>,
    /// Per-slot seed value on the preheader edge.
    entry_values: Vec<ValueId>,
    /// In-loop accesses, per slot.
    accesses: Vec<Vec<SlotAccess>>,
    /// Preheader gets that must be INSERTED (header-accessed slots with no
    /// preheader access): (slot index, fresh name-const ValueId).
    hoisted_loads: Vec<(usize, ValueId)>,
}

pub(super) fn promote_loop(
    func: &mut TirFunction,
    lp: &LoopInfo,
    module_root: ValueId,
    names: &HashMap<ValueId, String>,
    alias: &AliasAnalysisResult,
    stats: &mut PromotionStats,
    dbg: &mut DebugLog,
) -> bool {
    // ---- legality + classification (no mutation) ---------------------------
    let mut accesses_by_slot: HashMap<String, Vec<SlotAccess>> = HashMap::new();

    for &bid in &lp.linear_order {
        let block = &func.blocks[&bid];
        for (op_index, op) in block.ops.iter().enumerate() {
            match opcode_module_slot_access_role_table(op.opcode) {
                ModuleSlotAccessRole::KeyedAttr => {
                    // Const-named (wildcards were rejected function-wide); must
                    // be on THE module root.
                    let m = alias.root(op.operands[0]);
                    if m != module_root {
                        dbg.note(format!(
                            "{} loop@{:?}: refused (module op on non-root object)",
                            func.name, lp.header
                        ));
                        return false;
                    }
                    let name = names[&op.operands[1]].clone();
                    let is_set = op.opcode == OpCode::ModuleSetAttr;
                    let value = if is_set {
                        op.operands[2]
                    } else {
                        op.results[0]
                    };
                    accesses_by_slot.entry(name).or_default().push(SlotAccess {
                        block: bid,
                        op_index,
                        is_set,
                        value,
                    });
                }
                _ if op.opcode == OpCode::CheckException => {} // compensated, not a barrier
                _ => {
                    // Pure, movable ops (the licm-canonical S3 predicate) and
                    // plain value copies cannot observe or mutate any memory —
                    // never barriers. (The alias oracle's coarse taxonomy
                    // defaults unlisted ops like `Copy` to `GenericHeap`,
                    // which would otherwise alias everything.) Everything else
                    // barriers when its region may alias the module dict.
                    if crate::tir::passes::effects::opcode_is_pure_movable(op.opcode)
                        || op.is_plain_value_copy()
                        || is_marker_passthrough(op)
                    {
                        continue;
                    }
                    if alias.region_of(op).may_alias(&MemRegion::ModuleDict) {
                        let orig = match op.attrs.get("_original_kind") {
                            Some(AttrValue::Str(k)) => format!(" (_original_kind={k})"),
                            _ => String::new(),
                        };
                        dbg.note(format!(
                            "{} loop@{:?}: refused (barrier op {:?}{} in loop)",
                            func.name, lp.header, op.opcode, orig
                        ));
                        return false;
                    }
                }
            }
        }
    }
    if accesses_by_slot.is_empty() {
        return false;
    }

    // No in-loop module-op result may be used outside the loop (LCSSA refusal).
    let in_loop_results: HashSet<ValueId> = accesses_by_slot
        .values()
        .flatten()
        .filter(|a| !a.is_set)
        .map(|a| a.value)
        .collect();
    for (bid, block) in &func.blocks {
        if lp.blocks.contains(bid) {
            continue;
        }
        let uses_outside = block
            .ops
            .iter()
            .any(|op| op.operands.iter().any(|v| in_loop_results.contains(v)))
            || terminator_uses(&block.terminator, &in_loop_results);
        if uses_outside {
            dbg.note(format!(
                "{} loop@{:?}: refused (in-loop module value used outside; LCSSA)",
                func.name, lp.header
            ));
            return false;
        }
    }

    // Entry availability per slot.
    let mut slots: Vec<String> = accesses_by_slot.keys().cloned().collect();
    slots.sort();
    // The entry-seed scan walks ops BACKWARDS across the straight-line chain
    // of blocks ending at the preheader (the lift often splits the set-up
    // block from the loop with tiny pass-through blocks, so the seeds live a
    // block or two back). Chain membership: walk back while the current block
    // has exactly one predecessor and that predecessor unconditionally falls
    // through (single successor). Ops are cloned so the per-slot walk below
    // can allocate fresh ids on `func` without a live borrow of `func.blocks`.
    let preheader_ops: Vec<TirOp> = {
        let pred_map = build_pred_map_with(func, CfgEdgePolicy::TerminatorOnly);
        let mut chain = vec![lp.preheader];
        let mut cur = lp.preheader;
        loop {
            let preds = pred_map.get(&cur).map(Vec::as_slice).unwrap_or(&[]);
            let [single] = preds else { break };
            if terminator_successors(&func.blocks[single].terminator).len() != 1 {
                break;
            }
            chain.push(*single);
            cur = *single;
        }
        // Oldest block first, so a reversed scan sees the LAST access first.
        chain.reverse();
        chain
            .iter()
            .flat_map(|b| func.blocks[b].ops.iter().cloned())
            .collect()
    };
    let mut entry_values = Vec::new();
    let mut hoisted_loads = Vec::new();
    for (slot_idx, slot) in slots.iter().enumerate() {
        // Walk the preheader block backwards for the LAST access of this slot
        // with no ModuleDict-aliasing barrier after it.
        let mut found: Option<ValueId> = None;
        for op in preheader_ops.iter().rev() {
            match opcode_module_slot_access_role_table(op.opcode) {
                ModuleSlotAccessRole::KeyedAttr => {
                    if alias.root(op.operands[0]) == module_root
                        && names.get(&op.operands[1]).map(String::as_str) == Some(slot.as_str())
                    {
                        found = Some(if op.opcode == OpCode::ModuleSetAttr {
                            op.operands[2]
                        } else {
                            op.results[0]
                        });
                        break;
                    }
                    // A different slot's const-named access: key-disjoint, keep
                    // walking.
                }
                _ if op.opcode == OpCode::CheckException => {}
                _ if crate::tir::passes::effects::opcode_is_pure_movable(op.opcode)
                    || op.is_plain_value_copy()
                    || is_marker_passthrough(op) => {}
                _ if alias.region_of(op).may_alias(&MemRegion::ModuleDict) => break,
                _ => {}
            }
        }
        match found {
            Some(v) => {
                entry_values.push(v);
            }
            None => {
                // Hoist a preheader load — legal ONLY when (a) the slot's
                // first in-loop access is in the header block (executes on
                // every entry, so the load raises exactly where iteration 1
                // would), AND (b) the function has no real exception handlers:
                // a hoisted load that raises is observed by the FIRST
                // CheckException after it rather than the one following the
                // original get, which under `try` handlers could route to a
                // DIFFERENT handler. Handler-free functions have a single
                // function-exit label, so routing is identical either way.
                if func.has_exception_handlers() {
                    dbg.note(format!(
                        "{} loop@{:?}: refused (hoisted load for '{}' in handler-bearing fn)",
                        func.name, lp.header, slot
                    ));
                    return false;
                }
                // The guaranteed-on-entry prefix: the linear blocks from the
                // header through the FIRST block that can leave the loop (an
                // in-loop CondBranch/Switch or an exit edge). Every block in
                // this prefix executes on every loop entry, so a preheader
                // load raises exactly where the first iteration's access
                // would. (The lift often leaves the header itself an empty
                // join and puts the condition gets one block later.)
                let mut guaranteed: HashSet<BlockId> = HashSet::new();
                for &b in &lp.linear_order {
                    guaranteed.insert(b);
                    let succs = terminator_successors(&func.blocks[&b].terminator);
                    let conditional =
                        succs.len() > 1 || succs.iter().any(|s| !lp.blocks.contains(s));
                    if conditional {
                        break;
                    }
                }
                let first_in_guaranteed = accesses_by_slot[slot]
                    .iter()
                    .min_by_key(|a| {
                        (
                            lp.linear_order.iter().position(|b| *b == a.block),
                            a.op_index,
                        )
                    })
                    .is_some_and(|a| guaranteed.contains(&a.block));
                if !first_in_guaranteed {
                    dbg.note(format!(
                        "{} loop@{:?}: refused (slot '{}' not entry-available)",
                        func.name, lp.header, slot
                    ));
                    return false;
                }
                // The name const must dominate the preheader insertion point.
                // The in-loop ConstStr does NOT; synthesize a fresh ConstStr in
                // the preheader instead (constants are position-free).
                let fresh_name = func.fresh_value();
                let fresh_load = func.fresh_value();
                hoisted_loads.push((slot_idx, fresh_name));
                entry_values.push(fresh_load);
            }
        }
    }

    let plan = LoopPlan {
        accesses: slots.iter().map(|s| accesses_by_slot[s].clone()).collect(),
        slots,
        entry_values,
        hoisted_loads,
    };

    apply_promotion(func, lp, module_root, &plan, stats);
    true
}

fn apply_promotion(
    func: &mut TirFunction,
    lp: &LoopInfo,
    module_root: ValueId,
    plan: &LoopPlan,
    stats: &mut PromotionStats,
) {
    let n = plan.slots.len();
    stats.slots_promoted += n;

    // ---- 1. hoisted preheader loads (fresh ConstStr + ModuleGetAttr) -------
    // entry_values for hoisted slots were pre-allocated as fresh ids; define
    // them now at the end of the preheader.
    {
        let mut hoist_ops = Vec::new();
        for &(slot_idx, fresh_name) in &plan.hoisted_loads {
            let mut name_attrs = crate::tir::ops::AttrDict::new();
            name_attrs.insert(
                "s_value".into(),
                AttrValue::Str(plan.slots[slot_idx].clone()),
            );
            hoist_ops.push(TirOp {
                dialect: crate::tir::ops::Dialect::Molt,
                opcode: OpCode::ConstStr,
                operands: vec![],
                results: vec![fresh_name],
                attrs: name_attrs,
                source_span: None,
            });
            hoist_ops.push(TirOp {
                dialect: crate::tir::ops::Dialect::Molt,
                opcode: OpCode::ModuleGetAttr,
                operands: vec![module_root, fresh_name],
                results: vec![plan.entry_values[slot_idx]],
                attrs: crate::tir::ops::AttrDict::new(),
                source_span: None,
            });
        }
        let pre = func.blocks.get_mut(&lp.preheader).expect("preheader");
        pre.ops.extend(hoist_ops);
    }

    // ---- 2. header phi args -------------------------------------------------
    // One fresh carried value per slot; type = the entry value's known type
    // (DynBox floor keeps Repr sound — value_range re-proves on the merged
    // body during the post-pass re-pipeline).
    let carried: Vec<ValueId> = (0..n).map(|_| func.fresh_value()).collect();
    for (i, &cv) in carried.iter().enumerate() {
        let ty = func
            .value_types
            .get(&plan.entry_values[i])
            .cloned()
            .unwrap_or(crate::tir::types::TirType::DynBox);
        func.value_types.insert(cv, ty.clone());
        let header = func.blocks.get_mut(&lp.header).expect("header");
        header.args.push(TirValue { id: cv, ty });
    }

    // Preheader edge passes the entry values; back edges pass the (renamed)
    // latch values — appended AFTER renaming computes them (step 4).
    {
        let pre = func.blocks.get_mut(&lp.preheader).expect("preheader");
        append_args_on_edges_to(&mut pre.terminator, lp.header, &plan.entry_values);
    }

    // ---- 3. rename through the linear body ---------------------------------
    // cur[i] = the SSA value of slot i at the current program point.
    //
    // DIRTINESS IS LOOP-LEVEL, NOT POSITIONAL: on iteration ≥2 the back edge
    // makes ANY in-loop set reach EVERY program point in the loop, so a slot
    // with at least one set is dirty at every CheckException and every exit —
    // even ones that appear BEFORE the set in linear block order. (A positional
    // dirty bit would skip compensation for a check that precedes the set in
    // block order yet runs after it at runtime — a wrong-observable-state
    // miscompile on iteration 2+.) A never-set slot is never dirty (its dict
    // value is already correct) and needs no stores anywhere.
    let slot_dirty: Vec<bool> = plan
        .accesses
        .iter()
        .map(|accs| accs.iter().any(|a| a.is_set))
        .collect();
    let any_dirty = slot_dirty.iter().any(|&d| d);
    let mut cur: Vec<ValueId> = carried.clone();
    // value replacement map for deleted gets: old result -> current value.
    let mut replace: HashMap<ValueId, ValueId> = HashMap::new();
    // Per-block end values for back-edge args + exit-edge store-backs.
    let mut values_at_block_end: HashMap<BlockId, Vec<ValueId>> = HashMap::new();
    // Compensation blocks to create for dirty CheckExceptions.
    struct Compensation {
        check_block: BlockId,
        check_op_index: usize,
        values: Vec<ValueId>,
        original_label: i64,
        original_operands: Vec<ValueId>,
    }
    let mut compensations: Vec<Compensation> = Vec::new();

    let slot_index_of_access: HashMap<(BlockId, usize), usize> = plan
        .accesses
        .iter()
        .enumerate()
        .flat_map(|(i, accs)| accs.iter().map(move |a| ((a.block, a.op_index), i)))
        .collect();

    for &bid in &lp.linear_order {
        let block = func.blocks.get_mut(&bid).expect("loop block");
        let mut new_ops: Vec<TirOp> = Vec::with_capacity(block.ops.len());
        for (op_index, op) in block.ops.iter().enumerate() {
            if let Some(&slot) = slot_index_of_access.get(&(bid, op_index)) {
                if op.opcode == OpCode::ModuleSetAttr {
                    // Redefine the carried value; delete the store.
                    cur[slot] = op.operands[2];
                } else {
                    // Replace the get's result with the carried value; delete.
                    replace.insert(op.results[0], cur[slot]);
                }
                stats.ops_eliminated += 1;
                continue;
            }
            if op.opcode == OpCode::CheckException && any_dirty {
                let label = match op.attrs.get("value") {
                    Some(AttrValue::Int(l)) => *l,
                    _ => {
                        // No label → keep as-is (defensive; nothing to retarget).
                        new_ops.push(op.clone());
                        continue;
                    }
                };
                compensations.push(Compensation {
                    check_block: bid,
                    check_op_index: new_ops.len(),
                    values: cur.clone(),
                    original_label: label,
                    original_operands: op.operands.clone(),
                });
                // The op itself is rewritten in step 5 (fresh label, no
                // operands); push a placeholder clone for now.
                new_ops.push(op.clone());
                continue;
            }
            new_ops.push(op.clone());
        }
        block.ops = new_ops;
        values_at_block_end.insert(bid, cur.clone());
    }

    // Apply the get-result replacement everywhere in the loop (operands and
    // terminators; outside uses were refused pre-transform). Resolve chains
    // (get B replaced by get A's replacement).
    let resolve = |v: ValueId, replace: &HashMap<ValueId, ValueId>| -> ValueId {
        let mut cur = v;
        while let Some(&next) = replace.get(&cur) {
            cur = next;
        }
        cur
    };
    for &bid in &lp.linear_order {
        let block = func.blocks.get_mut(&bid).expect("loop block");
        for op in &mut block.ops {
            for v in &mut op.operands {
                *v = resolve(*v, &replace);
            }
        }
        rewrite_terminator_values(&mut block.terminator, &|v| resolve(v, &replace));
    }
    // Carried values may appear in the recorded states; resolve them too.
    for vals in values_at_block_end.values_mut() {
        for v in vals.iter_mut() {
            *v = resolve(*v, &replace);
        }
    }
    for c in &mut compensations {
        for v in &mut c.values {
            *v = resolve(*v, &replace);
        }
        for v in &mut c.original_operands {
            *v = resolve(*v, &replace);
        }
    }

    // ---- 4. back edges + exit edges -----------------------------------------
    // Back edges (in-loop preds of the header): append the latch-end values.
    let in_loop_header_preds: Vec<BlockId> = lp
        .linear_order
        .iter()
        .copied()
        .filter(|b| terminator_successors(&func.blocks[b].terminator).contains(&lp.header))
        .collect();
    for pred in &in_loop_header_preds {
        let vals = values_at_block_end[pred].clone();
        let block = func.blocks.get_mut(pred).expect("latch");
        append_args_on_edges_to(&mut block.terminator, lp.header, &vals);
    }

    // Stray header predecessors: UNREACHABLE blocks (e.g. a vestigial
    // loop-else) may still physically branch to the header. Discovery ignored
    // them (correctly — they never execute), but the verifier checks branch
    // ARITY on every block regardless of reachability, so their edges must be
    // padded too. The entry seeds are well-formed dominating-exempt values for
    // a dead edge; semantics are unaffected.
    let updated: HashSet<BlockId> = in_loop_header_preds
        .iter()
        .copied()
        .chain([lp.preheader])
        .collect();
    let stray_preds: Vec<BlockId> = func
        .blocks
        .iter()
        .filter(|(bid, b)| {
            !updated.contains(bid) && terminator_successors(&b.terminator).contains(&lp.header)
        })
        .map(|(bid, _)| *bid)
        .collect();
    for pred in stray_preds {
        let block = func.blocks.get_mut(&pred).expect("stray pred");
        append_args_on_edges_to(&mut block.terminator, lp.header, &plan.entry_values);
    }

    // Exit edges: split each in-loop→outside edge with a store-back block for
    // the dirty slots at that block's end.
    let exits: Vec<(BlockId, BlockId)> = lp
        .linear_order
        .iter()
        .flat_map(|&b| {
            terminator_successors(&func.blocks[&b].terminator)
                .into_iter()
                .filter(|s| !lp.blocks.contains(s))
                .map(move |s| (b, s))
        })
        .collect();
    for (from, to) in exits {
        if !any_dirty {
            continue;
        }
        let vals = values_at_block_end[&from].clone();
        let store_ops = alloc_store_back_ops(func, module_root, &plan.slots, &vals, &slot_dirty);
        // The new edge block forwards the original edge args unchanged.
        let edge_block = func.fresh_block();
        let original_args = edge_args(&func.blocks[&from].terminator, to);
        func.blocks.insert(
            edge_block,
            TirBlock {
                id: edge_block,
                args: vec![],
                ops: store_ops,
                terminator: Terminator::Branch {
                    target: to,
                    args: original_args,
                },
            },
        );
        let from_block = func.blocks.get_mut(&from).expect("exit pred");
        retarget_edge(&mut from_block.terminator, to, edge_block);
    }

    // ---- 5. compensation blocks for dirty CheckExceptions -------------------
    if !compensations.is_empty() {
        let label_to_block: HashMap<i64, BlockId> = func
            .label_id_map
            .iter()
            .map(|(b, l)| (*l, BlockId(*b)))
            .collect();
        let mut next_label = func.label_id_map.values().copied().max().unwrap_or(0) + 1;
        for c in compensations {
            let Some(&handler_block) = label_to_block.get(&c.original_label) else {
                continue; // unresolvable label: leave the op untouched (sound).
            };
            let comp_ops =
                alloc_store_back_ops(func, module_root, &plan.slots, &c.values, &slot_dirty);
            let comp_block = func.fresh_block();
            let fresh_label = next_label;
            next_label += 1;
            func.label_id_map.insert(comp_block.0, fresh_label);
            func.blocks.insert(
                comp_block,
                TirBlock {
                    id: comp_block,
                    args: vec![],
                    ops: comp_ops,
                    terminator: Terminator::Branch {
                        target: handler_block,
                        args: c.original_operands.clone(),
                    },
                },
            );
            let block = func.blocks.get_mut(&c.check_block).expect("check block");
            let op = &mut block.ops[c.check_op_index];
            debug_assert_eq!(op.opcode, OpCode::CheckException);
            op.attrs.insert("value".into(), AttrValue::Int(fresh_label));
            op.operands.clear();
        }
    }
}

/// Build the `ConstStr name` + `ModuleSetAttr` pairs that store the dirty
/// slots back. Fresh ConstStr ops are synthesized per block (the loop's name
/// consts may not dominate the new blocks; string constants are position-free)
/// — which requires `&mut func` for the fresh result ids, so call this BEFORE
/// taking any other borrow of `func.blocks`.
fn alloc_store_back_ops(
    func: &mut TirFunction,
    module_root: ValueId,
    slots: &[String],
    values: &[ValueId],
    dirty: &[bool],
) -> Vec<TirOp> {
    let mut ops = Vec::new();
    for (i, slot) in slots.iter().enumerate() {
        if !dirty[i] {
            continue;
        }
        let name_id = func.fresh_value();
        func.value_types
            .insert(name_id, crate::tir::types::TirType::Str);
        let mut name_attrs = crate::tir::ops::AttrDict::new();
        name_attrs.insert("s_value".into(), AttrValue::Str(slot.clone()));
        ops.push(TirOp {
            dialect: crate::tir::ops::Dialect::Molt,
            opcode: OpCode::ConstStr,
            operands: vec![],
            results: vec![name_id],
            attrs: name_attrs,
            source_span: None,
        });
        ops.push(TirOp {
            dialect: crate::tir::ops::Dialect::Molt,
            opcode: OpCode::ModuleSetAttr,
            operands: vec![module_root, name_id, values[i]],
            results: vec![],
            attrs: crate::tir::ops::AttrDict::new(),
            source_span: None,
        });
    }
    ops
}
