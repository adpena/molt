//! Ownership lattice â€” minimal slice (the #58 finalizer-ORDERING keystone).
//!
//! THE BUG (#58, doc 50 Â§A): a finalizer-sensitive value is released at its SSA
//! last-READ, not at its Python-visible lifetime boundary (`del` statement / scope
//! exit), so `__del__` fires too early. Repro `c_scope`:
//! ```python
//! def run():
//!     bag = [A()]        # A defines __del__; bag is never read again
//!     print("in run")    # CPython: __del__ runs AFTER this (scope exit)
//! ```
//! molt drops `bag` at its SSA last-use (the assignment) â†’ the list â†’ `A` â†’ DEL
//! fires before `print`. CPython holds the local to frame teardown.
//!
//! THE FIX DIRECTION (council-binding, CLAUDE.md): a minimal OWNERSHIP LATTICE,
//! NOT another DropInsertion special-case. The rungs:
//!   * alias-root â€” the canonical owning value (rung 0; full alias unification is a
//!     later rung â€” here a value is its own root except across the pure-move copies
//!     `finalizer_alloc_roots` already folds).
//!   * **FinalizerSensitive** â€” the transitive closure of `finalizer_alloc_roots`
//!     through container owners: releasing such a value can fire a `__del__`.
//!   * **AbsorbedFinalizerProducer** â€” a finalizer-sensitive producer operand has
//!     been retained by a container owner at this statement. The producer's own
//!     caller ref can release at this absorption boundary; the container owner
//!     remains FinalizerSensitive until its Python lifetime boundary.
//!
//! STATUS â€” ACTIVE. DropInsertion consumes this lattice to extend a
//! FinalizerSensitive value's release to the Python lifetime boundary. Non-
//! finalizer values KEEP SSA-last-use release (no perf loss); the gate is
//! exactly this generated fact-plane set.

use std::collections::{HashMap, HashSet};

use crate::ir::ParameterCustody;
use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    ExplicitReleaseOperands, OperandCategory, OperandOwnership, TerminatorKind,
    kind_consumed_operand_table, kind_container_absorbed_operand_table,
    kind_result_absorbs_operand_ownership_table, kind_result_finalizer_source_operand_table,
    opcode_container_absorbed_operand, opcode_explicit_release_operands_table,
    opcode_operand_ownership_table, opcode_result_absorbs_operand_ownership_table,
    opcode_result_is_conditionally_valid_only_on_edge, terminator_operand_is_transferred,
};
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::values::ValueId;

use super::alias_analysis::{
    AliasUnionFind, CopyLowering, classify_copy_kind, copy_kind_is_exception_creation_ref,
    copy_kind_is_explicit_no_heap_move,
};
use super::escape_analysis::finalizer_alloc_roots;

mod replacement;

pub(crate) use replacement::{Replacements, owned_alias};

