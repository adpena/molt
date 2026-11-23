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
//! * At an innermost loop's header, edges from outside may arrive with the
//!   home current and no SSA value for its binding (`for i in range(n)`: `i`
//!   has none before the loop), while the latch arrives with it stale. The
//!   header then carries the binding in an added parameter, with a flag that
//!   only the latch sets, as LLVM's LICM promotes a location to a header phi.
//!   An edge leaving the loop writes a carried binding under that flag, so a
//!   loop that ran no iteration leaves the home as it found it. A reader
//!   inside the loop on a path not every iteration takes (a generic slow
//!   path) writes a carried binding the same way, in a block split before it.
//!   The hot path pays two block arguments; a reader every iteration runs
//!   still fails its slot.
//!
//! * The frame record's location word is placed the same way: a
//!   `frame_set_location` in an innermost loop is a constant write, and its
//!   readers are location placement's (`operation_reads`, `check_uses`, a
//!   return), so a publication only a cold path reads moves there. Location
//!   placement then attaches `frame_location` to Origin checks and deletes the
//!   publications no reader sees.
//!
//! On every path a loop iteration writes each word at most as often as before:
//! a reader cleans the word, and only a deleted write dirties it again. A word
//! that a reader in a block every iteration runs would need keeps its writes.
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
use super::frame_location_placement::{
    binding_releases, block_order, check_uses, home_slot, operation_reads,
};

/// Function marker: placement ran, so a rerun of the terminal phase leaves the
/// placed writes alone.
const PLACED_ATTR: &str = "frame_homes_placed";

type Representation = HashMap<ValueId, Repr>;

fn representation(func: &TirFunction) -> Representation {
    crate::representation_facts::analyze_representations(func)
}

fn repr_of(repr: &Representation, value: ValueId) -> Repr {
    repr.get(&value).copied().unwrap_or(Repr::DynBox)
}

pub(crate) fn is_home_store(op: &TirOp) -> bool {
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

/// Record the nothrow proof on every home store that boxes nothing. It runs
/// before check elimination, so the checks after those stores are gone when
/// placement runs: a check is a reader of the location word.
pub fn prove_home_writes_nothrow(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "frame_home_nothrow",
        ..Default::default()
    };
    if func.blocks.values().any(|block| {
        block
            .ops
            .iter()
            .any(|op| is_home_store(op) && !op.has_nothrow_proof())
    }) {
        stats.attrs_changed += prove_nothrow(func, &representation(func));
    }
    stats
}

