//! Frame-home placement: a home is written where a reader can run, not at
//! every rebinding (call_anatomy.md 4.7, item 2).
//!
//! A synchronous frame keeps each local in a home of its record
//! (`frame_home_store`). Compiled code reads its locals from SSA values; only
//! an observer reads a home: a traceback's frame, frame introspection or a
//! signal handler reached through Python code the body runs, the frame's exit,
//! and the body's own `frame_home_load`/`frame_home_take`. So a store no
//! reader can see before the slot's next store is dead on that path. This pass
//! is partial dead-store elimination (Knoop, Rüthing and Steffen 1994) in the
//! form HotSpot uses for safepoints: inside an innermost loop it deletes the
//! home stores of values that own no reference and writes each home again, from
//! the SSA value that holds its binding, only before a reader, at the
//! service path of the loop's asynchronous-work poll, and on every edge that
//! leaves the loop.
//!
//! * A **candidate** is a `frame_home_store` in an innermost loop whose operand
//!   is a carrier with no reference count (a raw int, raw bool, raw float or a
//!   boxed immediate; `Repr::carrier_refcount_is_noop`), whose result, if any,
//!   is not a boxed view of a raw int, and whose previous binding is proven
//!   finalizer-free (`frame_location_placement`, M2.1). Deleting it moves no
//!   reference and no finalizer.
//! * A **reader** is an operation at which an observer can run
//!   (`frame_elision::op_observer`: it may raise, may run Python, or reads the
//!   frame), a branch that tests truth through Python, a `Return`, a release of
//!   a binding that may finalize, and, for one slot, `frame_home_load` and
//!   `frame_home_take`. A non-poll `CheckException` is not a reader: the
//!   operation that raised was, and the home was written before it, so a
//!   traceback shows the binding at the raise, as CPython's does.
//! * The poll at a loop's back edge services signal handlers and pending
//!   calls. It becomes an `AsyncWorkPending` test whose cold block writes the
//!   dirty homes and then polls; the loop's normal path writes no home.
//! * A forward analysis tracks, per slot, whether its home is stale and which
//!   SSA value holds its binding. At a join the binding is the common value or
//!   the block parameter every edge passes it in. A slot whose binding is not
//!   known at a reader keeps its stores. Every path that reaches a reader
//!   passes the definition of the value it writes, so the value dominates it.
//!
//! On every path a loop iteration writes each home at most as often as before:
//! a reader cleans the slot, and only a deleted store dirties it again.
//!
//! The boundary is the PEP 667 boundary of M2.1: a callback that rebinds a
//! local through a frame proxy does not clear these facts, so a value injected
//! that way into a home this pass keeps stale is released at the next write of
//! that home, not at the compiled rebinding.
//!
//! The pass also records the nothrow proof on every `frame_home_store` that
//! boxes nothing: a home write raises only when it boxes a full-range raw int
//! into its home (HF-866). Check elimination then deletes the check after it.

use std::collections::{BTreeMap, BTreeSet};

use molt_ir::deterministic_hash::{HashMap, HashSet};
use molt_ir::{ExecutionContextPolicy, OpKind};

use crate::repr::Repr;
use crate::tir::analysis::{AnalysisManager, ImmediateDoms, LoopForest, LoopForestResult};
use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::dominators;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

use super::PassStats;
use super::frame_elision::{op_observer, terminator_observer};
use super::frame_location_placement::{binding_releases, block_order, home_slot};

/// Function marker: placement ran, so a rerun of the terminal phase leaves the
/// placed writes alone.
const PLACED_ATTR: &str = "frame_homes_placed";

type Representation = HashMap<ValueId, Repr>;

fn representation(func: &TirFunction) -> Representation {
    let ranges = crate::representation_facts::value_range_for(func);
    crate::representation_facts::repr_by_value_for(func, Some(&ranges))
}

fn repr_of(repr: &Representation, value: ValueId) -> Repr {
    repr.get(&value).copied().unwrap_or(Repr::DynBox)
}

fn is_home_store(op: &TirOp) -> bool {
    op.opcode == OpCode::Copy && op.origin == Some(OpKind::FrameHomeStore)
}

