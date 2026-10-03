//! Program-point availability of owned roots for DropInsertion placement.
//!
//! An RC operation may name a root only where the root is defined on every path
//! to that point, including a path that enters a block through a mid-block
//! exception observation, where the root's bits are initialized, and where the
//! root's name still owns its object. `ProgramPointDominance` answers the first
//! question. The second is the conditional-validity region that
//! `OwnershipRootFacts` records: an `IterNextUnboxed` value is initialized only
//! below its not-done edge. The third is custody, recorded once by
//! `PhiTransport`. Every placement consumer asks this one authority: exception
//! observations in straight-line code, block-entry, block-exit and arc
//! releases, phi retains, and lexical Python lifetimes. Exceptional landings
//! ask for definitions and initialization only: they release what final
//! liveness abandons, and a root that no longer owns its object is dead there.
//!
//! Custody follows every canonical control arc. A terminator arc binds its
//! target's block arguments at its source's exit, and an observation's arc
//! binds its handler's arguments at the observation, only when it raises. A
//! region registration's arc keeps its handler reachable, so custody counts it
//! among the handler's entries and follows it as program-point dominance does,
//! but control never leaves through it and it binds nothing. Each binding of an
//! owned argument is classified once: a
//! function-owned root that the target body does not read moves its own +1
//! into the first argument it binds, and any other owned binding is retained on
//! its arc. A root's name also gives up its object at an explicit release and at
//! an operation that adopts its own +1 (`transfers.rs`). From such a point the
//! root owns nothing on any path until its definition runs again, although that
//! definition still reaches: the argument, the release or the adopting
//! operation has taken the object.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

use crate::tir::blocks::{BlockId, TirBlock};
use crate::tir::dominators::ProgramPointDominance;
use crate::tir::exception_regions::ExceptionOpPosition;
use crate::tir::function::TirFunction;
use crate::tir::passes::liveness::TirLivenessResult;
use crate::tir::passes::ownership_lattice_min::{
    DropEligibility, OwnershipRootFacts, explicit_release_values,
};
use crate::tir::values::ValueId;

use super::arcs::{ArcSite, ControlArc, control_arcs, exception_arcs_for_block};

#[derive(Debug, Clone, Copy)]
pub(super) struct ValueDefinition {
    block: BlockId,
    op_index: Option<usize>,
}

impl ValueDefinition {
    /// Whether this definition runs before `op_index` of `block` on the way
    /// through that block. `usize::MAX` is the terminator boundary.
    fn precedes(self, block: BlockId, op_index: usize) -> bool {
        self.block == block && self.op_index.is_none_or(|index| index < op_index)
    }
}

pub(super) fn value_definitions(func: &TirFunction) -> HashMap<ValueId, ValueDefinition> {
    let mut defs: HashMap<ValueId, ValueDefinition> = HashMap::new();
    for (&bid, block) in &func.blocks {
        for arg in &block.args {
            defs.insert(
                arg.id,
                ValueDefinition {
                    block: bid,
                    op_index: None,
                },
            );
        }
        for (op_index, op) in block.ops.iter().enumerate() {
            for &result in &op.results {
                defs.insert(
                    result,
                    ValueDefinition {
                        block: bid,
                        op_index: Some(op_index),
                    },
                );
            }
        }
    }
    defs
}

pub(super) fn definition_available_before_position(
    def: ValueDefinition,
    position: ExceptionOpPosition,
    dominance: &ProgramPointDominance,
) -> bool {
    dominance.definition_available(def.block, def.op_index, position.block, position.op_index)
}

pub(super) fn definition_available_on_edge(
    def: ValueDefinition,
    pred: BlockId,
    dominance: &ProgramPointDominance,
) -> bool {
    dominance.definition_available(def.block, def.op_index, pred, usize::MAX)
}

/// What one control arc does with the owner behind a target block argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PhiPayload {
    /// No release obligation: a raw carrier, an argument that owns nothing, or
    /// a conditional result the arc does not initialize.
    Unowned,
    /// The arc moves this root's own +1 into the argument.
    Transfer(ValueId),
    /// The argument needs its own +1, retained on the arc.
    Retain(ValueId),
}