pub(crate) fn original_kind(op: &TirOp) -> Option<&str> {
    match op.attrs.get("_original_kind") {
        Some(AttrValue::Str(kind)) => Some(kind.as_str()),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NoHeapCopyAlias {
    pub(crate) source: ValueId,
    pub(crate) result: ValueId,
}

/// True when the result owns the operand lifetimes. This is generated fact-plane
/// authority, split by representation: first-class TIR opcodes read the opcode
/// table; Copy-preserved SimpleIR spellings read the `_original_kind` table.
pub(crate) fn op_result_absorbs_operand_ownership(op: &TirOp) -> bool {
    opcode_result_absorbs_operand_ownership_table(op.opcode)
        || (op.opcode == OpCode::Copy
            && original_kind(op).is_some_and(kind_result_absorbs_operand_ownership_table))
}

/// How an operation takes one of its operands' references.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OperandTransfer {
    /// The op borrows the operand; its holder keeps its reference.
    Borrowed,
    /// The op consumes the operand by its generated ownership, such as the
    /// CallArgs builder `call_bind` frees. A Python binding of that operand
    /// remains bound and supplies a retained reference.
    Consumed,
    /// The consumed operand becomes a home-owned Python binding. The generated
    /// consumed position and binding-view result facts jointly identify this
    /// transfer. Any former SSA binding owner must move into the home; keeping
    /// it would change the binding's observable lifetime.
    BindingStore,
    /// A source Python call instruction adopts the operand by its typed
    /// operand custody. The invocation owns the reference, and a Python
    /// binding that passed it stays bound after the call.
    Adopted,
}

/// How `op` takes each operand, in operand order. The op, its callee or its
/// runtime entry owns each consumed or adopted reference on the normal and the
/// exceptional continuation alike, whether or not a callee runs; a root named
/// at two taking positions owes two references. Taking is declared, never
/// inferred from use shape: generated operand ownership marks the consuming
/// spellings and opcodes, and a source call's typed operand custody marks the
/// operands its Python call instruction adopts: its arguments, and an ordinary
/// call's callable. A position both declare is consumed. An explicit release
/// (`DecRef`, `DeleteVar`, `DelBoundary`) takes nothing: it ends the holder's
/// own reference.
pub(crate) fn op_transferred_operands(op: &TirOp) -> Vec<OperandTransfer> {
    let arity = op.operands.len();
    let spelling_consumed =
        original_kind(op).and_then(|kind| kind_consumed_operand_table(kind, arity));
    let binding_store =
        op.opcode == OpCode::Copy && original_kind(op).is_some_and(is_binding_view_kind);
    let released = opcode_explicit_release_operands_table(op.opcode, arity);
    (0..arity)
        .map(|index| {
            let opcode_consumed = opcode_operand_ownership_table(op.opcode, index)
                == OperandOwnership::Consumed
                && released != ExplicitReleaseOperands::All
                && released != ExplicitReleaseOperands::One(index);
            if spelling_consumed == Some(index) && binding_store {
                OperandTransfer::BindingStore
            } else if spelling_consumed == Some(index) || opcode_consumed {
                OperandTransfer::Consumed
            } else if op.operand_custody(index) == ParameterCustody::Transferred {
                OperandTransfer::Adopted
            } else {
                OperandTransfer::Borrowed
            }
        })
        .collect()
}

/// A `Copy` that aliases exactly one operand into one result without creating or
/// moving a heap ownership obligation. DropPlacement may remap SSA through this
/// alias during CFG surgery; the classifier read itself stays in the ownership
/// fact module.
pub(crate) fn copy_transparent_alias(op: &TirOp) -> Option<NoHeapCopyAlias> {
    if op.opcode != OpCode::Copy || op.operands.len() != 1 || op.results.len() != 1 {
        return None;
    }
    if !copy_kind_is_explicit_no_heap_move(original_kind(op)) {
        return None;
    }
    Some(NoHeapCopyAlias {
        source: op.operands[0],
        result: op.results[0],
    })
}

/// SSA values whose `_original_kind` marks a fresh exception CreationRef.
/// DropInsertion owns the raise-boundary placement; this helper owns the
/// lifetime fact that the value is released by the runtime exception-state
/// transfer at `Raise`.
pub(crate) fn exception_creation_ref_values(func: &TirFunction) -> HashSet<ValueId> {
    let mut values = HashSet::new();
    for block in func.blocks.values() {
        for op in &block.ops {
            if op.opcode != OpCode::Copy {
                continue;
            }
            if !original_kind(op).is_some_and(copy_kind_is_exception_creation_ref) {
                continue;
            }
            values.extend(op.results.iter().copied());
        }
    }
    values
}

/// The zero-cost discriminant of `term`, the key for the generated
/// per-terminator operand-ownership authority (`terminator_operand_ownership_table`
/// / `terminator_operand_is_transferred`, design 27 Â§2.4). The ownership fact is
/// declarative (op_kinds.toml `[[terminator]]`); this structural shape map only
/// identifies which terminator variant carries which generated fact row.
fn terminator_kind(term: &Terminator) -> TerminatorKind {
    match term {
        Terminator::Branch { .. } => TerminatorKind::Branch,
        Terminator::CondBranch { .. } => TerminatorKind::CondBranch,
        Terminator::Switch { .. } => TerminatorKind::Switch,
        Terminator::StateDispatch { .. } => TerminatorKind::StateDispatch,
        Terminator::Return { .. } => TerminatorKind::Return,
        Terminator::Unreachable => TerminatorKind::Unreachable,
    }
}

/// Values forwarded as successor block args when the generated terminator
/// authority classifies `BranchArg` ownership as transferred. The drop-placement
/// pass consumes this as the dual of phi ownership: the outgoing value has moved
/// into the successor block arg and must not also be edge-dropped.
pub(crate) fn terminator_branch_args(term: &Terminator) -> HashSet<ValueId> {
    let mut out = HashSet::new();
    if !terminator_operand_is_transferred(terminator_kind(term), OperandCategory::BranchArg) {
        return out;
    }
    match term {
        Terminator::Branch { args, .. } => out.extend(args.iter().copied()),
        Terminator::CondBranch {
            then_args,
            else_args,
            ..
        } => {
            out.extend(then_args.iter().copied());
            out.extend(else_args.iter().copied());
        }
        Terminator::Switch {
            cases,
            default_args,
            ..
        }
        | Terminator::StateDispatch {
            cases,
            default_args,
            ..
        } => {
            for (_, _, args) in cases {
                out.extend(args.iter().copied());
            }
            out.extend(default_args.iter().copied());
        }
        Terminator::Return { .. } | Terminator::Unreachable => {}
    }
    out
}

/// True if alias root `root` is read directly by `term`: either transferred by
/// the direct terminator slot (currently Return values) or borrowed by a direct
/// predicate slot (CondBranch/Switch). Both cases block straight-line drops at
/// the producing op; the generated table owns the transfer classification.
pub(crate) fn terminator_uses_root(
    term: &Terminator,
    root: ValueId,
    canon: &dyn Fn(ValueId) -> ValueId,
) -> bool {
    if terminator_operand_is_transferred(terminator_kind(term), OperandCategory::Direct)
        && let Terminator::Return { values } = term
        && values.iter().any(|&value| canon(value) == root)
    {
        return true;
    }
    match term {
        Terminator::CondBranch { cond, .. } => canon(*cond) == root,
        Terminator::Switch { value, .. } => canon(*value) == root,
        Terminator::StateDispatch { .. }
        | Terminator::Branch { .. }
        | Terminator::Return { .. }
        | Terminator::Unreachable => false,
    }
}

/// Existing-container/store absorption: operand 0 is the owner container and the
/// returned index is the value operand retained by that container. The operand
/// is still borrowed for ABI/drop purposes; this fact only supplies the producer
/// temp's finalizer release boundary.
fn op_container_absorbed_operand(op: &TirOp) -> Option<usize> {
    opcode_container_absorbed_operand(op.opcode).or_else(|| {
        original_kind(op)
            .and_then(|kind| kind_container_absorbed_operand_table(kind, op.operands.len()))
    })
}

/// A fresh result that inherits finalizer sensitivity from one source operand
/// while remaining a statement temporary unless Python-bound (for example,
/// `list_pop(list)` returning the popped element).
fn op_result_finalizer_source_operand(op: &TirOp) -> Option<usize> {
    (op.opcode == OpCode::Copy)
        .then(|| {
            original_kind(op).and_then(|kind| {
                kind_result_finalizer_source_operand_table(kind, op.operands.len())
            })
        })
        .flatten()
}

fn conditionally_valid_result_roots(
    func: &TirFunction,
    aliases: &AliasUnionFind,
) -> HashSet<ValueId> {
    let mut roots = HashSet::new();
    for block in func.blocks.values() {
        for op in &block.ops {
            for (result_idx, &result) in op.results.iter().enumerate() {
                if opcode_result_is_conditionally_valid_only_on_edge(op.opcode, result_idx) {
                    roots.insert(aliases.root(result));
                }
            }
        }
    }
    roots
}

/// The initialized region of each conditionally-valid result root. The
/// generated validity row marks a result written only on its producer's
/// continuation edge. That edge is the `else` arm of the producer block's
/// `CondBranch` on the producer's own completion flag (another result of the
/// same operation): the `IterNextUnboxed` not-done edge that range and iterator
/// devirtualization also consume. The region is that arm's target, and only
/// when the arm is the target's sole entry, normal or exceptional. Any other
/// shape records no region, so the root stays uninitialized at every point.
fn conditionally_valid_regions(
    func: &TirFunction,
    aliases: &AliasUnionFind,
) -> HashMap<ValueId, BlockId> {
    let labels = crate::tir::dominators::exception_label_to_block(func);
    let mut entries: HashMap<BlockId, usize> = HashMap::new();
    for block in func.blocks.values() {
        for target in block.terminator.successors() {
            *entries.entry(target).or_default() += 1;
        }
        for op in &block.ops {
            if crate::tir::dominators::is_exception_transfer_edge(op.opcode)
                && let Some(AttrValue::Int(label)) = op.attrs.get("value")
                && let Some(&target) = labels.get(label)
            {
                *entries.entry(target).or_default() += 1;
            }
        }
    }
    let mut regions: HashMap<ValueId, Option<BlockId>> = HashMap::new();
    for (&bid, block) in &func.blocks {
        let continuation = match &block.terminator {
            Terminator::CondBranch {
                cond,
                then_block,
                else_block,
                ..
            } if then_block != else_block
                && *else_block != bid
                && entries.get(else_block) == Some(&1) =>
            {
                Some((aliases.root(*cond), *else_block))
            }
            _ => None,
        };
        for op in &block.ops {
            for (index, &result) in op.results.iter().enumerate() {
                if !opcode_result_is_conditionally_valid_only_on_edge(op.opcode, index) {
                    continue;
                }
                let region = continuation.and_then(|(flag, region)| {
                    op.results
                        .iter()
                        .enumerate()
                        .any(|(other, &value)| other != index && aliases.root(value) == flag)
                        .then_some(region)
                });
                regions
                    .entry(aliases.root(result))
                    .and_modify(|known| {
                        if *known != region {
                            *known = None;
                        }
                    })
                    .or_insert(region);
            }
        }
    }
    regions
        .into_iter()
        .filter_map(|(root, region)| region.map(|region| (root, region)))
        .collect()
}

/// Entry-block argument roots that the activation owns when `owned`, and
/// borrows otherwise. Only `Transferred` custody hands the activation a
/// reference; every other declared custody leaves the parameter borrowed.
fn parameter_roots(func: &TirFunction, aliases: &AliasUnionFind, owned: bool) -> HashSet<ValueId> {
    func.blocks
        .get(&func.entry_block)
        .into_iter()
        .flat_map(|entry| entry.args.iter().enumerate())
        .filter(|&(position, _)| {
            (func.parameter_custody(position) == ParameterCustody::Transferred) == owned
        })
        .map(|(_, arg)| aliases.root(arg.id))
        .collect()
}

/// Whether `kind` is a binding-view spelling: a frame home store, cell store or
/// load, whose result is a view of the binding its home holds.
fn is_binding_view_kind(kind: &str) -> bool {
    crate::tir::op_kinds_generated::copy_kind_is_binding_view_table(kind)
}

/// Whether `op` is a `Copy` whose results hold no reference of their own: a
/// binding view by declaration, or a preserved spelling whose lowering neither
/// mints a fresh or aliased owner nor moves a no-heap value. These are the
/// frame binding views, the borrowed getters, the inert markers, and any kind
/// the classifier does not know, which fails closed.
fn copy_results_hold_no_reference(op: &TirOp) -> bool {
    if op.opcode != OpCode::Copy {
        return false;
    }
    let kind = original_kind(op);
    if kind.is_some_and(is_binding_view_kind) {
        return true;
    }
    let mints_owned = matches!(
        classify_copy_kind(kind),
        CopyLowering::OwnedValue | CopyLowering::OwnedAlias
    );
    !mints_owned && !copy_kind_is_explicit_no_heap_move(kind)
}

fn non_owning_copy_result_roots(func: &TirFunction, aliases: &AliasUnionFind) -> HashSet<ValueId> {
    func.blocks
        .values()
        .flat_map(|block| &block.ops)
        .filter(|op| copy_results_hold_no_reference(op))
        .flat_map(|op| op.results.iter().copied())
        .filter(|&result| aliases.root(result) == result)
        .collect()
}

/// Binding views, and the block arguments that carry only them.
///
/// A binding-view spelling returns a view of a frame binding: the object its
/// home holds, with no reference of its own, valid until the next write to its
/// slot. A block argument is a view too when every arc that binds it passes a
/// view: each terminator arc into its block, and each raising observation whose
/// handler it is. This is the greatest such set, so views joined around a loop
/// stay views. Any other input keeps the argument an owner, as the arc rules
/// already make it: an owned value moves in, and a raw carrier boxes on its arc,
/// where a full-range integer can allocate. A raw binding's own store view is a
/// view, so a local that starts as `0` or `None` still joins as one.
fn binding_view_roots(func: &TirFunction, aliases: &AliasUnionFind) -> HashSet<ValueId> {
    let mut views: HashSet<ValueId> = func
        .blocks
        .values()
        .flat_map(|block| &block.ops)
        .filter(|op| {
            op.opcode == OpCode::Copy && original_kind(op).is_some_and(is_binding_view_kind)
        })
        .flat_map(|op| op.results.iter().copied())
        .filter(|&result| aliases.root(result) == result)
        .collect();
    if views.is_empty() {
        return views;
    }
    // The root each binding arc passes to each block argument.
    let labels = crate::tir::dominators::exception_label_to_block(func);
    let mut inputs: HashMap<ValueId, Vec<ValueId>> = HashMap::new();
    let mut bind = |target: BlockId, values: &[ValueId]| {
        if let Some(block) = func.blocks.get(&target) {
            for (argument, &value) in block.args.iter().zip(values) {
                inputs
                    .entry(argument.id)
                    .or_default()
                    .push(aliases.root(value));
            }
        }
    };
    for block in func.blocks.values() {
        for op in &block.ops {
            if crate::tir::dominators::exception_edge_binds_handler_arguments(op.opcode)
                && let Some(AttrValue::Int(label)) = op.attrs.get("value")
                && let Some(&target) = labels.get(label)
            {
                bind(target, op.operands.as_slice());
            }
        }
        block.terminator.for_each_edge(&mut bind);
    }
    // The greatest fixpoint, in time linear in the bindings: an argument with
    // an input that is neither a view nor a candidate stops being a candidate,
    // and so does every argument it feeds.
    let mut candidates: HashSet<ValueId> = inputs.keys().copied().collect();
    let mut feeds: HashMap<ValueId, Vec<ValueId>> = HashMap::new();
    let mut pending: Vec<ValueId> = Vec::new();
    for (&argument, roots) in &inputs {
        for &root in roots {
            if candidates.contains(&root) {
                feeds.entry(root).or_default().push(argument);
            } else if !views.contains(&root) {
                pending.push(argument);
            }
        }
    }
    while let Some(argument) = pending.pop() {
        if candidates.remove(&argument) {
            pending.extend(feeds.get(&argument).into_iter().flatten().copied());
        }
    }
    views.extend(candidates);
    views
}

#[derive(Clone, Debug, Default)]
pub(crate) struct OwnershipRootFacts {
    borrowed_parameter_roots: HashSet<ValueId>,
    conditionally_valid_result_roots: HashSet<ValueId>,
    conditionally_valid_regions: HashMap<ValueId, BlockId>,
    non_owning_copy_result_roots: HashSet<ValueId>,
    /// Frame binding views and the block arguments that carry only them.
    binding_view_roots: HashSet<ValueId>,
}

impl OwnershipRootFacts {
    pub(crate) fn compute(func: &TirFunction, aliases: &AliasUnionFind) -> Self {
        Self {
            borrowed_parameter_roots: parameter_roots(func, aliases, false),
            conditionally_valid_result_roots: conditionally_valid_result_roots(func, aliases),
            conditionally_valid_regions: conditionally_valid_regions(func, aliases),
            non_owning_copy_result_roots: non_owning_copy_result_roots(func, aliases),
            binding_view_roots: binding_view_roots(func, aliases),
        }
    }

    pub(crate) fn is_borrowed_parameter_root(&self, root: ValueId) -> bool {
        self.borrowed_parameter_roots.contains(&root)
    }

    /// Alias roots whose result bits are valid only on a specific outgoing edge
    /// (currently the `IterNextUnboxed` value-out). These roots are never
    /// unconditionally droppable at joins or retained from the invalid edge.
    #[cfg(test)]
    pub(crate) fn conditionally_valid_result_roots(&self) -> &HashSet<ValueId> {
        &self.conditionally_valid_result_roots
    }

    pub(crate) fn is_conditionally_valid_result_root(&self, root: ValueId) -> bool {
        self.conditionally_valid_result_roots.contains(&root)
    }

    /// The block whose entry dominates every point where a conditionally-valid
    /// root is initialized: the sole successor of its producer's not-done
    /// edge. `None` keeps the root uninitialized at every point.
    pub(crate) fn conditionally_valid_region(&self, root: ValueId) -> Option<BlockId> {
        self.conditionally_valid_regions.get(&root).copied()
    }

    /// Self-rooting Copy-preserved result roots whose lowering does not mint an
    /// independent owned reference. Folded aliases stay governed by their source
    /// root; only a non-owning result that survives as its own root needs this
    /// fail-closed drop-eligibility fact.
    pub(crate) fn non_owning_copy_result_roots(&self) -> &HashSet<ValueId> {
        &self.non_owning_copy_result_roots
    }

    pub(crate) fn is_non_owning_copy_result_root(&self, root: ValueId) -> bool {
        self.non_owning_copy_result_roots.contains(&root)
    }

    /// Frame binding views, and the block arguments that carry only them: no
    /// reference of their own, valid until their slot's next write. Nothing
    /// releases one, and an operation that takes one receives a retained
    /// reference. A `Return` of one would need that retain after the frame's
    /// exit has released its home, so DropInsertion refuses it.
    pub(crate) fn is_binding_view_root(&self, root: ValueId) -> bool {
        self.binding_view_roots.contains(&root)
    }

    pub(crate) fn is_drop_owned_root_candidate(&self, root: ValueId) -> bool {
        !self.is_borrowed_parameter_root(root)
            && !self.is_non_owning_copy_result_root(root)
            && !self.is_binding_view_root(root)
    }

    /// Whether `result`, a result of `op`, holds a reference of its own: it is
    /// its own alias root, and `op` is not a `Copy` whose results hold none.
    /// This is [`Self::is_drop_owned_root_candidate`] for an operation's
    /// result, read from the operation alone, so a pass about to remove `op`
    /// (`Replacements`) keeps exactly the owner DropInsertion would have
    /// released without computing the whole function's facts. A result is no
    /// parameter, a view result is a non-owning copy result, and the view
    /// block arguments are no operation's results.
    pub(crate) fn result_holds_own_reference(
        op: &TirOp,
        result: ValueId,
        aliases: &AliasUnionFind,
    ) -> bool {
        aliases.root(result) == result && !copy_results_hold_no_reference(op)
    }
}

/// Drop eligibility over alias roots. This is the ownership-side predicate that
/// answers whether a value root carries a function-owned heap release obligation.
/// Raw-scalar production remains liveness/representation-owned; this struct only
/// consumes the already-computed raw carrier set so DropInsertion no longer owns
/// a parallel predicate.
pub(crate) struct DropEligibility<'a> {
    aliases: &'a AliasUnionFind,
    root_facts: &'a OwnershipRootFacts,
    raw_scalar_roots: HashSet<ValueId>,
}

