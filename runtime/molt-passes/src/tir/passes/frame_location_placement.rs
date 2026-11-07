//! Frame-location placement: keep only the publications a reader can observe.
//!
//! The frontend publishes the executing `LocationId` conservatively, before
//! most operations. This terminal pass, which runs after drop insertion, does
//! three things (call_anatomy.md, "Location publication cost"):
//!
//! 1. A forward reaching-publication analysis gives each Origin check that
//!    one publication reaches on every path a `frame_location` attr. Backends
//!    store that constant on the check's cold edges (M1), so the check no
//!    longer reads the word.
//! 2. A forward slot-content analysis proves which frame-home stores and
//!    clears release a finalizer-free previous binding (M2.1).
//! 3. A backward liveness of the location word deletes every publication no
//!    reader observes (M2). Liveness preserves the word's value at each
//!    remaining reader, so every reader sees what the conservative stream
//!    showed it.
//!
//! The pass only adds attrs and deletes `frame_set_location`, so it is
//! idempotent. A target without `cold_origin_frame_location` never runs it.

use std::collections::{BTreeMap, HashMap};

use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::dominators;
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    ExceptionEdgeKind, copy_kind_is_binding_release_table, opcode_is_state_machine_table,
    opcode_is_temporary_release_table,
};
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;
use molt_ir::ExecutionContextPolicy;

use super::PassStats;

/// The location word's value at a program point, in the conservative stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Word {
    /// Exactly this LocationId on every path.
    Known(i64),
    /// Function entry (ABSENT or a resumed saved word) or a join of values.
    Unknown,
}

impl Word {
    fn meet(self, other: Self) -> Self {
        if self == other { self } else { Word::Unknown }
    }
}

fn join_word(slot: &mut Option<Word>, incoming: Word) -> bool {
    let next = slot.map_or(incoming, |current| current.meet(incoming));
    let changed = *slot != Some(next);
    *slot = Some(next);
    changed
}

fn is_origin_check(op: &TirOp) -> bool {
    op.opcode == OpCode::CheckException
        && op.exception_edge_kind() == Some(ExceptionEdgeKind::Origin)
}

/// The executable exceptional successor of a check, if it has one.
fn check_handler(op: &TirOp, labels: &HashMap<i64, BlockId>) -> Option<BlockId> {
    if op.opcode != OpCode::CheckException
        || !dominators::exception_edge_binds_handler_arguments(op.opcode)
    {
        return None;
    }
    match op.attrs.get("value") {
        Some(AttrValue::Int(label)) => labels.get(label).copied(),
        _ => None,
    }
}

fn home_slot(op: &TirOp) -> Option<usize> {
    match op.attrs.get("value") {
        Some(AttrValue::Int(slot)) => usize::try_from(*slot).ok(),
        _ => None,
    }
}

/// Reverse-post-order is not required for correctness; a sorted order keeps
/// iteration deterministic.
fn block_order(func: &TirFunction) -> Vec<BlockId> {
    let mut order: Vec<BlockId> = func.blocks.keys().copied().collect();
    order.sort_by_key(|id| id.0);
    order
}

// ---------------------------------------------------------------------------
// Slot contents (M2.1)
// ---------------------------------------------------------------------------

/// Per-home "previous binding is finalizer-free" facts. `true` means unbound
/// or an exact builtin scalar.
type Slots = Vec<bool>;

fn slot_count(func: &TirFunction) -> usize {
    func.blocks
        .values()
        .flat_map(|block| &block.ops)
        .filter(|op| op.opcode == OpCode::Copy && op.origin.is_some_and(is_frame_home_kind))
        .filter_map(home_slot)
        .map(|slot| slot + 1)
        .max()
        .unwrap_or(0)
}

fn is_frame_home_kind(kind: molt_ir::OpKind) -> bool {
    matches!(
        kind,
        molt_ir::OpKind::FrameHomeStore
            | molt_ir::OpKind::FrameHomeClear
            | molt_ir::OpKind::FrameHomeTake
            | molt_ir::OpKind::FrameHomeCell
            | molt_ir::OpKind::FrameHomePrivateCell
    )
}

/// The slot facts right after this activation's own entry. A fresh
/// synchronous activation starts with every home unbound (the home arena
/// zeroes them); a resumed or inherited activation starts unknown.
fn entry_slots(func: &TirFunction, count: usize) -> Slots {
    let fresh =
        func.execution_context == ExecutionContextPolicy::Local && !func.has_state_machine();
    vec![fresh; count]
}