/// Custody over every canonical control arc: how each arc binds its target's
/// block arguments, and where each root's name gives up its object.
#[derive(Default)]
struct PhiTransport {
    /// Each binding arc's bindings in argument order, keyed by where the arc
    /// leaves. A region registration has none.
    payloads: HashMap<(BlockId, ArcSite), Vec<PhiPayload>>,
    /// Roots each arc target's body reads other than through its own arguments.
    body_live: HashMap<BlockId, HashSet<ValueId>>,
    /// The source of each canonical arc into a block, one entry per arc.
    incoming: HashMap<BlockId, Vec<BlockId>>,
    /// Each executable block's outgoing arcs: where each leaves, and its target.
    successors: HashMap<BlockId, Vec<(usize, BlockId)>>,
    /// Block arguments, by position, that some binding arc binds without an
    /// owned reference: a raw or uninitialized input, or no input at all.
    unowned_bindings: HashSet<(BlockId, usize)>,
    /// Per root, the target and argument of every arc that moves its +1.
    moves: HashMap<ValueId, Vec<(BlockId, ValueId)>>,
    /// Per root, the operations after which its name no longer owns its
    /// object: explicit releases, and adopting operations that took its own
    /// +1 (`PointAvailability::retire_adopted`).
    retirements: HashMap<ValueId, Vec<(BlockId, usize)>>,
    /// Per queried root, the blocks some path enters after the root gave up its
    /// object and before its definition ran again.
    regions: RefCell<HashMap<ValueId, HashSet<BlockId>>>,
    /// Blocks in all computed regions, for the stage audit.
    region_blocks: Cell<usize>,
}

impl PhiTransport {
    fn compute(
        func: &TirFunction,
        points: &PointAvailability<'_>,
        eligibility: &DropEligibility<'_>,
        live: &TirLivenessResult,
        labels: &HashMap<i64, BlockId>,
        reachable: &HashSet<BlockId>,
    ) -> Self {
        let canon = |value: ValueId| eligibility.root(value);
        let mut sources: Vec<BlockId> = reachable.iter().copied().collect();
        sources.sort_unstable_by_key(|block| block.0);
        let mut transport = Self::default();
        for source in sources {
            let block = &func.blocks[&source];
            for (index, op) in block.ops.iter().enumerate() {
                for root in explicit_release_values(op).map(canon) {
                    transport
                        .retirements
                        .entry(root)
                        .or_default()
                        .push((source, index));
                }
            }
            for arc in control_arcs(labels, block) {
                let Some(target) = func.blocks.get(&arc.target) else {
                    continue;
                };
                if !reachable.contains(&arc.target) {
                    continue;
                }
                transport
                    .incoming
                    .entry(arc.target)
                    .or_default()
                    .push(source);
                transport
                    .successors
                    .entry(source)
                    .or_default()
                    .push((arc.site.position(), arc.target));
                // A region registration keeps its handler reachable: the handler
                // counts it as an entry and retirement follows it. It never
                // raises into the handler, so it binds no argument.
                if matches!(arc.site, ArcSite::Registration(_)) {
                    continue;
                }
                let body_live = transport.body_live.entry(arc.target).or_insert_with(|| {
                    live.live_in
                        .get(&arc.target)
                        .into_iter()
                        .flatten()
                        .filter(|value| target.args.iter().all(|arg| arg.id != **value))
                        .map(|&value| canon(value))
                        .collect()
                });
                if target.args.is_empty() {
                    continue;
                }
                let payloads = points.classify(eligibility, source, &arc, target, body_live);
                for (position, argument) in target.args.iter().enumerate() {
                    match payloads.get(position) {
                        // A phi bound to itself stays with the same owner.
                        Some(&PhiPayload::Transfer(root)) if root != argument.id => {
                            transport
                                .moves
                                .entry(root)
                                .or_default()
                                .push((arc.target, argument.id));
                        }
                        Some(PhiPayload::Transfer(_) | PhiPayload::Retain(_)) => {}
                        Some(PhiPayload::Unowned) | None => {
                            transport.unowned_bindings.insert((arc.target, position));
                        }
                    }
                }
                transport.payloads.insert((source, arc.site), payloads);
            }
        }
        transport
    }