/// Whether the store publishes a boxed view of a full-range raw int: the box
/// may be a new heap int, allocated into the home. Both lowerers choose the
/// boxed view from the same representation facts.
fn boxes_heap_int(op: &TirOp, repr: &Representation) -> bool {
    op.operands
        .first()
        .is_some_and(|&value| repr_of(repr, value).is_raw_i64_full_deopt())
        && op
            .results
            .first()
            .is_some_and(|&result| !repr_of(repr, result).is_raw_i64_carrier())
}

/// Record the nothrow proof on every home store that boxes nothing.
fn prove_nothrow(func: &mut TirFunction, repr: &Representation) -> usize {
    let mut proven = 0;
    for block in func.blocks.values_mut() {
        for op in &mut block.ops {
            if is_home_store(op) && !op.has_nothrow_proof() && !boxes_heap_int(op, repr) {
                op.mark_nothrow_proof();
                proven += 1;
            }
        }
    }
    proven
}

/// Place `func`'s frame-home writes and record their nothrow proofs.
/// `split_polls`: the target lowers `AsyncWorkPending`, so a poll can move to
/// a cold block.
pub fn run(func: &mut TirFunction, am: &mut AnalysisManager, split_polls: bool) -> PassStats {
    let mut stats = PassStats {
        name: "frame_home_placement",
        ..Default::default()
    };
    if !func
        .blocks
        .values()
        .any(|block| block.ops.iter().any(is_home_store))
    {
        return stats;
    }
    let mut repr = representation(func);
    let placed = matches!(func.attrs.get(PLACED_ATTR), Some(AttrValue::Bool(true)));
    if !placed && eligible(func) {
        let forest = am.get::<LoopForest>(func).clone();
        let doms = am.get::<ImmediateDoms>(func).clone();
        if let Some(plan) = Planner::new(func, &forest, &doms, &repr, split_polls).plan() {
            let snapshot = func.clone();
            let mut applied = PassStats {
                name: "frame_home_placement",
                ..Default::default()
            };
            apply(func, &plan, &mut applied);
            let after = representation(func);
            // A written value must still own no reference under the facts of
            // the rewritten body; otherwise the write would adopt a reference
            // it was never given. Restore the body if the facts moved.
            if plan
                .written_values()
                .all(|value| repr_of(&after, value).carrier_refcount_is_noop())
            {
                func.attrs.insert(PLACED_ATTR.into(), AttrValue::Bool(true));
                applied.facts_changed += 1;
                stats = applied;
                repr = after;
            } else {
                *func = snapshot;
            }
        }
    }
    stats.attrs_changed += prove_nothrow(func, &repr);
    stats
}

/// A synchronous body that links its own record. A stateful body keeps its
/// bindings in its task, and a codegen partition writes its parent's homes.
fn eligible(func: &TirFunction) -> bool {
    func.execution_context == ExecutionContextPolicy::Local
        && !func.has_state_machine()
        && !func.is_codegen_partition()
}

// ---------------------------------------------------------------------------
// Analysis
// ---------------------------------------------------------------------------

/// The SSA value that holds a slot's binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Known {
    /// No path reached here yet.
    Top,
    Value(ValueId),
    /// Paths disagree, or the binding is not an SSA value this body holds.
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct State {
    /// The home is stale: a deleted store wrote its binding after the last
    /// write the home saw.
    dirty: Vec<bool>,
    known: Vec<Known>,
}

impl State {
    fn clean(slots: usize) -> Self {
        Self {
            dirty: vec![false; slots],
            known: vec![Known::Unknown; slots],
        }
    }
}

/// Where a home is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Point {
    /// Before op `index` of the block; `ops.len()` is before the terminator.
    BeforeOp(BlockId, usize),
    /// In the cold block of the poll at op `index`.
    PollCold(BlockId, usize),
    /// On normal edge `index` of the block's terminator.
    Edge(BlockId, usize),
}

/// One incoming transfer into a block.
enum Incoming {
    Edge {
        pred: BlockId,
        index: usize,
        args: Vec<ValueId>,
    },
    /// An exception edge: the raising operation wrote every stale home, and
    /// the binding is not a value this path holds.
    Exception,
    /// The function's entry.
    Entry,
}

struct Plan {
    candidates: BTreeSet<(BlockId, usize)>,
    writes: BTreeMap<Point, Vec<(usize, ValueId)>>,
}

impl Plan {
    fn written_values(&self) -> impl Iterator<Item = ValueId> + '_ {
        self.writes.values().flatten().map(|&(_, value)| value)
    }
}