/// Place `func`'s frame-record writes (its homes and its location word).
/// `split_polls`: the target lowers `AsyncWorkPending`, so a poll can move to
/// a cold block.
pub fn run(func: &mut TirFunction, am: &mut AnalysisManager, split_polls: bool) -> PassStats {
    let mut stats = PassStats {
        name: "frame_home_placement",
        ..Default::default()
    };
    let placed = matches!(func.attrs.get(PLACED_ATTR), Some(AttrValue::Bool(true)));
    if placed || !eligible(func) {
        return stats;
    }
    let repr = representation(func);
    let forest = am.get::<LoopForest>(func).clone();
    let doms = am.get::<ImmediateDoms>(func).clone();
    let Some(plan) = Planner::new(func, &forest, &doms, &repr, split_polls).plan() else {
        return stats;
    };
    let snapshot = func.clone();
    let carried = apply(func, &plan, &mut stats);
    let after = representation(func);
    // A written value must still own no reference under the facts of the
    // rewritten body, or the write would adopt a reference it was never
    // given; and a store proven nothrow must still box nothing, since its
    // check is gone. Restore the body if the facts moved.
    let moved = !plan
        .written_values()
        .chain(carried)
        .all(|value| repr_of(&after, value).carrier_refcount_is_noop())
        || func.blocks.values().any(|block| {
            block
                .ops
                .iter()
                .any(|op| is_home_store(op) && op.has_nothrow_proof() && boxes_heap_int(op, &after))
        });
    if moved {
        *func = snapshot;
        return PassStats {
            name: "frame_home_placement",
            ..Default::default()
        };
    }
    func.attrs.insert(PLACED_ATTR.into(), AttrValue::Bool(true));
    stats.facts_changed += 1;
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

/// How often one word of one block's entry state may change before the solve
/// pins it to `Unknown`. A settling body changes a word a few times (a header
/// first sees no latch, then its outside binding, its parameter, the binding
/// it carries); a pinned word only costs a placement, never correctness.
const WORD_CHANGES: u8 = 6;

/// Bound how often a block's entry state may change, so the solve settles
/// whatever the transfers do. A carried binding a reader inside the loop writes
/// makes the latch arrive clean, which un-carries the header, which drops the
/// write: a cycle that ran `py.app.main` (1,612 ops) through all
/// `4 * blocks + 16` sweeps, 1.05 s per build, and then placed nothing
/// (HF-908). A body whose words change fewer times keeps its exact result.
fn bound_changes(
    block: BlockId,
    previous: &State,
    state: &mut State,
    changes: &mut HashMap<(BlockId, usize), u8>,
    trace: bool,
) {
    for slot in 0..state.known.len() {
        let count = changes.entry((block, slot)).or_insert(0);
        if *count > WORD_CHANGES {
            state.known[slot] = Known::Unknown;
            state.dirty[slot] |= previous.dirty[slot];
            continue;
        }
        if previous.known[slot] != state.known[slot] || previous.dirty[slot] != state.dirty[slot] {
            *count += 1;
            if *count > WORD_CHANGES {
                state.known[slot] = Known::Unknown;
                state.dirty[slot] |= previous.dirty[slot];
                if trace {
                    eprintln!("[TIR-HOMES] word {slot} at {block}: change bound reached");
                }
            }
        }
    }
}

/// What a frame-record word holds: a home's binding, held by an SSA value,
/// or the location word's LocationId.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Bound {
    Value(ValueId),
    Location(i64),
    /// The binding an innermost loop's header carries in a parameter the
    /// rewrite adds (LLVM LICM's promotion of a location to a header phi).
    /// Every edge from inside the loop arrives with the home stale and passes
    /// its binding; every edge from outside arrives with the home current.
    /// So a write of this binding runs only when the header's added flag says
    /// the loop body ran: `for i in range(n)` binds `i` in no block the exit
    /// edge leaves from, and an empty range leaves `i` unbound.
    Carried(BlockId),
}

/// The binding a word holds on every path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Known {
    /// No path reached here yet.
    Top,
    Value(Bound),
    /// Paths disagree, or the binding is not an SSA value this body holds.
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct State {
    /// Per word (each home, then the location word): the word is stale, a
    /// deleted write set its binding after the last write the word saw.
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
    writes: BTreeMap<Point, Vec<(usize, Bound)>>,
    /// Header -> the bindings it carries (`Bound::Carried`).
    carries: BTreeMap<BlockId, Carry>,
}

impl Plan {
    fn written_values(&self) -> impl Iterator<Item = ValueId> + '_ {
        self.writes
            .values()
            .flatten()
            .filter_map(|&(_, bound)| match bound {
                Bound::Value(value) => Some(value),
                Bound::Location(_) | Bound::Carried(_) => None,
            })
    }
}

/// The parameters a header gains: one flag, true on an edge from inside the
/// loop, and one value per carried slot.
struct Carry {
    /// Slot -> the type of the binding it carries.
    slots: BTreeMap<usize, TirType>,
    edges: Vec<CarryEdge>,
}

/// One normal edge into a carrying header.
struct CarryEdge {
    pred: BlockId,
    index: usize,
    inside: bool,
    /// Per carried slot, the binding an edge from inside passes. An edge from
    /// outside passes a zero of the slot's type that no write reads.
    values: BTreeMap<usize, CarryArg>,
}

enum CarryArg {
    Value(ValueId),
    /// The latch did not rebind the slot: it passes the header's parameter.
    Passthrough,
}

/// Types a carried binding may have: a raw scalar with a zero constant.
fn carriable(ty: &TirType) -> bool {
    matches!(ty, TirType::I64 | TirType::F64 | TirType::Bool)
}

/// Where a transfer into a block comes from, relative to the innermost loop
/// the block heads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    /// A normal edge from a block of the loop: a latch.
    Inside,
    /// A normal edge from a block outside the loop.
    Outside,
    /// The function's entry or an exception edge: no block arguments.
    Foreign,
}