    /// Whether some path enters `block` after `root`'s name gave up its object
    /// and before the root's definition ran again. A move gives it up on its own
    /// arc, and an explicit release or an adoption on every later arc of its
    /// block. Computed once per queried root.
    fn retired_at_entry(
        &self,
        root: ValueId,
        definition: Option<ValueDefinition>,
        block: BlockId,
    ) -> bool {
        let moves = self.moves.get(&root);
        let retirements = self.retirements.get(&root);
        if moves.is_none() && retirements.is_none() {
            return false;
        }
        if let Some(region) = self.regions.borrow().get(&root) {
            return region.contains(&block);
        }
        // Whether the definition runs again in `current` after the retirement
        // at `after` (on entry when `None`) and before the arc leaving at
        // `position`.
        let redefined = |current: BlockId, after: Option<usize>, position: usize| {
            definition.is_some_and(|defined| {
                defined.block == current
                    && match (defined.op_index, after) {
                        (None, None) => true,
                        (None, Some(_)) => false,
                        (Some(index), None) => index < position,
                        (Some(index), Some(retired)) => retired < index && index < position,
                    }
            })
        };
        let mut pending: Vec<BlockId> = moves
            .into_iter()
            .flatten()
            .map(|&(target, _)| target)
            .collect();
        for &(at, retired) in retirements.into_iter().flatten() {
            for &(position, next) in self.successors.get(&at).into_iter().flatten() {
                if retired < position && !redefined(at, Some(retired), position) {
                    pending.push(next);
                }
            }
        }
        let mut region = HashSet::new();
        while let Some(current) = pending.pop() {
            if !region.insert(current) {
                continue;
            }
            for &(position, next) in self.successors.get(&current).into_iter().flatten() {
                if !redefined(current, None, position) {
                    pending.push(next);
                }
            }
        }
        self.region_blocks
            .set(self.region_blocks.get().saturating_add(region.len()));
        let retired = region.contains(&block);
        self.regions.borrow_mut().insert(root, region);
        retired
    }
}

/// Where an RC operation may name an owned root.
pub(super) struct PointAvailability<'a> {
    dominance: ProgramPointDominance,
    definitions: HashMap<ValueId, ValueDefinition>,
    root_facts: &'a OwnershipRootFacts,
    transport: PhiTransport,
}

impl<'a> PointAvailability<'a> {
    /// Definitions and initialization only, for the landing pass.
    pub(super) fn compute(func: &TirFunction, root_facts: &'a OwnershipRootFacts) -> Self {
        Self {
            dominance: ProgramPointDominance::compute(func),
            definitions: value_definitions(func),
            root_facts,
            transport: PhiTransport::default(),
        }
    }

    /// Definitions, initialization and custody over every control arc that
    /// leaves a `reachable` block.
    pub(super) fn compute_with_transport(
        func: &TirFunction,
        root_facts: &'a OwnershipRootFacts,
        eligibility: &DropEligibility<'_>,
        live: &TirLivenessResult,
        labels: &HashMap<i64, BlockId>,
        reachable: &HashSet<BlockId>,
    ) -> Self {
        let mut points = Self::compute(func, root_facts);
        points.transport =
            PhiTransport::compute(func, &points, eligibility, live, labels, reachable);
        points
    }

    /// The last point of `block` at which each root is read: an operation's
    /// operand, or the handler demand of an exception arc that leaves at that
    /// operation. A direct handler use is a use at the exact transfer even when
    /// SSA needs no block argument, so a root's normal-path release follows the
    /// observation and the exceptional path keeps the owner. A root that some
    /// path to the observation does not define, initialize or own cannot be
    /// kept for the handler there. Last-use releases and the adoption plan read
    /// this one projection.
    pub(super) fn last_reads(
        &self,
        block: &TirBlock,
        id: BlockId,
        live: &TirLivenessResult,
        labels: &HashMap<i64, BlockId>,
        canon: &dyn Fn(ValueId) -> ValueId,
    ) -> HashMap<ValueId, usize> {
        let mut last = HashMap::new();
        for (index, op) in block.ops.iter().enumerate() {
            for &operand in &op.operands {
                last.insert(canon(operand), index);
            }
        }
        for arc in exception_arcs_for_block(labels, block) {
            for &value in live.live_in.get(&arc.target).into_iter().flatten() {
                let root = canon(value);
                if self.available_before(root, id, arc.op_index) {
                    let read = last.entry(root).or_insert(arc.op_index);
                    *read = (*read).max(arc.op_index);
                }
            }
        }
        last
    }

    /// Record that each root's name gives up its object after the operation
    /// that adopted its own +1 (`transfers.rs`). A retirement region cached
    /// before this call is recomputed on its next query.
    pub(super) fn retire_adopted(
        &mut self,
        moved: impl IntoIterator<Item = (BlockId, usize, ValueId)>,
    ) {
        for (block, op_index, root) in moved {
            self.transport
                .retirements
                .entry(root)
                .or_default()
                .push((block, op_index));
            if let Some(stale) = self.transport.regions.get_mut().remove(&root) {
                let blocks = self.transport.region_blocks.get_mut();
                *blocks = blocks.saturating_sub(stale.len());
            }
        }
    }