impl<'a> DropEligibility<'a> {
    pub(crate) fn new(
        aliases: &'a AliasUnionFind,
        root_facts: &'a OwnershipRootFacts,
        raw_scalars: &HashSet<ValueId>,
    ) -> Self {
        Self {
            aliases,
            root_facts,
            raw_scalar_roots: raw_scalars
                .iter()
                .copied()
                .map(|value| aliases.root(value))
                .collect(),
        }
    }

    pub(crate) fn root(&self, value: ValueId) -> ValueId {
        self.aliases.root(value)
    }

    pub(crate) fn is_raw_scalar_root(&self, root: ValueId) -> bool {
        self.raw_scalar_roots.contains(&root)
    }

    pub(crate) fn is_conditionally_valid_result_root(&self, value: ValueId) -> bool {
        self.root_facts
            .is_conditionally_valid_result_root(self.root(value))
    }

    /// Whether returning `value` must publish a new owned result reference.
    ///
    /// A borrowed parameter enters `+0`, and transparent/non-owning copies
    /// preserve that borrow; a frame binding view, or a block argument
    /// carrying only views, holds no reference either. A Return transfers one
    /// owned result to the caller, so those roots require one retain at the
    /// callee boundary. Fresh and function-owned roots, a transferred
    /// parameter among them, already carry the transferable `+1`. The retain
    /// sits at the terminator, after frame exit, so it keeps only an object
    /// that outlives that exit: a return that frame teardown could invalidate
    /// returns the frontend's owned capture (`binding_alias`) taken before the
    /// teardown, which carries its own, and DropInsertion refuses a view that
    /// would need the retain. Raw and conditionally-valid carriers must never
    /// be retained here.
    pub(crate) fn return_requires_owned_publication(&self, value: ValueId) -> bool {
        let root = self.root(value);
        !self.is_raw_scalar_root(root)
            && !self.root_facts.is_conditionally_valid_result_root(root)
            && (self.root_facts.is_borrowed_parameter_root(root)
                || self.root_facts.is_non_owning_copy_result_root(root)
                || self.root_facts.is_binding_view_root(root))
    }