/// The writes a transfer needs, or the slots it cannot place.
#[derive(Default)]
struct Events {
    writes: BTreeMap<Point, Vec<(usize, ValueId)>>,
    failed: BTreeSet<usize>,
}

struct Planner<'a> {
    func: &'a TirFunction,
    repr: &'a Representation,
    split_polls: bool,
    exact: HashMap<ValueId, TirType>,
    /// Home store or clear -> whether the binding it releases is
    /// finalizer-free.
    releases: HashMap<(BlockId, usize), bool>,
    canon: HashMap<ValueId, ValueId>,
    /// Block -> the header of the innermost loop that holds it.
    region: HashMap<BlockId, BlockId>,
    /// Region blocks every iteration runs: they dominate each latch. A home
    /// written there is written as often as the store it replaces, so the
    /// slot keeps its stores.
    every_iteration: HashSet<BlockId>,
    slots: usize,
    excluded_slots: HashSet<usize>,
    order: Vec<BlockId>,
    incoming: HashMap<BlockId, Vec<Incoming>>,
}

impl<'a> Planner<'a> {
    fn new(
        func: &'a TirFunction,
        forest: &LoopForestResult,
        doms: &dominators::DominatorTree,
        repr: &'a Representation,
        split_polls: bool,
    ) -> Self {
        let exact = crate::tir::type_refine::extract_exact_scalar_map(func);
        let labels = dominators::exception_label_to_block(func);
        let all_blocks = block_order(func);
        let releases = binding_releases(func, &exact, &labels, &all_blocks);
        let reachable = dominators::executable_reachable_blocks(func);
        let order: Vec<BlockId> = all_blocks
            .into_iter()
            .filter(|bid| reachable.contains(bid))
            .collect();

        let mut canon = HashMap::default();
        let mut slots = 0;
        let mut excluded_slots = HashSet::default();
        for bid in &order {
            for op in &func.blocks[bid].ops {
                if let Some(source) = value_source(op) {
                    canon.insert(op.results[0], source);
                }
                if op.opcode == OpCode::Copy
                    && let Some(kind) = op.origin
                    && is_home_kind(kind)
                    && let Some(slot) = home_slot(op)
                {
                    slots = slots.max(slot + 1);
                    if matches!(kind, OpKind::FrameHomeCell | OpKind::FrameHomePrivateCell) {
                        excluded_slots.insert(slot);
                    }
                }
            }
        }

        let mut region = HashMap::default();
        let mut shared = HashSet::default();
        for header in &forest.headers {
            let body = &forest.bodies[header];
            let innermost = !forest
                .headers
                .iter()
                .any(|other| other != header && body.contains(other));
            if !innermost {
                continue;
            }
            for &bid in body {
                if region.insert(bid, *header).is_some() {
                    shared.insert(bid);
                }
            }
        }
        if !shared.is_empty() {
            // Two innermost loops claim one block: keep neither.
            let headers: HashSet<BlockId> = shared.iter().map(|bid| region[bid]).collect();
            region.retain(|_, header| !headers.contains(header));
        }

        let mut every_iteration = HashSet::default();
        for (&bid, header) in &region {
            let body = &forest.bodies[header];
            let mut latches = order.iter().filter(|pred| {
                body.contains(pred) && func.blocks[pred].terminator.successors().contains(header)
            });
            if latches.all(|latch| doms.dominates(bid, *latch)) {
                every_iteration.insert(bid);
            }
        }

        let mut incoming: HashMap<BlockId, Vec<Incoming>> = HashMap::default();
        incoming
            .entry(func.entry_block)
            .or_default()
            .push(Incoming::Entry);
        for bid in &order {
            let block = &func.blocks[bid];
            for op in &block.ops {
                if dominators::is_exception_transfer_edge(op.opcode)
                    && let Some(AttrValue::Int(label)) = op.attrs.get("value")
                    && let Some(&target) = labels.get(label)
                {
                    incoming
                        .entry(target)
                        .or_default()
                        .push(Incoming::Exception);
                }
            }
            let mut index = 0;
            block.terminator.for_each_edge(|target, args| {
                incoming.entry(target).or_default().push(Incoming::Edge {
                    pred: *bid,
                    index,
                    args: args.to_vec(),
                });
                index += 1;
            });
        }

        Self {
            func,
            repr,
            split_polls,
            exact,
            releases,
            canon,
            region,
            every_iteration,
            slots,
            excluded_slots,
            order,
            incoming,
        }
    }

