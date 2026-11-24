//! Frame-slot promotion for generator fusion: the value each read of a user
//! frame slot sees.
//!
//! A generator keeps its parameters and spilled locals in user frame slots
//! (`offset >= GEN_CONTROL_BYTES`) across suspensions. Fused, the frame is gone,
//! and each slot is an SSA variable of the poll's own control flow: a
//! `ClosureLoad` of the frame reads the definition that reaches it, a
//! `ClosureStore` makes a new definition, and a join that different definitions
//! reach takes a block argument. [`plan_slots`] is the classic construction over
//! the poll's terminator CFG, computed before anything is cloned. The joins are
//! the iterated dominance frontier of each slot's storing blocks, pruned to the
//! blocks the slot is live into. A dominator-tree walk binds every read to its
//! reaching definition, and every edge into a join to the definition leaving its
//! source. The consumer body, spliced in at the yield, touches no slot, so the
//! poll's own CFG decides every reaching definition.
//!
//! On entry a parameter slot holds the frame's reference to its argument, and
//! a local slot holds nothing. The plan refuses a fusion rather than change what
//! a read sees:
//! * a read that some path reaches without a store to its local slot, where
//!   the frame would read its zeroed slot;
//! * a slot live into a block that an exception edge enters, or an edge from a
//!   block that only an exception edge enters back into the body. Such a block
//!   sees the slot state from the middle of its raising block. A Phase-1 poll
//!   has no handler, so its exception exits read no slot and only return;
//! * the frame used other than as the base of a slot access, where it escapes;
//! * a store of a value the splice drops with its op (frame bookkeeping, an
//!   exception-stack save), which no read could copy;
//! * a user slot that is not a whole 8-byte slot.
//!
//! A block that the poll never enters holds no slot state, and the splice
//! prunes it.

use molt_ir::deterministic_hash::{HashMap, HashSet};
use std::collections::{BTreeMap, BTreeSet};

use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::dominators::{
    CfgEdgePolicy, build_dom_children, build_pred_map_with, compute_dominance_frontiers,
    compute_dominator_tree_with, dom_tree_preorder, exception_label_to_block, exception_successors,
    iterated_dominance_frontier, reachable_blocks_with,
};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::ExceptionEdgeKind;
use crate::tir::ops::{OpCode, TirOp};
use crate::tir::values::ValueId;
use molt_ir::runtime_fn_generated::RuntimeFn;

use super::clone::{exception_stack_values, is_bookkeeping_op};
use super::{GEN_CONTROL_BYTES, attr_value_int};

/// Where a promoted slot's value comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SlotDef {
    /// The frame's reference to the generator argument at this position.
    Argument(usize),
    /// The frame's reference to the value that the store at op `index` of
    /// `block` put in the slot.
    Stored { block: BlockId, index: usize },
    /// The argument that join block `block` takes for slot `slot`.
    Join { block: BlockId, slot: usize },
}

/// The promotion of a poll's user frame slots. A slot is its index in the
/// ascending list of promoted frame offsets.
pub(super) struct SlotPlan {
    /// The poll's frame parameter.
    pub(super) frame: ValueId,
    /// The argument positions whose frame reference some read may see.
    pub(super) arguments: BTreeSet<usize>,
    /// Each promoted store, by block and op index.
    pub(super) stores: BTreeSet<(BlockId, usize)>,
    /// The reaching definition of each promoted read, by the poll value the
    /// `ClosureLoad` defined.
    pub(super) reads: HashMap<ValueId, SlotDef>,
    /// Per join block, the slots its appended arguments carry, ascending.
    pub(super) joins: BTreeMap<BlockId, Vec<usize>>,
    /// Joins reachable from an unbound predecessor without an intervening
    /// store. These carry a Boolean presence argument alongside the value.
    pub(super) optional_joins: BTreeSet<(BlockId, usize)>,
    /// Per block with an edge into a join, each slot's definition at its exit.
    pub(super) exits: HashMap<BlockId, Vec<Option<SlotDef>>>,
    /// Each slot's frame offset, by slot.
    pub(super) offsets: Vec<i64>,
    /// Per block of the poll's own control flow, each slot's definition at
    /// its start (after its joins): the frame state a walk of its operations
    /// begins from (spliced_frames.md 4.5).
    pub(super) entry: HashMap<BlockId, Vec<Option<SlotDef>>>,
}