    pub(crate) fn is_droppable(&self, value: ValueId) -> bool {
        let root = self.root(value);
        root == value
            && !self.is_raw_scalar_root(root)
            && self.root_facts.is_drop_owned_root_candidate(root)
    }
}

/// Operand values whose held reference ends at this explicit release op.
/// Both point availability and whole-function lifetime facts consume the
/// generated release projection; placement owns neither a hand list nor a
/// second interpretation of DeleteVar's old-slot operand.
pub(crate) fn explicit_release_values(op: &TirOp) -> impl Iterator<Item = ValueId> + '_ {
    let operands = match opcode_explicit_release_operands_table(op.opcode, op.operands.len()) {
        ExplicitReleaseOperands::All => op.operands.as_slice(),
        ExplicitReleaseOperands::One(index) => op
            .operands
            .get(index)
            .map(std::slice::from_ref)
            .unwrap_or(&[]),
        ExplicitReleaseOperands::None => &[],
    };
    operands.iter().copied()
}

#[derive(Clone, Debug, Default)]
pub(crate) struct PythonLifetimeFacts {
    bound_local_roots: HashSet<ValueId>,
    local_store_roots: HashSet<ValueId>,
    named_slot_roots: HashSet<ValueId>,
    explicit_release_roots: HashSet<ValueId>,
    /// Source binding boundaries, captured before normalization to DecRef.
    /// A physical release alone proves a reference obligation, not a Python
    /// binding: exception MatchRefs and expression captures also have releases.
    binding_release_roots: HashSet<ValueId>,
    /// Parameters the activation owns by declaration: frame bindings from
    /// entry, released at their Python boundary like a local.
    parameter_binding_roots: HashSet<ValueId>,
}