/// One transfer's state at a block's entry.
struct Arrival<'a> {
    args: Option<&'a [ValueId]>,
    side: Side,
    state: State,
}

/// The writes a transfer needs, or the slots it cannot place.
#[derive(Default)]
struct Events {
    writes: BTreeMap<Point, Vec<(usize, Bound)>>,
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
    /// `MOLT_TIR_TRACE_FUNC` names this function: say why a word stays eager.
    trace: bool,
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
            trace: crate::tir::pass_manager::trace_func_enabled(&func.name),
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

    /// The word an op writes: its home slot, or the location word for a
    /// publication.
    fn word_of(&self, op: &TirOp) -> Option<usize> {
        if op.opcode == OpCode::FrameSetLocation {
            return location_of(op).map(|_| self.slots);
        }
        home_slot(op).filter(|slot| *slot < self.slots)
    }

    fn words(&self) -> usize {
        self.slots + 1
    }

    fn is_candidate(&self, bid: BlockId, index: usize, op: &TirOp) -> bool {
        if op.opcode == OpCode::FrameSetLocation {
            return location_of(op).is_some();
        }
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
            let Some(entry) = self.solve(&candidates) else {
                if self.trace {
                    eprintln!(
                        "[TIR-HOMES] {}: entry states did not settle",
                        self.func.name
                    );
                }
                return None;
            };
            let mut events = Events::default();
            for bid in &self.order {
                if let Some(state) = entry.get(bid) {
                    self.transfer(*bid, state.clone(), &candidates, Some(&mut events));
                }
            }
            if self.trace {
                eprintln!(
                    "[TIR-HOMES] {}: {} candidates, failed words {:?}",
                    self.func.name,
                    candidates.len(),
                    events.failed
                );
            }
            if events.failed.is_empty() {
                let carries = self.carries(&entry, &candidates, &events.writes);
                if self.trace && carries.is_none() {
                    eprintln!("[TIR-HOMES] {}: carried parameters refused", self.func.name);
                }
                let carries = carries?;
                return Some(Plan {
                    candidates,
                    writes: events.writes,
                    carries,
                });
            }
            let ops = &self.func.blocks;
            candidates.retain(|(bid, index)| {
                self.word_of(&ops[bid].ops[*index])
                    .is_none_or(|word| !events.failed.contains(&word))
            });
        }
        None
    }

    /// The parameters each header whose carried binding a write names gains,
    /// read from the settled edge states.
    fn carries(
        &self,
        entry: &HashMap<BlockId, State>,
        candidates: &BTreeSet<(BlockId, usize)>,
        writes: &BTreeMap<Point, Vec<(usize, Bound)>>,
    ) -> Option<BTreeMap<BlockId, Carry>> {
        let mut wanted: BTreeMap<BlockId, BTreeSet<usize>> = BTreeMap::new();
        for &(slot, bound) in writes.values().flatten() {
            if let Bound::Carried(header) = bound {
                wanted.entry(header).or_default().insert(slot);
            }
        }
        let mut carries = BTreeMap::new();
        for (header, slots) in wanted {
            if self
                .incoming
                .get(&header)
                .into_iter()
                .flatten()
                .any(|incoming| !matches!(incoming, Incoming::Edge { .. }))
            {
                return None;
            }
            // Every structural edge gains the arguments, including one from a
            // block the analysis found unreachable.
            let mut into_header = Vec::new();
            let mut preds: Vec<BlockId> = self.func.blocks.keys().copied().collect();
            preds.sort_unstable();
            for pred in preds {
                let mut index = 0;
                self.func.blocks[&pred].terminator.for_each_edge(|to, _| {
                    if to == header {
                        into_header.push((pred, index));
                    }
                    index += 1;
                });
            }
            let mut types: BTreeMap<usize, TirType> = BTreeMap::new();
            let mut edges = Vec::new();
            for (pred, index) in &into_header {
                let inside = self.region.get(pred) == Some(&header);
                let mut values = BTreeMap::new();
                // A predecessor the analysis never reached passes zeros: no
                // execution takes its edge.
                let state = entry.get(pred).filter(|_| inside).and_then(|state| {
                    self.transfer(*pred, state.clone(), candidates, None)
                        .into_iter()
                        .find(|(edge, _)| edge == index)
                        .map(|(_, state)| state)
                });
                if let Some(state) = &state {
                    for &slot in &slots {
                        let arg = match state.known[slot] {
                            Known::Value(Bound::Value(value)) => {
                                let ty = self.func.value_types.get(&value)?;
                                if types.get(&slot).is_some_and(|known| known != ty) {
                                    return None;
                                }
                                types.insert(slot, ty.clone());
                                CarryArg::Value(value)
                            }
                            Known::Value(Bound::Carried(at)) if at == header => {
                                CarryArg::Passthrough
                            }
                            _ => return None,
                        };
                        values.insert(slot, arg);
                    }
                }
                edges.push(CarryEdge {
                    pred: *pred,
                    index: *index,
                    inside: state.is_some(),
                    values,
                });
            }
            if types.len() != slots.len() {
                return None;
            }
            carries.insert(
                header,
                Carry {
                    slots: types,
                    edges,
                },
            );
        }
        Some(carries)
    }

    /// Whether `header` carries `slot`'s binding (`Bound::Carried`): it heads
    /// an innermost loop entered only by normal edges, every edge from
    /// outside the loop arrives with the home current, and every edge from
    /// inside holds a raw scalar of one type or the header's own carried
    /// binding. An inside edge may arrive with the home current: a poll on a
    /// path back to the header (a strip-mined loop's boundary) wrote it, and
    /// writing a current home again changes nothing. Requiring stale inside
    /// arrivals made the solve oscillate (a write un-carried the header, which
    /// dropped the write). An arrival still holding the header's previous
    /// state was computed from it and passes its binding through (`meet`).
    fn carries_slot(
        &self,
        header: BlockId,
        slot: usize,
        states: &[Arrival<'_>],
        own: Option<Known>,
    ) -> bool {
        if slot >= self.slots || self.region.get(&header) != Some(&header) {
            return false;
        }
        let mut ty: Option<&TirType> = None;
        let mut any_inside = false;
        for arrival in states {
            match arrival.side {
                Side::Outside => {
                    if arrival.state.dirty[slot] {
                        return false;
                    }
                }
                Side::Foreign => return false,
                Side::Inside => {
                    any_inside = true;
                    match arrival.state.known[slot] {
                        Known::Value(Bound::Carried(at)) if at == header => {}
                        known if Some(known) == own => {}
                        Known::Value(Bound::Value(value)) => {
                            let Some(value_ty) = self.func.value_types.get(&value) else {
                                return false;
                            };
                            if !carriable(value_ty)
                                || !repr_of(self.repr, value).carrier_refcount_is_noop()
                                || ty.is_some_and(|known| known != value_ty)
                            {
                                return false;
                            }
                            ty = Some(value_ty);
                        }
                        _ => return false,
                    }
                }
            }
        }
        any_inside
    }

    /// Entry states of every reachable block, or `None` when the analysis does
    /// not settle.
    fn solve(&self, candidates: &BTreeSet<(BlockId, usize)>) -> Option<HashMap<BlockId, State>> {
        let mut entry: HashMap<BlockId, State> = HashMap::default();
        let mut out: HashMap<(BlockId, usize), State> = HashMap::default();
        let mut changes: HashMap<(BlockId, usize), u8> = HashMap::default();
        let limit = 4 * self.order.len() + 16;
        for _ in 0..limit {
            let mut changed = false;
            for bid in &self.order {
                let Some(mut state) = self.meet(*bid, &out, entry.get(bid)) else {
                    continue;
                };
                if let Some(previous) = entry.get(bid) {
                    bound_changes(*bid, previous, &mut state, &mut changes, self.trace);
                }
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

    /// `previous` is the block's own state from the last round. An arrival
    /// that passes header parameter `p` through unchanged and still holds the
    /// header's previous binding is consistent with "the binding is `p`": its
    /// state was computed from that stale header state (optimistic phi
    /// matching, as in value numbering). At a fixpoint such an arrival holds
    /// `p` itself, so the rule admits nothing the plain match would refuse; a
    /// case it cannot settle oscillates into the change bound.
    fn meet(
        &self,
        bid: BlockId,
        out: &HashMap<(BlockId, usize), State>,
        previous: Option<&State>,
    ) -> Option<State> {
        let mut states: Vec<Arrival<'_>> = Vec::new();
        for incoming in self.incoming.get(&bid).into_iter().flatten() {
            match incoming {
                Incoming::Edge { pred, index, args } => {
                    if let Some(state) = out.get(&(*pred, *index)) {
                        let side = if self.region.get(pred) == Some(&bid) {
                            Side::Inside
                        } else {
                            Side::Outside
                        };
                        states.push(Arrival {
                            args: Some(args.as_slice()),
                            side,
                            state: state.clone(),
                        });
                    }
                }
                Incoming::Exception | Incoming::Entry => {
                    states.push(Arrival {
                        args: None,
                        side: Side::Foreign,
                        state: State::clean(self.words()),
                    });
                }
            }
        }
        if states.is_empty() {
            return None;
        }
        let params = &self.func.blocks[&bid].args;
        let mut merged = State::clean(self.words());
        for slot in 0..self.words() {
            merged.dirty[slot] = states.iter().any(|arrival| arrival.state.dirty[slot]);
            let mut values: Vec<(Option<&[ValueId]>, Bound)> = Vec::new();
            let mut unknown = false;
            for arrival in &states {
                match arrival.state.known[slot] {
                    Known::Top => {}
                    Known::Unknown => unknown = true,
                    Known::Value(value) => values.push((arrival.args, value)),
                }
            }
            let own = previous.map(|state| state.known[slot]);
            let passes_own = |arrival: &Arrival<'_>, param: usize| {
                Some(arrival.state.known[slot]) == own
                    && arrival
                        .args
                        .and_then(|args| args.get(param))
                        .is_some_and(|&arg| arg == params[param].id)
            };
            let param_binding = (0..params.len()).find(|&param| {
                states
                    .iter()
                    .all(|arrival| match arrival.state.known[slot] {
                        Known::Top => true,
                        _ if passes_own(arrival, param) => true,
                        Known::Value(value) => arrival
                            .args
                            .and_then(|args| args.get(param))
                            .is_some_and(|&arg| Bound::Value(self.canon(arg)) == value),
                        Known::Unknown => false,
                    })
            });
            merged.known[slot] = if unknown {
                param_binding.map_or(Known::Unknown, |param| {
                    Known::Value(Bound::Value(params[param].id))
                })
            } else if values.is_empty() {
                Known::Top
            } else if values.iter().all(|(_, value)| *value == values[0].1) {
                Known::Value(values[0].1)
            } else {
                param_binding.map_or(Known::Unknown, |param| {
                    Known::Value(Bound::Value(params[param].id))
                })
            };
            if merged.known[slot] == Known::Unknown && self.carries_slot(bid, slot, &states, own) {
                merged.known[slot] = Known::Value(Bound::Carried(bid));
            }
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
        let location = self.slots;
        for (index, op) in block.ops.iter().enumerate() {
            let at = Point::BeforeOp(bid, index);
            if op.opcode == OpCode::FrameSetLocation {
                if let Some(id) = location_of(op) {
                    state.dirty[location] = candidates.contains(&(bid, index));
                    state.known[location] = Known::Value(Bound::Location(id));
                }
                continue;
            }
            if op.opcode == OpCode::CheckException {
                let (_, reads_location) = check_uses(op);
                if op.is_async_work_poll() && self.split_polls && self.splittable(bid) {
                    // The cold block writes; the normal path stays stale.
                    let mut cold = state.clone();
                    self.write_all(&mut cold, Point::PollCold(bid, index), &mut events);
                } else if op.is_async_work_poll() {
                    self.write_all(&mut state, at, &mut events);
                } else if reads_location {
                    self.write(&mut state, location, at, &mut events);
                }
                continue;
            }
            // The location word's readers are location placement's own.
            if operation_reads(op, &self.exact, self.releases.get(&(bid, index)).copied()) {
                self.write(&mut state, location, at, &mut events);
            }
            if op.opcode == OpCode::Copy
                && let Some(kind) = op.origin
                && (is_home_kind(kind) || kind == OpKind::FrameLocalsSet)
            {
                let slot = home_slot(op).filter(|slot| *slot < self.slots);
                match (kind, slot) {
                    (OpKind::FrameLocalsSet, _) | (_, None) => {
                        self.write_all(&mut state, at, &mut events);
                        let word = state.known[location];
                        state = State::clean(self.words());
                        state.known[location] = word;
                    }
                    (OpKind::FrameHomeStore, Some(slot)) => {
                        let value = Known::Value(Bound::Value(self.canon(op.operands[0])));
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
        for slot in 0..self.words() {
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
            if self.trace {
                eprintln!("[TIR-HOMES] word {slot}: write at {at:?} runs every iteration");
            }
            events.failed.insert(slot);
            return;
        }
        match state.known[slot] {
            Known::Value(Bound::Value(value))
                if repr_of(self.repr, value).carrier_refcount_is_noop() =>
            {
                events
                    .writes
                    .entry(at)
                    .or_default()
                    .push((slot, Bound::Value(value)));
            }
            Known::Value(Bound::Location(id)) => {
                events
                    .writes
                    .entry(at)
                    .or_default()
                    .push((slot, Bound::Location(id)));
            }
            // A carried binding is written under the header's flag, which an
            // edge leaving the loop can test on a block of its own, and so can
            // a reader inside the loop on a path not every iteration takes:
            // its block splits before the reader (`split_carried_reader`).
            // A poll's cold block is placement's own, so it splits there too: a
            // strip-mined loop's boundary poll writes the binding the header
            // carries.
            Known::Value(Bound::Carried(header))
                if matches!(at, Point::Edge(..) | Point::PollCold(..))
                    || matches!(at, Point::BeforeOp(bid, index)
                        if index < self.func.blocks[&bid].ops.len() && self.splittable(bid)) =>
            {
                events
                    .writes
                    .entry(at)
                    .or_default()
                    .push((slot, Bound::Carried(header)));
            }
            ref known => {
                if self.trace {
                    eprintln!("[TIR-HOMES] word {slot}: write at {at:?} has no value ({known:?})");
                }
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

fn location_write(id: i64) -> TirOp {
    let mut op = TirOp::new(OpCode::FrameSetLocation, vec![], vec![]);
    op.attrs.insert("value".into(), AttrValue::Int(id));
    op
}

/// A header's added parameters: its flag and, per carried slot, its value.
struct CarryParams {
    flag: ValueId,
    values: BTreeMap<usize, ValueId>,
}

/// The writes at one point: the unconditional ones, and the carried ones
/// with the flag that guards them.
fn home_writes(
    writes: &[(usize, Bound)],
    params: &HashMap<BlockId, CarryParams>,
) -> (Vec<TirOp>, Option<(ValueId, Vec<TirOp>)>) {
    let mut plain = Vec::new();
    let mut carried: Option<(ValueId, Vec<TirOp>)> = None;
    for &(slot, bound) in writes {
        match bound {
            Bound::Value(value) => plain.push(home_write(slot, value)),
            Bound::Location(id) => plain.push(location_write(id)),
            Bound::Carried(header) => {
                let header = &params[&header];
                let (flag, ops) = carried.get_or_insert_with(|| (header.flag, Vec::new()));
                // One region holds the point, and one header heads it.
                assert_eq!(*flag, header.flag, "a point leaves one loop");
                ops.push(home_write(slot, header.values[&slot]));
            }
        }
    }
    (plain, carried)
}

/// A zero of a carriable type: the value an edge from outside the loop
/// passes for a carried slot. No write reads it.
fn zero_of(ty: &TirType, result: ValueId) -> TirOp {
    let (opcode, key, value) = match ty {
        TirType::I64 => (OpCode::ConstInt, "value", AttrValue::Int(0)),
        TirType::F64 => (OpCode::ConstFloat, "f_value", AttrValue::Float(0.0)),
        TirType::Bool => (OpCode::ConstBool, "value", AttrValue::Bool(false)),
        _ => unreachable!("a carried slot has a carriable type"),
    };
    let mut op = TirOp::new(opcode, vec![], vec![result]);
    op.attrs.insert(key.into(), value);
    op
}

fn const_flag(value: bool, result: ValueId) -> TirOp {
    let mut op = TirOp::new(OpCode::ConstBool, vec![], vec![result]);
    op.attrs.insert("value".into(), AttrValue::Bool(value));
    op
}

/// Give each carrying header its parameters and each edge into it the
/// arguments. Returns the parameters, and per predecessor its op count
/// before and after the constants the arguments need.
fn add_carry_params(
    func: &mut TirFunction,
    carries: &BTreeMap<BlockId, Carry>,
    stats: &mut PassStats,
) -> (
    HashMap<BlockId, CarryParams>,
    HashMap<BlockId, (usize, usize)>,
) {
    let mut params = HashMap::default();
    let mut grown: HashMap<BlockId, (usize, usize)> = HashMap::default();
    for (&header, carry) in carries {
        let flag = func.fresh_value();
        func.value_types.insert(flag, TirType::Bool);
        let mut values = BTreeMap::new();
        let mut added = vec![crate::tir::values::TirValue {
            id: flag,
            ty: TirType::Bool,
        }];
        for (&slot, ty) in &carry.slots {
            let value = func.fresh_value();
            func.value_types.insert(value, ty.clone());
            values.insert(slot, value);
            added.push(crate::tir::values::TirValue {
                id: value,
                ty: ty.clone(),
            });
        }
        func.blocks
            .get_mut(&header)
            .expect("carrying header")
            .args
            .extend(added);
        for edge in &carry.edges {
            let mut consts = Vec::new();
            let flag_arg = func.fresh_value();
            func.value_types.insert(flag_arg, TirType::Bool);
            consts.push(const_flag(edge.inside, flag_arg));
            let mut args = vec![flag_arg];
            for (&slot, ty) in &carry.slots {
                match edge.values.get(&slot) {
                    Some(CarryArg::Value(value)) => args.push(*value),
                    Some(CarryArg::Passthrough) => args.push(values[&slot]),
                    None => {
                        let zero = func.fresh_value();
                        func.value_types.insert(zero, ty.clone());
                        consts.push(zero_of(ty, zero));
                        args.push(zero);
                    }
                }
            }
            stats.ops_added += consts.len();
            let block = func.blocks.get_mut(&edge.pred).expect("carry edge");
            let len = block.ops.len();
            grown.entry(edge.pred).or_insert((len, len)).1 = len + consts.len();
            block.ops.extend(consts);
            let mut seen = 0;
            block.terminator.for_each_edge_mut(|_, edge_args| {
                if seen == edge.index {
                    edge_args.extend(args.iter().copied());
                }
                seen += 1;
            });
        }
        params.insert(header, CarryParams { flag, values });
    }
    (params, grown)
}

/// The LocationId a publication writes.
fn location_of(op: &TirOp) -> Option<i64> {
    match op.attrs.get("value") {
        Some(AttrValue::Int(id)) => Some(*id),
        _ => None,
    }
}

/// Rewrite `func` by `plan`. Returns the carried parameters it wrote, which
/// must own no reference, as the planned values must.
fn apply(func: &mut TirFunction, plan: &Plan, stats: &mut PassStats) -> Vec<ValueId> {
    let (carry_params, grown) = add_carry_params(func, &plan.carries, stats);
    let carried_values: Vec<ValueId> = carry_params
        .values()
        .flat_map(|params| params.values.values().copied())
        .collect();

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
    // Readers inside a loop that write a carried binding under its flag. The
    // reader op carries a marker until every other rewrite of its block is
    // done, then its block splits there.
    let mut carried_readers: Vec<(ValueId, Vec<TirOp>)> = Vec::new();
    type EdgeWrites = (BlockId, usize, Vec<TirOp>, Option<(ValueId, Vec<TirOp>)>);
    let mut edges: Vec<EdgeWrites> = Vec::new();
    for (point, writes) in &plan.writes {
        let (ops, carried) = home_writes(writes, &carry_params);
        stats.ops_added += ops.len() + carried.as_ref().map_or(0, |(_, ops)| ops.len());
        match *point {
            Point::BeforeOp(bid, index) => {
                // The terminator moved past the constants a carry edge added.
                let index = match grown.get(&bid) {
                    Some(&(before, after)) if index == before => after,
                    _ => index,
                };
                before.entry((bid, index)).or_default().extend(ops);
                if let Some((flag, carried_ops)) = carried {
                    let reader = &mut func.blocks.get_mut(&bid).expect("planned block").ops[index];
                    reader.attrs.insert(
                        CARRIED_READER_ATTR.into(),
                        AttrValue::Int(carried_readers.len() as i64),
                    );
                    carried_readers.push((flag, carried_ops));
                }
            }
            Point::PollCold(bid, index) => {
                polls.entry((bid, index)).or_default().extend(ops);
                if let Some((flag, carried_ops)) = carried {
                    // The poll moves to its cold block with this marker, and
                    // that block splits before it under the header's flag.
                    let poll = &mut func.blocks.get_mut(&bid).expect("planned block").ops[index];
                    debug_assert!(poll.is_async_work_poll());
                    poll.attrs.insert(
                        CARRIED_READER_ATTR.into(),
                        AttrValue::Int(carried_readers.len() as i64),
                    );
                    carried_readers.push((flag, carried_ops));
                }
            }
            Point::Edge(bid, index) => edges.push((bid, index, ops, carried)),
        }
    }

    for (pred, index, ops, carried) in edges {
        let mut target = None;
        let mut seen = 0;
        func.blocks[&pred].terminator.for_each_edge(|to, _| {
            if seen == index {
                target = Some(to);
            }
            seen += 1;
        });
        let target = target.expect("a planned edge exists");
        if carried.is_none() && target != func.entry_block && ways_in.get(&target) == Some(&1) {
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
        // A carried write runs only when the loop body ran:
        //   split: <plain writes>; cond_br %flag, ^write, ^target(args)
        //   write: <carried writes>; br ^target(args)
        let terminator = match carried {
            None => Terminator::Branch { target, args },
            Some((flag, carried_ops)) => {
                let write = func.fresh_block();
                func.blocks.insert(
                    write,
                    TirBlock {
                        id: write,
                        args: vec![],
                        ops: carried_ops,
                        terminator: Terminator::Branch {
                            target,
                            args: args.clone(),
                        },
                    },
                );
                Terminator::CondBranch {
                    cond: flag,
                    then_block: write,
                    then_args: vec![],
                    else_block: target,
                    else_args: args,
                }
            }
        };
        func.blocks.insert(
            split,
            TirBlock {
                id: split,
                args: vec![],
                ops,
                terminator,
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
    for (site, (flag, writes)) in carried_readers.into_iter().enumerate() {
        split_carried_reader(func, site as i64, flag, writes);
        stats.ops_added += 1;
    }
    carried_values
}

/// Marks the reader before which a carried binding is written.
const CARRIED_READER_ATTR: &str = "frame_home_carried_reader";

/// Split the block of the reader marked `site` so a carried binding is
/// written before it only when the loop body ran (the header's flag):
///
/// ```text
/// bid:    ...                        ; ops before the reader
///         cond_br %flag, ^write, ^cont
/// write:  frame_home_store ...       ; the carried bindings
///         br ^cont
/// cont:   reader ...                 ; the reader and the rest; bid's terminator
/// ```
fn split_carried_reader(func: &mut TirFunction, site: i64, flag: ValueId, writes: Vec<TirOp>) {
    let marked = |op: &TirOp| op.attrs.get(CARRIED_READER_ATTR) == Some(&AttrValue::Int(site));
    let (bid, at) = func
        .blocks
        .iter()
        .find_map(|(bid, block)| block.ops.iter().position(marked).map(|at| (*bid, at)))
        .expect("a marked reader");
    let write = func.fresh_block();
    let cont = func.fresh_block();
    let block = func.blocks.get_mut(&bid).expect("reader block");
    let mut tail = block.ops.split_off(at);
    tail[0].attrs.remove(CARRIED_READER_ATTR);
    let terminator = std::mem::replace(
        &mut block.terminator,
        Terminator::CondBranch {
            cond: flag,
            then_block: write,
            then_args: vec![],
            else_block: cont,
            else_args: vec![],
        },
    );
    func.blocks.insert(
        write,
        TirBlock {
            id: write,
            args: vec![],
            ops: writes,
            terminator: Terminator::Branch {
                target: cont,
                args: vec![],
            },
        },
    );
    func.blocks.insert(
        cont,
        TirBlock {
            id: cont,
            args: vec![],
            ops: tail,
            terminator,
        },
    );
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