    fn canon(&self, mut value: ValueId) -> ValueId {
        for _ in 0..64 {
            match self.canon.get(&value) {
                Some(&next) if next != value => value = next,
                _ => break,
            }
        }
        value
    }

    fn initial_candidates(&self) -> BTreeSet<(BlockId, usize)> {
        let mut candidates = BTreeSet::new();
        for bid in &self.order {
            if !self.region.contains_key(bid) {
                continue;
            }
            for (index, op) in self.func.blocks[bid].ops.iter().enumerate() {
                if self.is_candidate(*bid, index, op) {
                    candidates.insert((*bid, index));
                }
            }
        }
        candidates
    }

    fn is_candidate(&self, bid: BlockId, index: usize, op: &TirOp) -> bool {
        if !is_home_store(op) || op.operands.len() != 1 || op.results.len() > 1 {
            return false;
        }
        let Some(slot) = home_slot(op).filter(|slot| *slot < self.slots) else {
            return false;
        };
        if self.excluded_slots.contains(&slot) {
            return false;
        }
        let operand = repr_of(self.repr, op.operands[0]);
        if !operand.carrier_refcount_is_noop() {
            return false;
        }
        if operand.is_raw_i64_carrier()
            && op
                .results
                .first()
                .is_some_and(|&result| !repr_of(self.repr, result).is_raw_i64_carrier())
        {
            return false;
        }
        self.releases.get(&(bid, index)) == Some(&true)
    }

    fn plan(&self) -> Option<Plan> {
        let mut candidates = self.initial_candidates();
        while !candidates.is_empty() {
            let entry = self.solve(&candidates)?;
            let mut events = Events::default();
            for bid in &self.order {
                if let Some(state) = entry.get(bid) {
                    self.transfer(*bid, state.clone(), &candidates, Some(&mut events));
                }
            }
            if events.failed.is_empty() {
                return Some(Plan {
                    candidates,
                    writes: events.writes,
                });
            }
            let ops = &self.func.blocks;
            candidates.retain(|(bid, index)| {
                home_slot(&ops[bid].ops[*index]).is_none_or(|slot| !events.failed.contains(&slot))
            });
        }
        None
    }

    /// Entry states of every reachable block, or `None` when the analysis does
    /// not settle.
    fn solve(&self, candidates: &BTreeSet<(BlockId, usize)>) -> Option<HashMap<BlockId, State>> {
        let mut entry: HashMap<BlockId, State> = HashMap::default();
        let mut out: HashMap<(BlockId, usize), State> = HashMap::default();
        let limit = 4 * self.order.len() + 16;
        for _ in 0..limit {
            let mut changed = false;
            for bid in &self.order {
                let Some(state) = self.meet(*bid, &out) else {
                    continue;
                };
                if entry.get(bid) != Some(&state) {
                    entry.insert(*bid, state.clone());
                    changed = true;
                }
                for (index, edge) in self.transfer(*bid, state, candidates, None) {
                    out.insert((*bid, index), edge);
                }
            }
            if !changed {
                return Some(entry);
            }
        }
        None
    }

    fn meet(&self, bid: BlockId, out: &HashMap<(BlockId, usize), State>) -> Option<State> {
        let mut states: Vec<(Option<&[ValueId]>, State)> = Vec::new();
        for incoming in self.incoming.get(&bid).into_iter().flatten() {
            match incoming {
                Incoming::Edge { pred, index, args } => {
                    if let Some(state) = out.get(&(*pred, *index)) {
                        states.push((Some(args.as_slice()), state.clone()));
                    }
                }
                Incoming::Exception | Incoming::Entry => {
                    states.push((None, State::clean(self.slots)));
                }
            }
        }
        if states.is_empty() {
            return None;
        }
        let params = &self.func.blocks[&bid].args;
        let mut merged = State::clean(self.slots);
        for slot in 0..self.slots {
            merged.dirty[slot] = states.iter().any(|(_, state)| state.dirty[slot]);
            let mut values: Vec<(Option<&[ValueId]>, ValueId)> = Vec::new();
            let mut unknown = false;
            for (args, state) in &states {
                match state.known[slot] {
                    Known::Top => {}
                    Known::Unknown => unknown = true,
                    Known::Value(value) => values.push((*args, value)),
                }
            }
            merged.known[slot] = if unknown {
                Known::Unknown
            } else if values.is_empty() {
                Known::Top
            } else if values.iter().all(|(_, value)| *value == values[0].1) {
                Known::Value(values[0].1)
            } else {
                (0..params.len())
                    .find(|&param| {
                        values.iter().all(|(args, value)| {
                            args.and_then(|args| args.get(param))
                                .is_some_and(|&arg| self.canon(arg) == *value)
                        })
                    })
                    .map_or(Known::Unknown, |param| Known::Value(params[param].id))
            };
        }
        Some(merged)
    }

