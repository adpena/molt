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

/// The alias root of the operand whose ownership transfers into `op`, if any.
/// The consume signature is generated from `op_kinds.toml` instead of inferred by
/// the drop-placement pass: first-class opcodes read the opcode table, and
/// Copy-preserved SimpleIR spellings read the `_original_kind` table.
pub(crate) fn op_consumed_operand_root(
    op: &TirOp,
    canon: &dyn Fn(ValueId) -> ValueId,
) -> Option<ValueId> {
    let spelling_consumed =
        original_kind(op).and_then(|kind| kind_consumed_operand_table(kind, op.operands.len()));
    for idx in 0..op.operands.len() {
        let consumed = spelling_consumed == Some(idx)
            || opcode_operand_ownership_table(op.opcode, idx) == OperandOwnership::Consumed;
        if consumed {
            return op.operands.get(idx).copied().map(canon);
        }
    }
    None
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

fn parameter_roots(func: &TirFunction, aliases: &AliasUnionFind) -> HashSet<ValueId> {
    func.blocks
        .get(&func.entry_block)
        .into_iter()
        .flat_map(|entry| entry.args.iter())
        .map(|arg| aliases.root(arg.id))
        .collect()
}

fn produces_stack_value(opcode: OpCode) -> bool {
    matches!(opcode, OpCode::StackAlloc | OpCode::ObjectNewBoundStack)
}

fn stack_value_roots(func: &TirFunction, aliases: &AliasUnionFind) -> HashSet<ValueId> {
    let mut roots = HashSet::new();
    for block in func.blocks.values() {
        for op in &block.ops {
            if produces_stack_value(op.opcode) {
                roots.extend(
                    op.results
                        .iter()
                        .copied()
                        .map(|result| aliases.root(result)),
                );
            }
        }
    }
    roots
}

fn non_owning_copy_result_roots(func: &TirFunction, aliases: &AliasUnionFind) -> HashSet<ValueId> {
    let mut roots = HashSet::new();
    for block in func.blocks.values() {
        for op in &block.ops {
            if op.opcode != OpCode::Copy {
                continue;
            }
            let kind = original_kind(op);
            let mints_owned = matches!(
                classify_copy_kind(kind),
                CopyLowering::FreshValue | CopyLowering::OwnedAlias
            );
            let explicit_alias = copy_kind_is_explicit_no_heap_move(kind);
            if mints_owned || explicit_alias {
                continue;
            }
            for &result in &op.results {
                let root = aliases.root(result);
                if root == result {
                    roots.insert(root);
                }
            }
        }
    }
    roots
}

#[derive(Clone, Debug, Default)]
pub(crate) struct OwnershipRootFacts {
    borrowed_parameter_roots: HashSet<ValueId>,
    stack_value_roots: HashSet<ValueId>,
    conditionally_valid_result_roots: HashSet<ValueId>,
    non_owning_copy_result_roots: HashSet<ValueId>,
}

impl OwnershipRootFacts {
    pub(crate) fn compute(func: &TirFunction, aliases: &AliasUnionFind) -> Self {
        Self {
            borrowed_parameter_roots: parameter_roots(func, aliases),
            stack_value_roots: stack_value_roots(func, aliases),
            conditionally_valid_result_roots: conditionally_valid_result_roots(func, aliases),
            non_owning_copy_result_roots: non_owning_copy_result_roots(func, aliases),
        }
    }

    pub(crate) fn is_borrowed_parameter_root(&self, root: ValueId) -> bool {
        self.borrowed_parameter_roots.contains(&root)
    }

    pub(crate) fn is_stack_value_root(&self, root: ValueId) -> bool {
        self.stack_value_roots.contains(&root)
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

    pub(crate) fn is_drop_owned_root_candidate(&self, root: ValueId) -> bool {
        !self.is_borrowed_parameter_root(root)
            && !self.is_stack_value_root(root)
            && !self.is_non_owning_copy_result_root(root)
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
    /// Function parameters enter with the universal `+0` borrowed-argument
    /// contract, and transparent/non-owning copies preserve that borrow. A
    /// Return transfers one owned result to the caller, so those roots require
    /// one retain at the callee boundary. Fresh/function-owned roots already
    /// carry the transferable `+1`; raw, stack, and conditionally-valid carriers
    /// must never be retained here.
    pub(crate) fn return_requires_owned_publication(&self, value: ValueId) -> bool {
        let root = self.root(value);
        !self.is_raw_scalar_root(root)
            && !self.root_facts.is_stack_value_root(root)
            && !self.root_facts.is_conditionally_valid_result_root(root)
            && (self.root_facts.is_borrowed_parameter_root(root)
                || self.root_facts.is_non_owning_copy_result_root(root))
    }

    pub(crate) fn is_droppable(&self, value: ValueId) -> bool {
        let root = self.root(value);
        root == value
            && !self.is_raw_scalar_root(root)
            && self.root_facts.is_drop_owned_root_candidate(root)
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct PythonLifetimeFacts {
    bound_local_roots: HashSet<ValueId>,
    local_store_roots: HashSet<ValueId>,
    named_slot_roots: HashSet<ValueId>,
    explicit_release_roots: HashSet<ValueId>,
}

impl PythonLifetimeFacts {
    pub(crate) fn compute(func: &TirFunction, aliases: &AliasUnionFind) -> Self {
        let mut facts = Self::default();
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

                match opcode_explicit_release_operands_table(op.opcode, op.operands.len()) {
                    ExplicitReleaseOperands::All => {
                        facts.explicit_release_roots.extend(
                            op.operands
                                .iter()
                                .copied()
                                .map(|operand| aliases.root(operand)),
                        );
                    }
                    ExplicitReleaseOperands::One(idx) => {
                        if let Some(&released) = op.operands.get(idx) {
                            facts.explicit_release_roots.insert(aliases.root(released));
                        }
                    }
                    ExplicitReleaseOperands::None => {}
                }
            }
        }
        facts
    }

    /// Python-bound local-store roots whose release should be placed at the
    /// function boundary by DropInsertion. The lifetime fact is local-slot
    /// ownership minus explicit release boundaries, intersected with the
    /// finalizer-sensitive lattice: ordinary non-finalizer locals can release at
    /// SSA last use, while finalizer-sensitive locals preserve CPython-observable
    /// scope-exit ordering. DropInsertion owns only the eventual placement.
    pub(crate) fn boundary_release_roots(
        &self,
        drop_eligibility: &DropEligibility<'_>,
        ownership_lattice: &OwnershipLattice,
    ) -> HashSet<ValueId> {
        self.local_store_roots
            .iter()
            .copied()
            .filter(|root| {
                drop_eligibility.is_droppable(*root)
                    && ownership_lattice.is_finalizer_sensitive_root(*root)
                    && !self.has_explicit_release_boundary(*root)
                    && !drop_eligibility.is_conditionally_valid_result_root(*root)
            })
            .collect()
    }

    /// Finalizer-sensitive roots whose release can stay at the statement-local
    /// boundary. Local-store and explicit-release roots already have Python
    /// lifetime boundaries, so DropInsertion must not place a second statement
    /// release for them.
    pub(crate) fn is_statement_release_boundary_root(
        &self,
        root: ValueId,
        drop_eligibility: &DropEligibility<'_>,
    ) -> bool {
        drop_eligibility.is_droppable(root)
            && !self.local_store_roots.contains(&root)
            && !self.has_explicit_release_boundary(root)
    }

    /// Python-bound roots that must be held until the dominated return boundary
    /// when finalizer-sensitive. Slot-backed locals keep their own
    /// rebinding/delete boundary and are not return-boundary deferrals.
    pub(crate) fn is_return_boundary_deferred_root(
        &self,
        root: ValueId,
        drop_eligibility: &DropEligibility<'_>,
    ) -> bool {
        self.bound_local_roots.contains(&root)
            && !self.named_slot_roots.contains(&root)
            && !drop_eligibility.is_conditionally_valid_result_root(root)
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
    root_facts: OwnershipRootFacts,
    finalizer_sensitive_roots: HashSet<ValueId>,
    statement_release_finalizer_boundaries: Vec<StatementReleaseFinalizerBoundary>,
}

impl OwnershipLattice {
    /// Compute the FinalizerSensitive set: every value whose release would
    /// (transitively) fire a `__del__`.
    #[cfg(test)]
    pub(crate) fn compute(func: &TirFunction, aliases: &AliasUnionFind) -> Self {
        Self::compute_with_root_facts(func, aliases, OwnershipRootFacts::compute(func, aliases))
    }

    pub(crate) fn compute_with_root_facts(
        func: &TirFunction,
        aliases: &AliasUnionFind,
        root_facts: OwnershipRootFacts,
    ) -> Self {
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
                root_facts,
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
            root_facts,
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
    pub(crate) fn finalizer_sensitive_roots(&self) -> &HashSet<ValueId> {
        &self.finalizer_sensitive_roots
    }

    /// Alias roots whose result bits are valid only on a specific outgoing edge
    /// (currently the `IterNextUnboxed` value-out). These roots are never
    /// unconditionally droppable at joins or retained from the invalid edge.
    #[cfg(test)]
    pub(crate) fn conditionally_valid_result_roots(&self) -> &HashSet<ValueId> {
        self.root_facts.conditionally_valid_result_roots()
    }

    pub(crate) fn is_conditionally_valid_result_root(&self, root: ValueId) -> bool {
        self.root_facts.is_conditionally_valid_result_root(root)
    }

    #[cfg(test)]
    pub(crate) fn is_non_owning_copy_result_root(&self, root: ValueId) -> bool {
        self.root_facts.is_non_owning_copy_result_root(root)
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