impl SlotPlan {
    /// The argument positions some block-entry state holds: a frame state at
    /// an observer point may name them.
    pub(super) fn state_arguments(&self) -> BTreeSet<usize> {
        self.entry
            .values()
            .flatten()
            .filter_map(|def| match def {
                Some(SlotDef::Argument(position)) => Some(*position),
                _ => None,
            })
            .collect()
    }

    /// The slot that holds the frame offset `offset`.
    pub(super) fn slot_of(&self, offset: i64) -> Option<usize> {
        self.offsets.iter().position(|&slot| slot == offset)
    }
}

/// One access to the frame.
enum FrameAccess {
    /// A read of the slot at `offset` into `result`.
    Read { offset: i64, result: ValueId },
    /// A store of `value` into the slot at `offset`, by the op at `index`.
    Write {
        offset: i64,
        value: ValueId,
        index: usize,
    },
}

/// The frame access `op`, at `index` in its block, makes: `Ok(None)` when it is
/// no access, or a frame operation the splice drops; `Err` when the frame
/// escapes through it.
fn frame_access(op: &TirOp, index: usize, frame: ValueId) -> Result<Option<FrameAccess>, ()> {
    if !op.operands.contains(&frame) {
        return Ok(None);
    }
    let offset = attr_value_int(op);
    match op.opcode {
        OpCode::ClosureLoad if op.operands.len() == 1 && op.results.len() == 1 => {
            let offset = offset.ok_or(())?;
            Ok(Some(FrameAccess::Read {
                offset,
                result: op.results[0],
            }))
        }
        OpCode::ClosureStore
            if op.operands.len() == 2 && op.operands[0] == frame && op.operands[1] != frame =>
        {
            let offset = offset.ok_or(())?;
            Ok(Some(FrameAccess::Write {
                offset,
                value: op.operands[1],
                index,
            }))
        }
        // Frame bookkeeping the splice drops, and a check whose operands it
        // clears, keep no frame reference.
        OpCode::StateSwitch | OpCode::CheckException => Ok(None),
        _ if is_bookkeeping_op(op) => Ok(None),
        _ => Err(()),
    }
}