impl PythonLifetimeFacts {
    pub(crate) fn compute(func: &TirFunction, aliases: &AliasUnionFind) -> Self {
        let mut facts = Self {
            parameter_binding_roots: parameter_roots(func, aliases, true),
            ..Self::default()
        };
        for block in func.blocks.values() {
            for op in &block.ops {
                if matches!(op.attrs.get("bound_local"), Some(AttrValue::Bool(true))) {
                    facts.bound_local_roots.extend(
                        op.results
                            .iter()
                            .copied()
                            .map(|result| aliases.root(result)),
                    );
                }

                if op.opcode == OpCode::Copy {
                    match original_kind(op) {
                        Some("store_var") => {
                            facts.local_store_roots.extend(
                                op.operands
                                    .iter()
                                    .chain(op.results.iter())
                                    .copied()
                                    .map(|value| aliases.root(value)),
                            );
                            facts.named_slot_roots.extend(
                                op.operands
                                    .iter()
                                    .chain(op.results.iter())
                                    .copied()
                                    .map(|value| aliases.root(value)),
                            );
                        }
                        Some("load_var") => {
                            facts.named_slot_roots.extend(
                                op.operands
                                    .iter()
                                    .chain(op.results.iter())
                                    .copied()
                                    .map(|value| aliases.root(value)),
                            );
                        }
                        _ => {}
                    }
                }

                // The generated operand projection owns which value a source
                // binding boundary releases. DecRef is the physical operation
                // used by every explicit reference lifetime, so it cannot by
                // itself establish source binding provenance.
                for root in explicit_release_values(op).map(|value| aliases.root(value)) {
                    facts.explicit_release_roots.insert(root);
                    if op.opcode != OpCode::DecRef {
                        facts.binding_release_roots.insert(root);
                    }
                }
            }
        }
        facts
    }