    fn defined_before(&self, root: ValueId, block: BlockId, op_index: usize) -> bool {
        self.definitions.get(&root).is_some_and(|definition| {
            self.dominance.definition_available(
                definition.block,
                definition.op_index,
                block,
                op_index,
            )
        })
    }

    fn valid_before(&self, root: ValueId, block: BlockId, op_index: usize) -> bool {
        !self.root_facts.is_conditionally_valid_result_root(root)
            || self
                .root_facts
                .conditionally_valid_region(root)
                .is_some_and(|region| {
                    self.dominance
                        .definition_available(region, None, block, op_index)
                })
    }

    /// Whether `root`'s name still owns its object immediately before
    /// `block.ops[op_index]`: no path reaches that point after a move, an
    /// explicit release or an adoption of its own +1, unless its definition
    /// ran again on the way.
    fn owned_before(&self, root: ValueId, block: BlockId, op_index: usize) -> bool {
        let definition = self.definitions.get(&root).copied();
        let retired_here = self
            .transport
            .retirements
            .get(&root)
            .into_iter()
            .flatten()
            .filter(|&&(at, index)| at == block && index < op_index)
            .map(|&(_, index)| index)
            .max();
        match (definition, retired_here) {
            (Some(defined), Some(retired)) if defined.block == block => defined
                .op_index
                .is_some_and(|index| retired < index && index < op_index),
            (_, Some(_)) => false,
            (Some(defined), None) if defined.precedes(block, op_index) => true,
            (definition, None) => !self.transport.retired_at_entry(root, definition, block),
        }
    }

    /// Whether an RC operation immediately before `block.ops[op_index]` may name
    /// `root`. `usize::MAX` queries the terminator boundary.
    pub(super) fn available_before(&self, root: ValueId, block: BlockId, op_index: usize) -> bool {
        self.defined_before(root, block, op_index)
            && self.valid_before(root, block, op_index)
            && self.owned_before(root, block, op_index)
    }

    pub(super) fn available_at_entry(&self, root: ValueId, block: BlockId) -> bool {
        self.available_before(root, block, 0)
    }

    pub(super) fn available_at_exit(&self, root: ValueId, block: BlockId) -> bool {
        self.available_before(root, block, usize::MAX)
    }

    /// Whether a release placed on the normal arc `pred -> target` may name
    /// `root`.
    pub(super) fn available_on_arc(&self, root: ValueId, pred: BlockId, target: BlockId) -> bool {
        self.defined_before(root, pred, usize::MAX)
            && self.valid_on_arc(root, pred, target)
            && self.owned_before(root, pred, usize::MAX)
    }

    /// Validity alone, for a retain of a value the arc already forwards: the
    /// arc's source has initialized it, or the arc is the edge that does.
    pub(super) fn valid_on_arc(&self, root: ValueId, pred: BlockId, target: BlockId) -> bool {
        self.valid_before(root, pred, usize::MAX)
            || self.root_facts.conditionally_valid_region(root) == Some(target)
    }

    /// The block that defines `value`: the block it is an argument of, or the
    /// block of the operation producing it.
    pub(super) fn definition_block(&self, value: ValueId) -> Option<BlockId> {
        self.definitions
            .get(&value)
            .map(|definition| definition.block)
    }

    /// The block whose operation produces `value`; `None` for a block argument.
    pub(super) fn result_block(&self, value: ValueId) -> Option<BlockId> {
        self.definitions
            .get(&value)
            .filter(|definition| definition.op_index.is_some())
            .map(|definition| definition.block)
    }

