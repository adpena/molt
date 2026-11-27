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

use molt_ir::deterministic_hash::{HashMap, HashSet};

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
    fn after_poll(self, op: &TirOp) -> Self {
        if op.is_async_work_poll()
            && let Some(location) = op.frame_location()
        {
            self.meet(Word::Known(location))
        } else {
            self
        }
    }

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

pub(super) fn home_slot(op: &TirOp) -> Option<usize> {
    match op.attrs.get("value") {
        Some(AttrValue::Int(slot)) => usize::try_from(*slot).ok(),
        _ => None,
    }
}

/// Reverse-post-order is not required for correctness; a sorted order keeps
/// iteration deterministic.
pub(super) fn block_order(func: &TirFunction) -> Vec<BlockId> {
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

pub(super) fn is_frame_home_kind(kind: molt_ir::OpKind) -> bool {
    matches!(
        kind,
        molt_ir::OpKind::FrameHomeStore
            | molt_ir::OpKind::FrameHomeClear
            | molt_ir::OpKind::FrameHomeTake
            | molt_ir::OpKind::FrameHomeCell
            | molt_ir::OpKind::FrameHomePrivateCell
    )
}

/// Explicit runtime entries that bypass the frame home's typed TIR operations.
pub(super) fn direct_frame_runtime(op: &TirOp) -> bool {
    use molt_ir::runtime_fn_generated::RuntimeFn;
    matches!(
        op.runtime,
        Some(
            RuntimeFn::FrameCellPublish
                | RuntimeFn::FrameClassLocalsSet
                | RuntimeFn::FrameContextSet
                | RuntimeFn::FrameHomeLoad
                | RuntimeFn::FrameHomeTake
                | RuntimeFn::FrameLocalsBegin
                | RuntimeFn::FrameLocalsSet
                | RuntimeFn::FrameRecordEnter
                | RuntimeFn::FrameRecordExit
                | RuntimeFn::ClassFrameEnter
                | RuntimeFn::ClassFrameExit
                | RuntimeFn::ClassFrameSetLocation
                | RuntimeFn::TraceSetLine
        )
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
/// its homes in a body admitted for this analysis. The closed-program frame
/// writer fact is checked by `binding_releases` before using this transfer.
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
fn join_slots<T: Copy + Eq>(
    entry: &mut HashMap<BlockId, Vec<T>>,
    target: BlockId,
    slots: &[T],
    meet: impl Fn(T, T) -> T,
) -> bool {
    match entry.get_mut(&target) {
        None => {
            entry.insert(target, slots.to_vec());
            true
        }
        Some(current) => {
            let mut changed = false;
            for (have, incoming) in current.iter_mut().zip(slots) {
                let result = meet(*have, *incoming);
                if *have != result {
                    *have = result;
                    changed = true;
                }
            }
            changed
        }
    }
}

/// A home's facts before each reachable block. A check transfers its current
/// bindings to the exceptional successor before any later store in that block.
fn reaching_slot_entries<T: Copy + Eq>(
    func: &TirFunction,
    labels: &HashMap<i64, BlockId>,
    initial: Vec<T>,
    meet: impl Fn(T, T) -> T + Copy,
    transfer: impl Fn(&TirOp, &mut Vec<T>),
) -> HashMap<BlockId, Vec<T>> {
    let mut entry: HashMap<BlockId, Vec<T>> = HashMap::default();
    // Before the activation's own entry operation nothing is proven.
    entry.insert(func.entry_block, initial);
    let mut pending = vec![func.entry_block];
    let mut enqueued = HashSet::default();
    while let Some(bid) = pending.pop() {
        enqueued.remove(&bid);
        let mut slots = entry[&bid].clone();
        let block = &func.blocks[&bid];
        let mut propagate = |succ, state: &[T]| {
            if join_slots(&mut entry, succ, state, meet) && enqueued.insert(succ) {
                pending.push(succ);
            }
        };
        for op in &block.ops {
            if let Some(handler) = check_handler(op, labels) {
                propagate(handler, &slots);
            }
            transfer(op, &mut slots);
        }
        for succ in dominators::terminator_successors(&block.terminator) {
            if func.blocks.contains_key(&succ) {
                propagate(succ, &slots);
            }
        }
    }
    entry
}

/// Map every binding release to whether its previous binding is
/// finalizer-free, keyed by (block, op index).
pub(super) fn binding_releases(
    func: &TirFunction,
    types: &HashMap<ValueId, TirType>,
    labels: &HashMap<i64, BlockId>,
    order: &[BlockId],
) -> HashMap<(BlockId, usize), bool> {
    // A frame proxy can replace even a previously proven scalar with a
    // reference-bearing value. All consumers share this admission decision.
    if func.frame_locals_exposed() {
        return HashMap::default();
    }
    // A frame proxy can replace even a previously proven scalar with a
    // reference-bearing value. All consumers share this admission decision.
    if func.frame_locals_exposed() {
        return HashMap::default();
    }
    let entry = reaching_slot_entries(
        func,
        labels,
        vec![false; slot_count(func)],
        |a, b| a && b,
        |op, state| {
            slot_transfer(op, func, types, state);
        },
    );
    let mut releases = HashMap::default();
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

/// A reference-bearing argument held by its suspended physical caller, at the
/// original call instruction. Only the post-store view names the actual Python
/// object in the home: a raw full-range integer can box into a different object.
pub(super) fn held_call_arguments(
    func: &TirFunction,
    sites: &molt_ir::deterministic_hash::HashSet<(BlockId, usize)>,
    aliases: &super::alias_analysis::AliasUnionFind,
    carriers: &HashMap<ValueId, crate::repr::Repr>,
) -> molt_ir::deterministic_hash::HashSet<(BlockId, usize, ValueId)> {
    use molt_ir::OpKind;
    use molt_ir::deterministic_hash::HashSet;
    use molt_ir::frame_entry::FrameEntryKind;

    let mut held = HashSet::default();
    if sites.is_empty()
        || func.execution_context != ExecutionContextPolicy::Local
        || func.frame_locals_exposed()
        || func.has_state_machine()
        || func.has_exception_handlers()
    {
        return held;
    }
    let Some(enter) = func.blocks[&func.entry_block].ops.first() else {
        return held;
    };
    let Some(AttrValue::Int(encoded)) = enter.attrs.get("value") else {
        return held;
    };
    let Some(shape) = FrameEntryKind::from_entry_value(*encoded).shape() else {
        return held;
    };
    if enter.opcode != OpCode::Copy
        || enter.origin != Some(OpKind::TraceEnterSlot)
        || shape.entry_value() != *encoded
    {
        return held;
    }

    let labels = dominators::exception_label_to_block(func);
    let Some(entry_failure) = func.blocks[&func.entry_block]
        .ops
        .get(1)
        .and_then(|op| check_handler(op, &labels))
    else {
        return held;
    };
    let mut outside_roots = vec![entry_failure];
    let boxed = |value| {
        carriers
            .get(&value)
            .is_none_or(|r| !r.carrier_refcount_is_noop())
    };
    let requested: HashSet<_> = sites
        .iter()
        .flat_map(|&(id, at)| {
            func.blocks[&id]
                .ops
                .get(at)
                .into_iter()
                .flat_map(|op| &op.operands)
        })
        .copied()
        .filter(|&v| boxed(v))
        .map(|v| aliases.root(v))
        .collect();
    let mut view_slots = HashMap::default();
    let mut tracked_slots = HashMap::default();
    for (&id, block) in &func.blocks {
        let mut exited = false;
        for (at, op) in block.ops.iter().enumerate() {
            if op.origin == Some(OpKind::TraceEnterSlot) && (id != func.entry_block || at != 0) {
                return held;
            }
            if exited && let Some(target) = check_handler(op, &labels) {
                outside_roots.push(target);
            }
            exited |= op.origin == Some(OpKind::TraceExit);
            if direct_frame_runtime(op) {
                return held;
            }
            if !super::frame_home_placement::is_home_store(op) {
                continue;
            }
            let ([_], [view]) = (&*op.operands, &*op.results) else {
                continue;
            };
            if !requested.contains(view)
                || !boxed(*view)
                || super::frame_home_placement::boxes_heap_int(op, carriers)
            {
                continue;
            }
            let Some(slot) = home_slot(op).filter(|&s| s < shape.homes as usize) else {
                continue;
            };
            let next = tracked_slots.len();
            let index = *tracked_slots.entry(slot).or_insert(next);
            view_slots.insert(*view, index);
        }
        if exited {
            outside_roots.extend(dominators::terminator_successors(&block.terminator));
        }
    }
    if view_slots.is_empty() {
        return held;
    }
    let transfer = |op: &TirOp, state: &mut Vec<Option<ValueId>>| {
        if op.opcode != OpCode::Copy {
            return;
        }
        match op.origin {
            Some(
                OpKind::TraceEnterSlot
                | OpKind::TraceExit
                | OpKind::FrameLocalsSet
                | OpKind::FrameContextSet,
            ) => state.fill(None),
            Some(kind) if is_frame_home_kind(kind) => {
                let Some(slot) = home_slot(op) else {
                    state.fill(None);
                    return;
                };
                if let Some(&index) = tracked_slots.get(&slot) {
                    state[index] = op
                        .results
                        .first()
                        .copied()
                        .filter(|v| view_slots.get(v) == Some(&index));
                }
            }
            _ => {}
        }
    };
    // A failed entry has no record, and code after the pop no longer owns a
    // home, including when it rejoins a path on which the activation exists.
    let outside = dominators::reachable_blocks_from_roots_with(
        func,
        outside_roots,
        dominators::CfgEdgePolicy::Full,
    );
    let order = block_order(func);
    let entries = reaching_slot_entries(
        func,
        &labels,
        vec![None; tracked_slots.len()],
        |a, b| if a == b { a } else { None },
        transfer,
    );
    for id in order {
        if outside.contains(&id) {
            continue;
        }
        let Some(mut state) = entries.get(&id).cloned() else {
            continue;
        };
        for (at, op) in func.blocks[&id].ops.iter().enumerate() {
            if op.origin == Some(OpKind::TraceExit) {
                break;
            }
            if sites.contains(&(id, at)) {
                for &arg in &op.operands {
                    let view = aliases.root(arg);
                    if boxed(arg)
                        && view_slots
                            .get(&view)
                            .is_some_and(|&s| state[s] == Some(view))
                    {
                        held.insert((id, at, view));
                    }
                }
            }
            transfer(op, &mut state);
        }
    }
    held
}

// ---------------------------------------------------------------------------
// Reads
// ---------------------------------------------------------------------------

/// Whether a non-check, non-publication operation reads the location word.
/// Fail-closed: any operation whose typed effects may call Python reads it,
/// except a temporary release and a finalizer-free binding release (M2.1).
pub(super) fn operation_reads(
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
pub(super) fn check_uses(op: &TirOp) -> (bool, bool) {
    let materializes = is_origin_check(op) && op.frame_location().is_some();
    let reads = !materializes && (is_origin_check(op) || op.is_async_work_poll());
    (materializes, reads)
}

// ---------------------------------------------------------------------------
// Reaching publications (M1)
// ---------------------------------------------------------------------------

/// Visit each operation that is not a publication with the location word that
/// reaches it in the conservative stream: a forward analysis over terminator
/// edges and each check's exceptional edge.
fn for_each_reaching_word(
    func: &TirFunction,
    labels: &HashMap<i64, BlockId>,
    order: &[BlockId],
    mut visit: impl FnMut(BlockId, usize, &TirOp, Word),
) {
    let mut entry: HashMap<BlockId, Option<Word>> = HashMap::default();
    entry.insert(func.entry_block, Some(Word::Unknown));
    // A block moves from unreached to one known publication, then possibly
    // to unknown. Each transition visits only its own ops and successors.
    let mut pending = vec![func.entry_block];
    while let Some(bid) = pending.pop() {
        let mut word = entry[&bid].expect("queued block has a location fact");
        let block = &func.blocks[&bid];
        let mut propagate = |succ, word| {
            if join_word(entry.entry(succ).or_default(), word) {
                pending.push(succ);
            }
        };
        for op in &block.ops {
            if op.opcode == OpCode::FrameSetLocation {
                if let Some(AttrValue::Int(location)) = op.attrs.get("value") {
                    word = Word::Known(*location);
                }
                continue;
            }
            if let Some(handler) = check_handler(op, labels) {
                propagate(handler, op.frame_location().map_or(word, Word::Known));
            }
            word = word.after_poll(op);
        }
        for succ in dominators::terminator_successors(&block.terminator) {
            if func.blocks.contains_key(&succ) {
                propagate(succ, word);
            }
        }
    }
    for &bid in order {
        let Some(Some(mut word)) = entry.get(&bid).copied() else {
            continue;
        };
        for (index, op) in func.blocks[&bid].ops.iter().enumerate() {
            if op.opcode == OpCode::FrameSetLocation {
                if let Some(AttrValue::Int(location)) = op.attrs.get("value") {
                    word = Word::Known(*location);
                }
                continue;
            }
            visit(bid, index, op, word);
            word = word.after_poll(op);
        }
    }
}

/// The `LocationId` that one publication reaches on every path to each
/// operation of `func` (publications excluded); an operation absent from the
/// map has no single reaching publication. A splice reads the callee's own
/// locations here before it drops the callee's publications
/// (spliced_frames.md 4.5).
pub(crate) fn reaching_locations(func: &TirFunction) -> HashMap<(BlockId, usize), i64> {
    let labels = dominators::exception_label_to_block(func);
    let order = block_order(func);
    let mut locations = HashMap::default();
    for_each_reaching_word(func, &labels, &order, |bid, index, _op, word| {
        if let Word::Known(location) = word {
            locations.insert((bid, index), location);
        }
    });
    locations
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
    let mut attached = Vec::new();
    for_each_reaching_word(func, &labels, &order, |bid, index, op, word| {
        if is_origin_check(op)
            && op.frame_location().is_none()
            && let Word::Known(location) = word
        {
            attached.push((bid, index, location));
        }
    });
    for (bid, index, location) in attached {
        func.blocks.get_mut(&bid).expect("analyzed block").ops[index].set_frame_location(location);
        stats.attrs_changed += 1;
    }

    // 2. Slot contents for binding releases.
    let releases = binding_releases(func, &types, &labels, &order);

    // 3. Backward liveness through the last write or block-entry join.
    // A reader only depends on its current definition. Trace live entries
    // through their incoming definitions once, even at a loop or handler.
    #[derive(Clone, Copy, PartialEq, Eq, Hash)]
    enum Publication {
        Entry(BlockId),
        Store(BlockId, usize),
    }
    let mut incoming: HashMap<BlockId, Vec<Publication>> = HashMap::default();
    let mut pending = Vec::new();
    for &bid in &order {
        let block = &func.blocks[&bid];
        let mut definition = Publication::Entry(bid);
        for (index, op) in block.ops.iter().enumerate() {
            match op.opcode {
                OpCode::FrameSetLocation => definition = Publication::Store(bid, index),
                OpCode::CheckException => {
                    let (materializes, reads) = check_uses(op);
                    if reads {
                        pending.push(definition);
                    } else if !materializes && let Some(handler) = check_handler(op, &labels) {
                        incoming.entry(handler).or_default().push(definition);
                    }
                }
                _ if operation_reads(op, &types, releases.get(&(bid, index)).copied()) => {
                    pending.push(definition);
                }
                _ => {}
            }
        }
        match &block.terminator {
            // Every frame-retiring path ends here; exit freezes the word.
            Terminator::Return { .. } | Terminator::Unwind => pending.push(definition),
            terminator => {
                for succ in dominators::terminator_successors(terminator) {
                    incoming.entry(succ).or_default().push(definition);
                }
            }
        }
    }
    let mut live = HashSet::default();
    while let Some(definition) = pending.pop() {
        if live.insert(definition)
            && let Publication::Entry(bid) = definition
            && let Some(definitions) = incoming.remove(&bid)
        {
            pending.extend(definitions);
        }
    }
    for bid in &order {
        let block = func.blocks.get_mut(bid).expect("analyzed block");
        let before = block.ops.len();
        let mut index = 0usize;
        block.ops.retain(|op| {
            let keep = op.opcode != OpCode::FrameSetLocation
                || live.contains(&Publication::Store(*bid, index));
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