    /// Existing Python binding owners, independent of where they release.
    /// Explicit physical release keeps a reference to its boundary; it does not
    /// turn an expression or handler-control owner into a Python local name.
    /// Home stores end this provenance, including its canonical phi carriers.
    pub(crate) fn binding_custody_roots(
        &self,
        drop_eligibility: &DropEligibility<'_>,
        ownership_lattice: &OwnershipLattice,
    ) -> HashSet<ValueId> {
        self.bound_local_roots
            .iter()
            .copied()
            .chain(self.parameter_binding_roots.iter().copied())
            .chain(self.binding_release_roots.iter().copied())
            .chain(
                self.local_store_roots
                    .iter()
                    .copied()
                    .filter(|&root| ownership_lattice.is_finalizer_sensitive_root(root)),
            )
            .filter(|&root| drop_eligibility.is_droppable(root))
            .collect()
    }

    /// Normalization can delete a raw or conditionally-valid DelBoundary.
    /// Refresh only physical release obligations against that edited stream;
    /// retain the source binding provenance captured before normalization.
    pub(crate) fn refresh_explicit_release_roots(
        &mut self,
        func: &TirFunction,
        aliases: &AliasUnionFind,
    ) {
        self.explicit_release_roots = func
            .blocks
            .values()
            .flat_map(|block| &block.ops)
            .flat_map(explicit_release_values)
            .map(|value| aliases.root(value))
            .collect();
    }

    /// Failure context from these existing facts; diagnostic strings never
    /// establish custody, nor does an exception producer spelling.
    pub(crate) fn describe_binding_provenance(
        &self,
        root: ValueId,
        ownership_lattice: &OwnershipLattice,
    ) -> String {
        format!(
            "parameter={} bound_local={} local_store={} finalizer_sensitive={} source_binding_release={} explicit_release={}",
            self.parameter_binding_roots.contains(&root),
            self.bound_local_roots.contains(&root),
            self.local_store_roots.contains(&root),
            ownership_lattice.is_finalizer_sensitive_root(root),
            self.binding_release_roots.contains(&root),
            self.explicit_release_roots.contains(&root),
        )
    }