    /// Run block `bid` from `state`; return its normal edges' states and, with
    /// `events`, record the writes it needs.
    fn transfer(
        &self,
        bid: BlockId,
        mut state: State,
        candidates: &BTreeSet<(BlockId, usize)>,
        mut events: Option<&mut Events>,
    ) -> Vec<(usize, State)> {
        let block = &self.func.blocks[&bid];
        for (index, op) in block.ops.iter().enumerate() {
            let at = Point::BeforeOp(bid, index);
            if op.opcode == OpCode::Copy
                && let Some(kind) = op.origin
                && (is_home_kind(kind) || kind == OpKind::FrameLocalsSet)
            {
                let slot = home_slot(op).filter(|slot| *slot < self.slots);
                match (kind, slot) {
                    (OpKind::FrameLocalsSet, _) | (_, None) => {
                        self.write_all(&mut state, at, &mut events);
                        state = State::clean(self.slots);
                    }
                    (OpKind::FrameHomeStore, Some(slot)) => {
                        let value = Known::Value(self.canon(op.operands[0]));
                        if candidates.contains(&(bid, index)) {
                            state.dirty[slot] = true;
                        } else {
                            if self.releases.get(&(bid, index)) != Some(&true) {
                                self.write_all(&mut state, at, &mut events);
                            }
                            state.dirty[slot] = false;
                        }
                        state.known[slot] = value;
                    }
                    (OpKind::FrameHomeClear, Some(slot)) => {
                        if self.releases.get(&(bid, index)) != Some(&true) {
                            self.write_all(&mut state, at, &mut events);
                        }
                        state.dirty[slot] = false;
                        state.known[slot] = Known::Unknown;
                    }
                    (OpKind::FrameHomeLoad | OpKind::FrameHomeTake, Some(slot)) => {
                        self.write(&mut state, slot, at, &mut events);
                        state.known[slot] = Known::Unknown;
                    }
                    (_, Some(slot)) => {
                        // A cell store: its slot is never a candidate's.
                        state.dirty[slot] = false;
                        state.known[slot] = Known::Unknown;
                    }
                }
                continue;
            }
            if op.opcode == OpCode::CheckException && op.is_async_work_poll() {
                if self.split_polls && self.splittable(bid) {
                    // The cold block writes; the normal path stays stale.
                    let mut cold = state.clone();
                    self.write_all(&mut cold, Point::PollCold(bid, index), &mut events);
                } else {
                    self.write_all(&mut state, at, &mut events);
                }
                continue;
            }
            // A `line` publication writes the location word, not a home.
            if op.origin != Some(OpKind::Line) && op_observer(op, &self.exact).is_some() {
                self.write_all(&mut state, at, &mut events);
            }
        }
        if matches!(block.terminator, Terminator::Return { .. })
            || terminator_observer(&block.terminator, &self.exact).is_some()
        {
            self.write_all(
                &mut state,
                Point::BeforeOp(bid, block.ops.len()),
                &mut events,
            );
        }
        let home_region = self.region.get(&bid);
        let mut edges = Vec::new();
        let mut index = 0;
        block.terminator.for_each_edge(|target, _| {
            let mut edge = state.clone();
            if home_region.is_some() && self.region.get(&target) != home_region {
                self.write_all(&mut edge, Point::Edge(bid, index), &mut events);
            }
            edges.push((index, edge));
            index += 1;
        });
        edges
    }

    /// Whether a poll's block may split: the structured-loop metadata the
    /// SimpleIR back-conversion reads names no such block.
    fn splittable(&self, bid: BlockId) -> bool {
        let func = self.func;
        func.loop_roles
            .get(&bid)
            .is_none_or(|role| *role == crate::tir::blocks::LoopRole::None)
            && !func.loop_pairs.values().any(|end| *end == bid)
            && !func.loop_cond_blocks.values().any(|cond| *cond == bid)
    }