    /// The source of each canonical arc into `block`, one entry per arc.
    pub(super) fn incoming_sources(&self, block: BlockId) -> &[BlockId] {
        self.transport
            .incoming
            .get(&block)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// The values one control arc must retain: each binds an owned block
    /// argument that cannot take its root's own +1.
    pub(super) fn retains(&self, source: BlockId, site: ArcSite) -> Vec<ValueId> {
        self.payloads(source, site)
            .iter()
            .filter_map(|payload| match *payload {
                PhiPayload::Retain(value) => Some(value),
                PhiPayload::Unowned | PhiPayload::Transfer(_) => None,
            })
            .collect()
    }

    /// The argument positions whose values `retains` names, in order.
    pub(super) fn retained_positions(&self, source: BlockId, site: ArcSite) -> Vec<usize> {
        self.payloads(source, site)
            .iter()
            .enumerate()
            .filter(|(_, payload)| matches!(payload, PhiPayload::Retain(_)))
            .map(|(position, _)| position)
            .collect()
    }

    /// Whether the arc leaving `source` at `site` moves `root`'s own +1.
    pub(super) fn moves_on(&self, source: BlockId, site: ArcSite, root: ValueId) -> bool {
        self.payloads(source, site)
            .contains(&PhiPayload::Transfer(root))
    }

    /// Whether the body of `target` reads `root` other than through its own
    /// block arguments. Such a root cannot move into one of them, and a
    /// release on an arc into `target` would free an object the body uses.
    pub(super) fn lives_into_body(&self, root: ValueId, target: BlockId) -> bool {
        self.transport
            .body_live
            .get(&target)
            .is_some_and(|roots| roots.contains(&root))
    }

    /// Whether every arc that binds `block`'s arguments binds the one at
    /// `position` to an owned reference, moved or retained, never a raw or
    /// uninitialized input. A region registration binds none of them.
    pub(super) fn binds_owner_on_every_arc(&self, block: BlockId, position: usize) -> bool {
        !self.transport.unowned_bindings.contains(&(block, position))
    }

    /// `roots` and, transitively, every block argument that some arc moves one
    /// of them into: the carriers that now hold their objects.
    pub(super) fn with_carriers(
        &self,
        roots: impl IntoIterator<Item = ValueId>,
    ) -> HashSet<ValueId> {
        let mut closed = HashSet::new();
        let mut pending: Vec<ValueId> = roots.into_iter().collect();
        while let Some(root) = pending.pop() {
            if !closed.insert(root) {
                continue;
            }
            for &(_, argument) in self.transport.moves.get(&root).into_iter().flatten() {
                pending.push(argument);
            }
        }
        closed
    }

    /// Moves and canonical arcs, for the stage audit.
    pub(super) fn transport_counts(&self) -> (usize, usize) {
        (
            self.transport.moves.values().map(Vec::len).sum(),
            self.transport.incoming.values().map(Vec::len).sum(),
        )
    }

    /// Retirement regions computed so far and the blocks they cover, for the
    /// stage audit.
    pub(super) fn retirement_counts(&self) -> (usize, usize) {
        (
            self.transport.regions.borrow().len(),
            self.transport.region_blocks.get(),
        )
    }

    fn payloads(&self, source: BlockId, site: ArcSite) -> &[PhiPayload] {
        self.transport
            .payloads
            .get(&(source, site))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Classify one arc's bindings in argument order. An owned argument takes
    /// its root's own +1 when the root is function-owned, the target body does
    /// not read it, and no earlier argument on this arc took it. Droppability
    /// is tested on the root: a transparent alias of a fresh owner transfers,
    /// and a parameter, stack or non-owning root is borrowed. Every other owned
    /// argument is retained on the arc. A raw carrier holds no reference, and a
    /// conditional result that the arc does not initialize holds stale bits, so
    /// neither binds an obligation.
    fn classify(
        &self,
        eligibility: &DropEligibility<'_>,
        source: BlockId,
        arc: &ControlArc,
        target: &TirBlock,
        body_live: &HashSet<ValueId>,
    ) -> Vec<PhiPayload> {
        let mut moved = HashSet::new();
        arc.args
            .iter()
            .zip(&target.args)
            .map(|(&value, argument)| {
                let root = eligibility.root(value);
                let initialized = match arc.site {
                    ArcSite::Terminator(_) => self.valid_on_arc(root, source, arc.target),
                    ArcSite::Exception(op_index) => self.valid_before(root, source, op_index),
                    ArcSite::Registration(_) => {
                        unreachable!("a region registration binds no handler argument")
                    }
                };
                if !eligibility.is_droppable(argument.id)
                    || eligibility.is_raw_scalar_root(root)
                    || !initialized
                {
                    PhiPayload::Unowned
                } else if eligibility.is_droppable(root)
                    && !body_live.contains(&root)
                    && moved.insert(root)
                {
                    PhiPayload::Transfer(root)
                } else {
                    PhiPayload::Retain(value)
                }
            })
            .collect()
    }
}