    /// Python-bound roots whose release DropInsertion places at the function
    /// boundary: local-slot owners and transferred parameters, minus explicit
    /// release boundaries. A transferred parameter is a frame binding by
    /// declaration. For a local store, positive `bound_local` provenance
    /// requires the Python owner lifetime even without a known finalizer:
    /// opaque results and mutable classes do not prove destruction
    /// unobservable. The existing finalizer closure also retains transported
    /// legacy slot owners. Unmarked, nonsensitive compiler temporaries stay out.
    pub(crate) fn boundary_release_roots(
        &self,
        drop_eligibility: &DropEligibility<'_>,
        ownership_lattice: &OwnershipLattice,
    ) -> HashSet<ValueId> {
        self.local_store_roots
            .iter()
            .copied()
            .filter(|root| {
                self.bound_local_roots.contains(root)
                    || ownership_lattice.is_finalizer_sensitive_root(*root)
            })
            .chain(self.parameter_binding_roots.iter().copied())
            .filter(|root| {
                drop_eligibility.is_droppable(*root)
                    && !self.has_explicit_release_boundary(*root)
                    && !drop_eligibility.is_conditionally_valid_result_root(*root)
            })
            .collect()
    }

    /// Finalizer-sensitive roots whose release can stay at the statement-local
    /// boundary. Local-store, parameter-binding and explicit-release roots
    /// already have Python lifetime boundaries, so DropInsertion must not place
    /// a second statement release for them.
    pub(crate) fn is_statement_release_boundary_root(
        &self,
        root: ValueId,
        drop_eligibility: &DropEligibility<'_>,
    ) -> bool {
        drop_eligibility.is_droppable(root)
            && !self.local_store_roots.contains(&root)
            && !self.parameter_binding_roots.contains(&root)
            && !self.has_explicit_release_boundary(root)
    }

    /// Python-bound roots that must be held until the dominated return boundary.
    /// Slot-backed locals keep their own
    /// rebinding/delete boundary and are not return-boundary deferrals.
    pub(crate) fn is_return_boundary_deferred_root(
        &self,
        root: ValueId,
        drop_eligibility: &DropEligibility<'_>,
    ) -> bool {
        self.bound_local_roots.contains(&root)
            && !self.named_slot_roots.contains(&root)
            && !self.has_explicit_release_boundary(root)
            && !drop_eligibility.is_conditionally_valid_result_root(root)
    }

    /// Owned named values eligible for the existing pure-SSA return planner.
    /// This is positive lexical ownership, never an inference from absent
    /// finalizer metadata. Placement still validates transfers and control flow.
    pub(crate) fn return_boundary_candidate_roots(
        &self,
        drop_eligibility: &DropEligibility<'_>,
    ) -> HashSet<ValueId> {
        self.bound_local_roots
            .iter()
            .copied()
            .filter(|root| {
                drop_eligibility.is_droppable(*root)
                    && self.is_return_boundary_deferred_root(*root, drop_eligibility)
            })
            .collect()
    }

    /// Canonical roots with an explicit Python release boundary. The lexical
    /// placement planner consumes this already-computed set unchanged.
    pub(crate) fn explicit_release_roots(&self) -> &HashSet<ValueId> {
        &self.explicit_release_roots
    }

    pub(crate) fn has_explicit_release_boundary(&self, root: ValueId) -> bool {
        self.explicit_release_roots.contains(&root)
    }
}

/// The minimal ownership-lattice slice for finalizer ordering (#58).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct StatementReleaseFinalizerBoundary {
    pub(crate) block: BlockId,
    pub(crate) op_index: usize,
    pub(crate) root: ValueId,
}

pub(crate) struct OwnershipLattice {
    finalizer_sensitive_roots: HashSet<ValueId>,
    statement_release_finalizer_boundaries: Vec<StatementReleaseFinalizerBoundary>,
}