    fn write_all(&self, state: &mut State, at: Point, events: &mut Option<&mut Events>) {
        for slot in 0..self.slots {
            self.write(state, slot, at, events);
        }
    }

    /// Write `slot`'s home at `at` if it is stale.
    fn write(&self, state: &mut State, slot: usize, at: Point, events: &mut Option<&mut Events>) {
        if !state.dirty[slot] {
            return;
        }
        state.dirty[slot] = false;
        let Some(events) = events.as_deref_mut() else {
            return;
        };
        if let Point::BeforeOp(bid, _) = at
            && self.every_iteration.contains(&bid)
        {
            events.failed.insert(slot);
            return;
        }
        match state.known[slot] {
            Known::Value(value) if repr_of(self.repr, value).carrier_refcount_is_noop() => {
                events.writes.entry(at).or_default().push((slot, value));
            }
            _ => {
                events.failed.insert(slot);
            }
        }
    }
}

fn is_home_kind(kind: OpKind) -> bool {
    matches!(
        kind,
        OpKind::FrameHomeStore
            | OpKind::FrameHomeClear
            | OpKind::FrameHomeTake
            | OpKind::FrameHomeLoad
            | OpKind::FrameHomeCell
            | OpKind::FrameHomePrivateCell
    )
}

/// The value a Copy forwards unchanged: a value-identity copy, a binding
/// alias (which only retains), or a home store's view of its operand.
fn value_source(op: &TirOp) -> Option<ValueId> {
    if let Some(source) = super::value_identity::copy_value_source(op) {
        return Some(source);
    }
    (op.opcode == OpCode::Copy
        && op.operands.len() == 1
        && op.results.len() == 1
        && matches!(
            op.origin,
            Some(OpKind::BindingAlias | OpKind::FrameHomeStore)
        ))
    .then(|| op.operands[0])
}

// ---------------------------------------------------------------------------
// Rewrite
// ---------------------------------------------------------------------------

fn home_write(slot: usize, value: ValueId) -> TirOp {
    let mut op = TirOp::new(OpCode::Copy, vec![value], vec![]);
    op.origin = Some(OpKind::FrameHomeStore);
    op.attrs.insert("value".into(), AttrValue::Int(slot as i64));
    op.mark_nothrow_proof();
    op
}

fn home_writes(writes: &[(usize, ValueId)]) -> impl Iterator<Item = TirOp> + '_ {
    writes.iter().map(|&(slot, value)| home_write(slot, value))
}