/// Plans the promotion of `poll`'s user frame slots, the first `arity` of which
/// hold the generator's arguments. `None` refuses the fusion.
pub(super) fn plan_slots(poll: &TirFunction, arity: usize) -> Option<SlotPlan> {
    let frame = poll.blocks.get(&poll.entry_block)?.args.first()?.id;
    let pred_map = build_pred_map_with(poll, CfgEdgePolicy::TerminatorOnly);
    let tree = compute_dominator_tree_with(poll, &pred_map, CfgEdgePolicy::TerminatorOnly);
    // The blocks the poll's own control flow enters, those any edge enters,
    // and those an exception edge enters.
    let body: HashSet<BlockId> = tree.blocks().iter().copied().collect();
    let executable = reachable_blocks_with(poll, CfgEdgePolicy::Full);
    let labels = exception_label_to_block(poll);
    let exception_targets: HashSet<BlockId> = executable
        .iter()
        .flat_map(|block| exception_successors(&poll.blocks[block], &labels))
        .collect();
    // The values the splice drops with their ops: frame bookkeeping results and
    // the exception-stack saves.
    let mut dropped = exception_stack_values(poll);
    dropped.extend(
        poll.blocks
            .values()
            .flat_map(|block| &block.ops)
            .filter(|op| is_bookkeeping_op(op))
            .flat_map(|op| op.results.iter().copied()),
    );

    // The frame owns its entire constructor payload. A callback can observe
    // an argument even if the poll never reads its slot (a 3.15 genexpr's .0).
    let payload_end = GEN_CONTROL_BYTES.checked_add(i64::try_from(arity).ok()?.checked_mul(8)?)?;
    let mut offsets: BTreeSet<_> = (GEN_CONTROL_BYTES..payload_end).step_by(8).collect();
    // Every user-slot access of the body, in op order.
    let mut accesses: HashMap<BlockId, Vec<FrameAccess>> = HashMap::default();
    let mut publications = HashMap::default();
    let mut has_observer = false;
    let exact = crate::tir::type_refine::extract_exact_scalar_map(poll);
    for (&bid, block) in &poll.blocks {
        let mut frame_used = false;
        block
            .terminator
            .for_each_value(|value| frame_used |= value == frame);
        if frame_used {
            return None;
        }
        let mut after_dropped_call = false;
        for (index, op) in block.ops.iter().enumerate() {
            let skipped = super::frame_states::skip_frame_op(op, &mut after_dropped_call);
            let access = frame_access(op, index, frame).ok()?;
            let Some(access) = access else {
                if body.contains(&bid) && !skipped {
                    let yielding = op.opcode == OpCode::StateYield;
                    let observes = !yielding
                        && (op.exception_edge_kind() == Some(ExceptionEdgeKind::Origin)
                            || op.runtime == Some(RuntimeFn::ExceptionTracePrepend)
                            || super::super::spliced_frame::may_observe_frames(op, &exact));
                    if observes || yielding {
                        has_observer |= observes;
                        publications.entry(bid).or_insert(index);
                    }
                }
                continue;
            };
            let (offset, read, stored) = match access {
                FrameAccess::Read { offset, .. } => (offset, true, None),
                FrameAccess::Write { offset, value, .. } => (offset, false, Some(value)),
            };
            if offset < GEN_CONTROL_BYTES {
                // A control slot: its reads see `None`, its stores go.
                continue;
            }
            if (offset - GEN_CONTROL_BYTES) % 8 != 0 {
                return None;
            }
            if body.contains(&bid) {
                if stored.is_some_and(|value| dropped.contains(&value)) {
                    return None;
                }
                offsets.insert(offset);
                accesses.entry(bid).or_default().push(access);
            } else if executable.contains(&bid) && read {
                return None;
            }
        }
        if body.contains(&bid)
            && matches!(block.terminator, Terminator::Return { .. })
            && block.ops.iter().any(|op| op.opcode == OpCode::StateSet)
        {
            publications.entry(bid).or_insert(block.ops.len());
        }
        if executable.contains(&bid) && !body.contains(&bid) {
            let mut enters_body = false;
            block
                .terminator
                .for_each_edge(|target, _| enters_body |= body.contains(&target));
            if enters_body {
                return None;
            }
        }
    }
    let slot_of: HashMap<i64, usize> = offsets
        .iter()
        .enumerate()
        .map(|(slot, &offset)| (offset, slot))
        .collect();
    let initial: Vec<Option<SlotDef>> = offsets
        .iter()
        .map(|&offset| {
            let position = usize::try_from((offset - GEN_CONTROL_BYTES) / 8).ok()?;
            (position < arity).then_some(SlotDef::Argument(position))
        })
        .collect();

    // Per slot, the blocks that store it and the blocks it is live into: a read
    // before any store in a block makes it live there, and liveness flows back
    // through every block that does not store the slot.
    let slots = offsets.len();
    let mut stores: Vec<HashSet<BlockId>> = vec![HashSet::default(); slots];
    let mut live: Vec<HashSet<BlockId>> = vec![HashSet::default(); slots];
    for &bid in &body {
        let mut first_store = vec![usize::MAX; slots];
        for access in accesses.get(&bid).into_iter().flatten() {
            match *access {
                FrameAccess::Read { offset, .. } => {
                    let slot = slot_of[&offset];
                    if first_store[slot] == usize::MAX {
                        live[slot].insert(bid);
                    }
                }
                FrameAccess::Write { offset, index, .. } => {
                    let slot = slot_of[&offset];
                    first_store[slot] = first_store[slot].min(index);
                    stores[slot].insert(bid);
                }
            }
        }
        // A publication reads every bound slot. Only its first occurrence
        // can read the block's incoming value before the block overwrites it.
        // Yield and completion only refresh a frame first exposed elsewhere.
        if has_observer && let Some(&publish) = publications.get(&bid) {
            for (slot, &written) in first_store.iter().enumerate() {
                if publish <= written {
                    live[slot].insert(bid);
                }
            }
        }
    }
    for (slot_live, slot_stores) in live.iter_mut().zip(&stores) {
        let mut work: Vec<BlockId> = slot_live.iter().copied().collect();
        while let Some(block) = work.pop() {
            for &pred in &pred_map[&block] {
                if body.contains(&pred) && !slot_stores.contains(&pred) && slot_live.insert(pred) {
                    work.push(pred);
                }
            }
        }
        if slot_live
            .iter()
            .any(|block| exception_targets.contains(block))
        {
            return None;
        }
    }

    // Each slot joins where its stores' iterated dominance frontier meets its
    // liveness. Slots are visited in order, so each join lists them ascending.
    let frontiers = compute_dominance_frontiers(&tree, &pred_map);
    let mut joins: BTreeMap<BlockId, Vec<usize>> = BTreeMap::new();
    for (slot, (slot_stores, slot_live)) in stores.iter().zip(&live).enumerate() {
        for block in iterated_dominance_frontier(slot_stores, &frontiers) {
            if slot_live.contains(&block) {
                joins.entry(block).or_default().push(slot);
            }
        }
    }

    // Rename in dominator-tree preorder: a block starts from its immediate
    // dominator's exit, or the frame's initial state at the entry, and its
    // joins shadow that.
    let children = build_dom_children(&tree);
    let mut exit_state: HashMap<BlockId, Vec<Option<SlotDef>>> = HashMap::default();
    let mut entry_state: HashMap<BlockId, Vec<Option<SlotDef>>> = HashMap::default();
    let mut reads: HashMap<ValueId, SlotDef> = HashMap::default();
    let mut arguments: BTreeSet<usize> = BTreeSet::new();
    let mut promoted_stores: BTreeSet<(BlockId, usize)> = BTreeSet::new();
    for block in dom_tree_preorder(poll.entry_block, &children) {
        let mut state = match tree.idom(block) {
            Some(parent) => exit_state[&parent].clone(),
            None => initial.clone(),
        };
        for &slot in joins.get(&block).into_iter().flatten() {
            state[slot] = Some(SlotDef::Join { block, slot });
        }
        entry_state.insert(block, state.clone());
        for access in accesses.get(&block).into_iter().flatten() {
            match *access {
                FrameAccess::Read { offset, result } => {
                    let def = state[slot_of[&offset]]?;
                    if let SlotDef::Argument(position) = def {
                        arguments.insert(position);
                    }
                    reads.insert(result, def);
                }
                FrameAccess::Write { offset, index, .. } => {
                    promoted_stores.insert((block, index));
                    state[slot_of[&offset]] = Some(SlotDef::Stored { block, index });
                }
            }
        }
        exit_state.insert(block, state);
    }

    // Propagate possible absence along the SSA edges between joins, including
    // loop backedges. A store or constructor argument supplies a bound value.
    let mut exits = HashMap::default();
    let mut optional_joins = BTreeSet::new();
    let mut users: HashMap<_, Vec<_>> = HashMap::default();
    for &block in &body {
        let state = &exit_state[&block];
        let mut feeds_join = false;
        poll.blocks[&block].terminator.for_each_edge(|target, _| {
            for &slot in joins.get(&target).into_iter().flatten() {
                feeds_join = true;
                let join = (target, slot);
                match state[slot] {
                    None => {
                        optional_joins.insert(join);
                    }
                    Some(SlotDef::Join { block, slot }) => {
                        users.entry((block, slot)).or_default().push(join);
                    }
                    Some(SlotDef::Argument(position)) => {
                        arguments.insert(position);
                    }
                    Some(SlotDef::Stored { .. }) => {}
                }
            }
        });
        if feeds_join {
            exits.insert(block, state.clone());
        }
    }
    let mut work: Vec<_> = optional_joins.iter().copied().collect();
    while let Some(join) = work.pop() {
        for &user in users.get(&join).into_iter().flatten() {
            if optional_joins.insert(user) {
                work.push(user);
            }
        }
    }
    // A snapshot can represent an unbound slot. A program load requires the
    // actual stored value, and must never read the unused absence payload.
    if reads.values().any(|def| {
        matches!(
            def, SlotDef::Join { block, slot } if optional_joins.contains(&(*block, *slot))
        )
    }) {
        return None;
    }

    Some(SlotPlan {
        frame,
        arguments,
        stores: promoted_stores,
        reads,
        joins,
        optional_joins,
        exits,
        offsets: offsets.iter().copied().collect(),
        entry: entry_state,
    })
}