/// Classify a binding release and update the slot facts.
///
/// Only this activation's own home operations and a `frame_locals_set` write
/// its homes. A callback that rebinds a local through a PEP 667 proxy is the
/// documented boundary of M2.1: it does not clear these facts, so a finalizer
/// of a value injected that way sees the last surviving publication.
fn slot_transfer(
    op: &TirOp,
    func: &TirFunction,
    types: &HashMap<ValueId, TirType>,
    slots: &mut Slots,
) -> SlotRelease {
    if op.opcode != OpCode::Copy {
        return SlotRelease::NotABindingRelease;
    }
    let Some(kind) = op.origin else {
        return SlotRelease::NotABindingRelease;
    };
    match kind {
        molt_ir::OpKind::TraceEnterSlot => {
            let count = slots.len();
            *slots = entry_slots(func, count);
            return SlotRelease::NotABindingRelease;
        }
        molt_ir::OpKind::FrameLocalsSet => {
            slots.iter_mut().for_each(|slot| *slot = false);
            return SlotRelease::NotABindingRelease;
        }
        _ => {}
    }
    if !is_frame_home_kind(kind) {
        return SlotRelease::NotABindingRelease;
    }
    let Some(slot) = home_slot(op).filter(|slot| *slot < slots.len()) else {
        // An unaddressable home is never proven: every slot becomes unknown.
        slots.iter_mut().for_each(|slot| *slot = false);
        return if copy_kind_is_binding_release_table(kind) {
            SlotRelease::MayFinalize
        } else {
            SlotRelease::NotABindingRelease
        };
    };
    let release = if !copy_kind_is_binding_release_table(kind) {
        SlotRelease::NotABindingRelease
    } else if slots[slot] {
        SlotRelease::FinalizerFree
    } else {
        SlotRelease::MayFinalize
    };
    slots[slot] = match kind {
        molt_ir::OpKind::FrameHomeStore => op
            .operands
            .first()
            .is_some_and(|value| types.contains_key(value)),
        molt_ir::OpKind::FrameHomeClear | molt_ir::OpKind::FrameHomeTake => true,
        _ => false,
    };
    release
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SlotRelease {
    NotABindingRelease,
    FinalizerFree,
    MayFinalize,
}

/// Meet incoming slot facts into `target`'s entry: a fact holds only if it
/// holds on every incoming edge.
fn join_slots(entry: &mut HashMap<BlockId, Slots>, target: BlockId, slots: &Slots) -> bool {
    match entry.get_mut(&target) {
        None => {
            entry.insert(target, slots.clone());
            true
        }
        Some(current) => {
            let mut changed = false;
            for (have, incoming) in current.iter_mut().zip(slots) {
                if *have && !incoming {
                    *have = false;
                    changed = true;
                }
            }
            changed
        }
    }
}

/// Map every binding release to whether its previous binding is
/// finalizer-free, keyed by (block, op index).
fn binding_releases(
    func: &TirFunction,
    types: &HashMap<ValueId, TirType>,
    labels: &HashMap<i64, BlockId>,
    order: &[BlockId],
) -> HashMap<(BlockId, usize), bool> {
    let count = slot_count(func);
    let mut entry: HashMap<BlockId, Slots> = HashMap::new();
    // Before the activation's own entry operation nothing is proven.
    entry.insert(func.entry_block, vec![false; count]);
    let mut changed = true;
    while changed {
        changed = false;
        for bid in order {
            let Some(mut slots) = entry.get(bid).cloned() else {
                continue;
            };
            let block = &func.blocks[bid];
            for op in &block.ops {
                if let Some(handler) = check_handler(op, labels) {
                    changed |= join_slots(&mut entry, handler, &slots);
                }
                slot_transfer(op, func, types, &mut slots);
            }
            for succ in dominators::terminator_successors(&block.terminator) {
                if func.blocks.contains_key(&succ) {
                    changed |= join_slots(&mut entry, succ, &slots);
                }
            }
        }
    }
    let mut releases = HashMap::new();
    for bid in order {
        let Some(mut slots) = entry.get(bid).cloned() else {
            continue;
        };
        for (index, op) in func.blocks[bid].ops.iter().enumerate() {
            match slot_transfer(op, func, types, &mut slots) {
                SlotRelease::NotABindingRelease => {}
                SlotRelease::FinalizerFree => {
                    releases.insert((*bid, index), true);
                }
                SlotRelease::MayFinalize => {
                    releases.insert((*bid, index), false);
                }
            }
        }
    }
    releases
}

// ---------------------------------------------------------------------------
// Reads
// ---------------------------------------------------------------------------

/// Whether a non-check, non-publication operation reads the location word.
/// Fail-closed: any operation whose typed effects may call Python reads it,
/// except a temporary release and a finalizer-free binding release (M2.1).
fn operation_reads(
    op: &TirOp,
    types: &HashMap<ValueId, TirType>,
    finalizer_free_release: Option<bool>,
) -> bool {
    debug_assert!(op.opcode != OpCode::FrameSetLocation && op.opcode != OpCode::CheckException);
    if opcode_is_state_machine_table(op.opcode) {
        return true;
    }
    if opcode_is_temporary_release_table(op.opcode) {
        return false;
    }
    if let Some(finalizer_free) = finalizer_free_release {
        return !finalizer_free;
    }
    super::effects::op_effects_with_types(op, types).may_call_python
}

/// How a check uses the word on its exceptional successor and, for a poll,
/// on its service path: whether it writes its own constant, or reads.
fn check_uses(op: &TirOp) -> (bool, bool) {
    let materializes = is_origin_check(op) && op.frame_location().is_some();
    let reads = !materializes && (is_origin_check(op) || op.is_async_work_poll());
    (materializes, reads)
}

// ---------------------------------------------------------------------------
// The pass
// ---------------------------------------------------------------------------

pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "frame_location_placement",
        ..Default::default()
    };
    let has_publication = func.blocks.values().any(|block| {
        block
            .ops
            .iter()
            .any(|op| op.opcode == OpCode::FrameSetLocation)
    });
    if !has_publication {
        return stats;
    }
    let types = crate::tir::type_refine::extract_exact_scalar_map(func);
    let labels = dominators::exception_label_to_block(func);
    let order = block_order(func);

    // 1. Reaching publications: attach `frame_location` to Origin checks.
    let mut entry: HashMap<BlockId, Option<Word>> = HashMap::new();
    entry.insert(func.entry_block, Some(Word::Unknown));
    let mut changed = true;
    while changed {
        changed = false;
        for bid in &order {
            let Some(Some(mut word)) = entry.get(bid).copied() else {
                continue;
            };
            let block = &func.blocks[bid];
            for op in &block.ops {
                if op.opcode == OpCode::FrameSetLocation {
                    if let Some(AttrValue::Int(location)) = op.attrs.get("value") {
                        word = Word::Known(*location);
                    }
                    continue;
                }
                if let Some(handler) = check_handler(op, &labels) {
                    let edge = op.frame_location().map_or(word, Word::Known);
                    changed |= join_word(entry.entry(handler).or_default(), edge);
                }
                if op.is_async_work_poll()
                    && let Some(location) = op.frame_location()
                {
                    word = word.meet(Word::Known(location));
                }
            }
            for succ in dominators::terminator_successors(&block.terminator) {
                if func.blocks.contains_key(&succ) {
                    changed |= join_word(entry.entry(succ).or_default(), word);
                }
            }
        }
    }
    let mut attached = Vec::new();
    for bid in &order {
        let Some(Some(mut word)) = entry.get(bid).copied() else {
            continue;
        };
        for (index, op) in func.blocks[bid].ops.iter().enumerate() {
            if op.opcode == OpCode::FrameSetLocation {
                if let Some(AttrValue::Int(location)) = op.attrs.get("value") {
                    word = Word::Known(*location);
                }
                continue;
            }
            if is_origin_check(op)
                && op.frame_location().is_none()
                && let Word::Known(location) = word
            {
                attached.push((*bid, index, location));
            }
        }
    }
    for (bid, index, location) in attached {
        func.blocks.get_mut(&bid).expect("analyzed block").ops[index].set_frame_location(location);
        stats.attrs_changed += 1;
    }

    // 2. Slot contents for binding releases.
    let releases = binding_releases(func, &types, &labels, &order);

    // 3. Backward liveness of the word.
    let mut live_in: HashMap<BlockId, bool> = HashMap::new();
    let mut needed: BTreeMap<(BlockId, usize), bool> = BTreeMap::new();
    let mut changed = true;
    while changed {
        changed = false;
        for bid in order.iter().rev() {
            let block = &func.blocks[bid];
            let mut live = match &block.terminator {
                // Every frame-retiring path ends here; exit freezes the word.
                Terminator::Return { .. } => true,
                terminator => dominators::terminator_successors(terminator)
                    .into_iter()
                    .any(|succ| live_in.get(&succ).copied().unwrap_or(false)),
            };
            for (index, op) in block.ops.iter().enumerate().rev() {
                if op.opcode == OpCode::FrameSetLocation {
                    needed.insert((*bid, index), live);
                    live = false;
                    continue;
                }
                if op.opcode == OpCode::CheckException {
                    let (materializes, reads) = check_uses(op);
                    let edge = if materializes {
                        false
                    } else {
                        reads
                            || check_handler(op, &labels).is_some_and(|handler| {
                                live_in.get(&handler).copied().unwrap_or(false)
                            })
                    };
                    live |= edge;
                    continue;
                }
                if operation_reads(op, &types, releases.get(&(*bid, index)).copied()) {
                    live = true;
                }
            }
            if live_in.get(bid).copied() != Some(live) {
                live_in.insert(*bid, live);
                changed = true;
            }
        }
    }
    for bid in &order {
        let block = func.blocks.get_mut(bid).expect("analyzed block");
        let before = block.ops.len();
        let mut index = 0usize;
        block.ops.retain(|op| {
            let keep = op.opcode != OpCode::FrameSetLocation
                || needed.get(&(*bid, index)).copied().unwrap_or(true);
            index += 1;
            keep
        });
        stats.ops_removed += before - block.ops.len();
    }
    stats
}

#[cfg(test)]
#[path = "frame_location_placement_tests.rs"]
mod tests;