fn apply(func: &mut TirFunction, plan: &Plan, stats: &mut PassStats) {
    // Normal edges into each block, to place an edge's writes in its target
    // when the edge is the target's only way in.
    let labels = dominators::exception_label_to_block(func);
    let mut ways_in: HashMap<BlockId, usize> = HashMap::default();
    for block in func.blocks.values() {
        block
            .terminator
            .for_each_edge(|target, _| *ways_in.entry(target).or_default() += 1);
        for op in &block.ops {
            // An exception target is never an edge's private block.
            if dominators::is_exception_transfer_edge(op.opcode)
                && let Some(AttrValue::Int(label)) = op.attrs.get("value")
                && let Some(&target) = labels.get(label)
            {
                *ways_in.entry(target).or_default() += 2;
            }
        }
    }

    let mut before: BTreeMap<(BlockId, usize), Vec<TirOp>> = BTreeMap::new();
    let mut polls: BTreeMap<(BlockId, usize), Vec<TirOp>> = BTreeMap::new();
    let mut edges: Vec<(BlockId, usize, Vec<TirOp>)> = Vec::new();
    for (point, writes) in &plan.writes {
        let ops: Vec<TirOp> = home_writes(writes).collect();
        stats.ops_added += ops.len();
        match *point {
            Point::BeforeOp(bid, index) => before.entry((bid, index)).or_default().extend(ops),
            Point::PollCold(bid, index) => polls.entry((bid, index)).or_default().extend(ops),
            Point::Edge(bid, index) => edges.push((bid, index, ops)),
        }
    }

    for (pred, index, ops) in edges {
        let mut target = None;
        let mut seen = 0;
        func.blocks[&pred].terminator.for_each_edge(|to, _| {
            if seen == index {
                target = Some(to);
            }
            seen += 1;
        });
        let target = target.expect("a planned edge exists");
        if target != func.entry_block && ways_in.get(&target) == Some(&1) {
            let at = before.entry((target, 0)).or_default();
            let mut placed = ops;
            placed.append(at);
            *at = placed;
            continue;
        }
        let split = func.fresh_block();
        let mut seen = 0;
        let mut args = Vec::new();
        func.blocks
            .get_mut(&pred)
            .expect("planned block")
            .terminator
            .for_each_edge_mut(|to, edge_args| {
                if seen == index {
                    *to = split;
                    args = std::mem::take(edge_args);
                }
                seen += 1;
            });
        func.blocks.insert(
            split,
            TirBlock {
                id: split,
                args: vec![],
                ops,
                terminator: Terminator::Branch { target, args },
            },
        );
    }

    let mut poll_positions: Vec<(BlockId, usize, Vec<TirOp>)> = Vec::new();
    let block_ids: Vec<BlockId> = func.blocks.keys().copied().collect();
    for bid in block_ids {
        let block = func.blocks.get_mut(&bid).expect("listed block");
        let len = block.ops.len();
        let has_work = plan.candidates.iter().any(|(b, _)| *b == bid)
            || before.range((bid, 0)..=(bid, len)).next().is_some()
            || polls.range((bid, 0)..=(bid, len)).next().is_some();
        if !has_work {
            continue;
        }
        let old = std::mem::take(&mut block.ops);
        let mut ops = Vec::with_capacity(old.len());
        for (index, op) in old.into_iter().enumerate() {
            if let Some(writes) = before.remove(&(bid, index)) {
                ops.extend(writes);
            }
            if plan.candidates.contains(&(bid, index)) {
                if op.results.is_empty() {
                    stats.ops_removed += 1;
                } else {
                    // The binding stays in its SSA value; the result names it.
                    ops.push(TirOp {
                        source_span: op.source_span,
                        ..TirOp::new(OpCode::Copy, op.operands, op.results)
                    });
                    stats.values_changed += 1;
                }
                continue;
            }
            if let Some(writes) = polls.remove(&(bid, index)) {
                poll_positions.push((bid, ops.len(), writes));
            }
            ops.push(op);
        }
        if let Some(writes) = before.remove(&(bid, len)) {
            ops.extend(writes);
        }
        block.ops = ops;
    }

    // Split each planned poll, last first within a block.
    poll_positions.sort_by_key(|(bid, at, _)| (*bid, std::cmp::Reverse(*at)));
    for (bid, at, writes) in poll_positions {
        split_poll(func, bid, at, writes);
        stats.ops_added += 1;
    }
}

/// Turn the poll at `at` into a pending-work test whose cold block writes the
/// stale homes and polls:
///
/// ```text
/// bid:   ...                         ; ops before the poll
///        %p = AsyncWorkPending
///        cond_br %p, ^cold, ^rest
/// cold:  frame_home_store ...        ; the stale homes
///        check_exception (poll)      ; services the work, as before
///        br ^rest
/// rest:  ...                         ; ops after the poll; bid's terminator
/// ```
fn split_poll(func: &mut TirFunction, bid: BlockId, at: usize, writes: Vec<TirOp>) {
    let pending = func.fresh_value();
    func.value_types.insert(pending, TirType::Bool);
    let cold = func.fresh_block();
    let rest = func.fresh_block();
    let block = func.blocks.get_mut(&bid).expect("poll block");
    let tail = block.ops.split_off(at + 1);
    let poll = block.ops.pop().expect("the poll");
    debug_assert!(poll.is_async_work_poll());
    block
        .ops
        .push(TirOp::new(OpCode::AsyncWorkPending, vec![], vec![pending]));
    let terminator = std::mem::replace(
        &mut block.terminator,
        Terminator::CondBranch {
            cond: pending,
            then_block: cold,
            then_args: vec![],
            else_block: rest,
            else_args: vec![],
        },
    );
    let mut cold_ops = writes;
    cold_ops.push(poll);
    func.blocks.insert(
        cold,
        TirBlock {
            id: cold,
            args: vec![],
            ops: cold_ops,
            terminator: Terminator::Branch {
                target: rest,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        rest,
        TirBlock {
            id: rest,
            args: vec![],
            ops: tail,
            terminator,
        },
    );
}

#[cfg(test)]
#[path = "frame_home_placement_tests.rs"]
mod tests;