impl OwnershipLattice {
    /// Compute the FinalizerSensitive set: every value whose release would
    /// (transitively) fire a `__del__`.
    pub(crate) fn compute(func: &TirFunction, aliases: &AliasUnionFind) -> Self {
        // Rung: seed with the direct finalizer-bearing allocations (already folded
        // across pure-move copies by `finalizer_alloc_roots`).
        let mut finalizer_sensitive_roots: HashSet<ValueId> = finalizer_alloc_roots(func)
            .into_iter()
            .map(|value| aliases.root(value))
            .collect();
        let mut statement_release_finalizer_boundaries = Vec::new();
        let mut statement_release_finalizer_boundary_keys = HashSet::new();
        if finalizer_sensitive_roots.is_empty() {
            return Self {
                finalizer_sensitive_roots,
                statement_release_finalizer_boundaries,
            };
        }
        // Rung: ownership-transfer closure. A container constructor that absorbs a
        // finalizer-sensitive element yields a finalizer-sensitive owner. Existing
        // container stores do the same for operand 0 while marking the producer
        // operand as absorbed at this statement. Forward fixpoint so an owner can
        // feed another (`[[A()]]`) or a later store.
        let mut changed = true;
        while changed {
            changed = false;
            for (&block_id, block) in &func.blocks {
                for (op_index, op) in block.ops.iter().enumerate() {
                    if op_result_absorbs_operand_ownership(op) {
                        let absorbed_sensitive: Vec<ValueId> = op
                            .operands
                            .iter()
                            .copied()
                            .map(|operand| aliases.root(operand))
                            .filter(|root| finalizer_sensitive_roots.contains(root))
                            .collect();
                        if !absorbed_sensitive.is_empty() {
                            for &absorbed in &absorbed_sensitive {
                                if statement_release_finalizer_boundary_keys
                                    .insert((block_id, op_index, absorbed))
                                {
                                    statement_release_finalizer_boundaries.push(
                                        StatementReleaseFinalizerBoundary {
                                            block: block_id,
                                            op_index,
                                            root: absorbed,
                                        },
                                    );
                                }
                            }
                            for &result in &op.results {
                                if finalizer_sensitive_roots.insert(aliases.root(result)) {
                                    changed = true;
                                }
                            }
                        }
                    }
                    if let Some(absorbed_idx) = op_container_absorbed_operand(op)
                        && let Some(&absorbed) = op.operands.get(absorbed_idx)
                    {
                        let absorbed_root = aliases.root(absorbed);
                        if !finalizer_sensitive_roots.contains(&absorbed_root) {
                            continue;
                        }
                        if statement_release_finalizer_boundary_keys.insert((
                            block_id,
                            op_index,
                            absorbed_root,
                        )) {
                            statement_release_finalizer_boundaries.push(
                                StatementReleaseFinalizerBoundary {
                                    block: block_id,
                                    op_index,
                                    root: absorbed_root,
                                },
                            );
                        }
                        if let Some(&owner) = op.operands.first()
                            && finalizer_sensitive_roots.insert(aliases.root(owner))
                        {
                            changed = true;
                        }
                    }
                    if let Some(source_idx) = op_result_finalizer_source_operand(op)
                        && let Some(&source) = op.operands.get(source_idx)
                    {
                        let source_root = aliases.root(source);
                        if finalizer_sensitive_roots.contains(&source_root) {
                            for &result in &op.results {
                                let result_root = aliases.root(result);
                                if finalizer_sensitive_roots.insert(result_root) {
                                    changed = true;
                                }
                            }
                        }
                    }
                }
            }
        }
        statement_release_finalizer_boundaries
            .sort_by_key(|boundary| (boundary.block.0, boundary.op_index, boundary.root.0));
        Self {
            finalizer_sensitive_roots,
            statement_release_finalizer_boundaries,
        }
    }

    /// True iff releasing `root` would (transitively) fire a `__del__`, so its
    /// release must land at the Python lifetime boundary, NOT its SSA last-use.
    pub(crate) fn is_finalizer_sensitive_root(&self, root: ValueId) -> bool {
        self.finalizer_sensitive_roots.contains(&root)
    }

    /// The full FinalizerSensitive set (the gate the ordering fix consumes).
    #[cfg(test)]
    pub(crate) fn finalizer_sensitive_roots(&self) -> &HashSet<ValueId> {
        &self.finalizer_sensitive_roots
    }

    pub fn statement_release_finalizer_boundaries(&self) -> &[StatementReleaseFinalizerBoundary] {
        &self.statement_release_finalizer_boundaries
    }
}

/// Sorted statement-boundary releases for finalizer-sensitive producer refs.
///
/// The ownership module owns the semantic composition: a FinalizerSensitive
/// absorption boundary only becomes a statement release when Python lifetime
/// facts say the root is not slot/local-boundary managed and DropEligibility
/// says the root carries a real heap release obligation. DropInsertion consumes
/// this plan and only materializes the DecRef placements.
#[derive(Clone, Debug, Default)]
pub(crate) struct StatementReleasePlan {
    after_op: HashMap<BlockId, HashMap<usize, Vec<ValueId>>>,
    released_roots: HashSet<ValueId>,
}

impl StatementReleasePlan {
    pub(crate) fn compute(
        lattice: &OwnershipLattice,
        python_lifetime_facts: &PythonLifetimeFacts,
        drop_eligibility: &DropEligibility<'_>,
    ) -> Self {
        let mut plan = Self::default();
        for boundary in lattice.statement_release_finalizer_boundaries() {
            let root = boundary.root;
            if !python_lifetime_facts.is_statement_release_boundary_root(root, drop_eligibility) {
                continue;
            }
            plan.after_op
                .entry(boundary.block)
                .or_default()
                .entry(boundary.op_index)
                .or_default()
                .push(root);
            plan.released_roots.insert(root);
        }
        for by_op in plan.after_op.values_mut() {
            for roots in by_op.values_mut() {
                roots.sort_unstable_by_key(|root| root.0);
                roots.dedup();
            }
        }
        plan
    }

    pub(crate) fn after_op(&self) -> &HashMap<BlockId, HashMap<usize, Vec<ValueId>>> {
        &self.after_op
    }

    pub(crate) fn contains_released_root(&self, root: ValueId) -> bool {
        self.released_roots.contains(&root)
    }
}

#[cfg(test)]
mod tests;
