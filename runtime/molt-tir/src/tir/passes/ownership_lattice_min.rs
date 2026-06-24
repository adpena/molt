//! Ownership lattice — minimal slice (the #58 finalizer-ORDERING keystone).
//!
//! THE BUG (#58, doc 50 §A): a finalizer-sensitive value is released at its SSA
//! last-READ, not at its Python-visible lifetime boundary (`del` statement / scope
//! exit), so `__del__` fires too early. Repro `c_scope`:
//! ```python
//! def run():
//!     bag = [A()]        # A defines __del__; bag is never read again
//!     print("in run")    # CPython: __del__ runs AFTER this (scope exit)
//! ```
//! molt drops `bag` at its SSA last-use (the assignment) → the list → `A` → DEL
//! fires before `print`. CPython holds the local to frame teardown.
//!
//! THE FIX DIRECTION (council-binding, CLAUDE.md): a minimal OWNERSHIP LATTICE,
//! NOT another DropInsertion special-case. The rungs:
//!   * alias-root — the canonical owning value (rung 0; full alias unification is a
//!     later rung — here a value is its own root except across the pure-move copies
//!     `finalizer_alloc_roots` already folds).
//!   * **FinalizerSensitive** — the transitive closure of `finalizer_alloc_roots`
//!     through container owners: releasing such a value can fire a `__del__`.
//!   * **AbsorbedFinalizerProducer** — a finalizer-sensitive producer operand has
//!     been retained by a container owner at this statement. The producer's own
//!     caller ref can release at this absorption boundary; the container owner
//!     remains FinalizerSensitive until its Python lifetime boundary.
//!
//! STATUS — ACTIVE. DropInsertion consumes this lattice to extend a
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
/// / `terminator_operand_is_transferred`, design 27 §2.4). The ownership fact is
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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
    statement_release_finalizer_roots: HashSet<ValueId>,
    statement_release_finalizer_boundaries: Vec<StatementReleaseFinalizerBoundary>,
}

impl OwnershipLattice {
    /// Compute the FinalizerSensitive set: every value whose release would
    /// (transitively) fire a `__del__`.
    #[allow(dead_code)]
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
        let mut statement_release_finalizer_roots = HashSet::new();
        let mut statement_release_finalizer_boundaries = Vec::new();
        let mut statement_release_finalizer_boundary_keys = HashSet::new();
        if finalizer_sensitive_roots.is_empty() {
            return Self {
                root_facts,
                finalizer_sensitive_roots,
                statement_release_finalizer_roots,
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
                            statement_release_finalizer_roots
                                .extend(absorbed_sensitive.iter().copied());
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
                        statement_release_finalizer_roots.insert(absorbed_root);
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
                                statement_release_finalizer_roots.insert(result_root);
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
            statement_release_finalizer_roots,
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
    #[allow(dead_code)]
    pub(crate) fn conditionally_valid_result_roots(&self) -> &HashSet<ValueId> {
        self.root_facts.conditionally_valid_result_roots()
    }

    pub(crate) fn is_conditionally_valid_result_root(&self, root: ValueId) -> bool {
        self.root_facts.is_conditionally_valid_result_root(root)
    }

    /// Alias roots for `Copy` results that do not own an independent heap ref.
    #[allow(dead_code)]
    pub(crate) fn non_owning_copy_result_roots(&self) -> &HashSet<ValueId> {
        self.root_facts.non_owning_copy_result_roots()
    }

    #[allow(dead_code)]
    pub(crate) fn is_non_owning_copy_result_root(&self, root: ValueId) -> bool {
        self.root_facts.is_non_owning_copy_result_root(root)
    }

    /// Finalizer-sensitive values whose own producer/extraction reference should
    /// release at the statement boundary unless Python-bound. This includes
    /// producer refs retained by a container owner and fresh extracted results
    /// such as discarded `list_pop`.
    #[allow(dead_code)]
    pub(crate) fn statement_release_finalizer_roots(&self) -> &HashSet<ValueId> {
        &self.statement_release_finalizer_roots
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
mod tests {
    use super::*;
    use crate::tir::blocks::Terminator;
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::passes::alias_analysis::build_alias_union_find;
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;

    fn op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands,
            results,
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    fn del_op(result: ValueId) -> TirOp {
        let mut o = op(OpCode::ObjectNewBound, vec![], vec![result]);
        o.attrs.insert("defines_del".into(), AttrValue::Bool(true));
        o
    }

    fn del_call_bind(result: ValueId) -> TirOp {
        let mut o = op(OpCode::Call, vec![], vec![result]);
        o.attrs
            .insert("_original_kind".into(), AttrValue::Str("call_bind".into()));
        o.attrs.insert("defines_del".into(), AttrValue::Bool(true));
        o
    }

    fn original_kind_copy(kind: &str, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
        let mut o = op(OpCode::Copy, operands, results);
        o.attrs
            .insert("_original_kind".into(), AttrValue::Str(kind.into()));
        o
    }

    fn func() -> TirFunction {
        TirFunction::new("f".into(), vec![], TirType::None)
    }

    fn lattice(func: &TirFunction) -> OwnershipLattice {
        let aliases = build_alias_union_find(func);
        OwnershipLattice::compute(func, &aliases)
    }

    #[test]
    fn direct_finalizer_object_is_sensitive() {
        let mut f = func();
        let a = f.fresh_value();
        let entry = f.blocks.get_mut(&f.entry_block).unwrap();
        entry.ops.push(del_op(a));
        entry.terminator = Terminator::Return { values: vec![] };

        let lat = lattice(&f);
        assert!(lat.is_finalizer_sensitive_root(a));
    }

    #[test]
    fn iter_next_unboxed_value_result_root_is_conditionally_valid_without_finalizers() {
        let mut f = func();
        let iter = f.fresh_value();
        let value = f.fresh_value();
        let value_alias = f.fresh_value();
        let done = f.fresh_value();
        let entry = f.blocks.get_mut(&f.entry_block).unwrap();
        entry
            .ops
            .push(op(OpCode::IterNextUnboxed, vec![iter], vec![value, done]));
        let mut alias = op(OpCode::Copy, vec![value], vec![value_alias]);
        alias
            .attrs
            .insert("_original_kind".into(), AttrValue::Str("copy".into()));
        entry.ops.push(alias);
        entry.terminator = Terminator::Return { values: vec![] };

        let aliases = build_alias_union_find(&f);
        assert_eq!(
            aliases.root(value_alias),
            aliases.root(value),
            "the test fixture must prove conditional validity is stored per root"
        );
        let lat = OwnershipLattice::compute(&f, &aliases);
        assert!(
            lat.finalizer_sensitive_roots().is_empty(),
            "the conditional-validity fact must not depend on finalizer seeds"
        );
        assert!(
            lat.is_conditionally_valid_result_root(aliases.root(value)),
            "IterNextUnboxed result 0 root is valid only on the not-done edge"
        );
        assert!(
            lat.is_conditionally_valid_result_root(aliases.root(value_alias)),
            "transparent aliases of the value result share the conditional-validity root"
        );
        assert!(
            !lat.is_conditionally_valid_result_root(aliases.root(done)),
            "IterNextUnboxed result 1 is the done flag and is always valid"
        );
        assert_eq!(lat.conditionally_valid_result_roots().len(), 1);
    }

    #[test]
    fn exception_creation_ref_values_select_only_exception_creation_copies() {
        let mut f = func();
        let source = f.fresh_value();
        let creation = f.fresh_value();
        let plain_copy = f.fresh_value();
        let direct_exception = f.fresh_value();
        let entry = f.blocks.get_mut(&f.entry_block).unwrap();
        entry.ops.push(original_kind_copy(
            "exception_new_builtin_one",
            vec![source],
            vec![creation],
        ));
        entry
            .ops
            .push(original_kind_copy("list_new", vec![], vec![plain_copy]));
        entry
            .ops
            .push(op(OpCode::Call, vec![], vec![direct_exception]));
        entry.terminator = Terminator::Return { values: vec![] };

        let facts = exception_creation_ref_values(&f);
        assert!(facts.contains(&creation));
        assert!(!facts.contains(&plain_copy));
        assert!(!facts.contains(&direct_exception));
    }

    #[test]
    fn copy_transparent_alias_selects_only_single_operand_copy_aliases() {
        let mut f = func();
        let source = f.fresh_value();
        let alias_result = f.fresh_value();
        let extra = f.fresh_value();
        let fresh_result = f.fresh_value();

        let alias = original_kind_copy("copy_var", vec![source], vec![alias_result]);
        assert_eq!(
            copy_transparent_alias(&alias),
            Some(NoHeapCopyAlias {
                source,
                result: alias_result,
            })
        );

        let non_no_heap = original_kind_copy("list_new", vec![source], vec![fresh_result]);
        assert_eq!(copy_transparent_alias(&non_no_heap), None);

        let too_many_operands =
            original_kind_copy("copy_var", vec![source, extra], vec![alias_result]);
        assert_eq!(copy_transparent_alias(&too_many_operands), None);

        let too_many_results =
            original_kind_copy("copy_var", vec![source], vec![alias_result, extra]);
        assert_eq!(copy_transparent_alias(&too_many_results), None);

        let non_copy = op(OpCode::Call, vec![source], vec![alias_result]);
        assert_eq!(copy_transparent_alias(&non_copy), None);
    }

    #[test]
    fn non_owning_copy_result_roots_are_lattice_facts() {
        let mut f = func();
        let source = f.fresh_value();
        let explicit_alias = f.fresh_value();
        let unknown_passthrough = f.fresh_value();
        let bare_passthrough = f.fresh_value();
        let fresh = f.fresh_value();
        let owned_alias = f.fresh_value();
        let entry = f.blocks.get_mut(&f.entry_block).unwrap();
        entry
            .ops
            .push(op(OpCode::ObjectNewBound, vec![], vec![source]));
        entry.ops.push(original_kind_copy(
            "copy",
            vec![source],
            vec![explicit_alias],
        ));
        entry.ops.push(original_kind_copy(
            "not_registered_yet",
            vec![source],
            vec![unknown_passthrough],
        ));
        entry
            .ops
            .push(op(OpCode::Copy, vec![source], vec![bare_passthrough]));
        entry
            .ops
            .push(original_kind_copy("list_new", vec![source], vec![fresh]));
        entry.ops.push(original_kind_copy(
            "binding_alias",
            vec![source],
            vec![owned_alias],
        ));
        entry.terminator = Terminator::Return { values: vec![] };

        let aliases = build_alias_union_find(&f);
        assert_eq!(
            aliases.root(explicit_alias),
            aliases.root(source),
            "explicit no-heap moves must already share the source root"
        );
        assert_eq!(
            aliases.root(unknown_passthrough),
            unknown_passthrough,
            "unknown passthroughs remain independent roots and need the lattice fact"
        );
        assert_eq!(
            aliases.root(bare_passthrough),
            aliases.root(source),
            "bare Copy passthroughs are already folded aliases"
        );
        assert_eq!(
            aliases.root(owned_alias),
            owned_alias,
            "owned aliases keep a distinct ownership root"
        );
        let root_facts = OwnershipRootFacts::compute(&f, &aliases);
        assert!(
            root_facts.is_non_owning_copy_result_root(unknown_passthrough),
            "unknown Copy kinds fail closed as non-owning result roots"
        );
        assert!(
            !root_facts.is_non_owning_copy_result_root(aliases.root(bare_passthrough)),
            "folded bare Copy aliases must not mark their source root non-droppable"
        );
        assert!(
            !root_facts.is_non_owning_copy_result_root(aliases.root(explicit_alias)),
            "explicit aliases are handled by alias-root folding, not this root set"
        );
        assert!(
            !root_facts.is_non_owning_copy_result_root(fresh),
            "fresh-owned Copy results keep their independent drop obligation"
        );
        assert!(
            !root_facts.is_non_owning_copy_result_root(owned_alias),
            "owned alias Copy results keep their independent drop obligation"
        );
        let lat = OwnershipLattice::compute_with_root_facts(&f, &aliases, root_facts);
        assert!(
            lat.is_non_owning_copy_result_root(unknown_passthrough),
            "OwnershipLattice exposes the same root fact to placement"
        );
    }

    #[test]
    fn parameter_and_stack_roots_are_lattice_drop_eligibility_facts() {
        let mut f = TirFunction::new("param_stack".into(), vec![TirType::Str], TirType::None);
        let param = f.blocks[&f.entry_block].args[0].id;
        let stack = f.fresh_value();
        let heap = f.fresh_value();
        let entry = f.blocks.get_mut(&f.entry_block).unwrap();
        entry.ops.push(op(OpCode::StackAlloc, vec![], vec![stack]));
        entry
            .ops
            .push(op(OpCode::ObjectNewBound, vec![], vec![heap]));
        entry.terminator = Terminator::Return { values: vec![] };

        let aliases = build_alias_union_find(&f);
        let root_facts = OwnershipRootFacts::compute(&f, &aliases);
        assert!(
            root_facts.is_borrowed_parameter_root(param),
            "entry block args are borrowed from the caller"
        );
        assert!(
            root_facts.is_stack_value_root(stack),
            "StackAlloc results carry no RC obligation"
        );
        assert!(
            !root_facts.is_drop_owned_root_candidate(param),
            "borrowed parameter roots are not function-owned drop candidates"
        );
        assert!(
            !root_facts.is_drop_owned_root_candidate(stack),
            "stack roots are not function-owned drop candidates"
        );
        assert!(
            root_facts.is_drop_owned_root_candidate(heap),
            "ordinary heap roots remain function-owned drop candidates"
        );
    }

    #[test]
    fn drop_eligibility_combines_root_facts_and_raw_scalar_filter() {
        let mut f = TirFunction::new("drop_eligibility".into(), vec![TirType::Str], TirType::None);
        let param = f.blocks[&f.entry_block].args[0].id;
        let stack = f.fresh_value();
        let heap = f.fresh_value();
        let heap_alias = f.fresh_value();
        let raw = f.fresh_value();
        let unknown_passthrough = f.fresh_value();
        let entry = f.blocks.get_mut(&f.entry_block).unwrap();
        entry.ops.push(op(OpCode::StackAlloc, vec![], vec![stack]));
        entry
            .ops
            .push(op(OpCode::ObjectNewBound, vec![], vec![heap]));
        entry
            .ops
            .push(original_kind_copy("copy", vec![heap], vec![heap_alias]));
        entry.ops.push(op(OpCode::ConstInt, vec![], vec![raw]));
        entry.ops.push(original_kind_copy(
            "not_registered_yet",
            vec![heap],
            vec![unknown_passthrough],
        ));
        entry.terminator = Terminator::Return { values: vec![] };

        let aliases = build_alias_union_find(&f);
        let root_facts = OwnershipRootFacts::compute(&f, &aliases);
        let raw_scalars = std::collections::HashSet::from([raw]);
        let eligibility = DropEligibility::new(&aliases, &root_facts, &raw_scalars);
        assert!(
            eligibility.is_droppable(heap),
            "ordinary heap roots are droppable"
        );
        assert!(
            !eligibility.is_droppable(param),
            "borrowed parameter roots are not droppable"
        );
        assert!(
            !eligibility.is_droppable(stack),
            "stack roots carry no RC obligation"
        );
        assert!(
            !eligibility.is_droppable(raw),
            "raw scalar roots carry no heap release obligation"
        );
        assert!(
            !eligibility.is_droppable(heap_alias),
            "transparent aliases are not independently droppable"
        );
        assert!(
            !eligibility.is_droppable(unknown_passthrough),
            "self-rooting unknown Copy results fail closed as non-droppable"
        );
    }

    #[test]
    fn python_lifetime_facts_track_bound_slots_and_explicit_releases() {
        let mut f = func();
        let bound = f.fresh_value();
        let stored = f.fresh_value();
        let loaded = f.fresh_value();
        let explicit = f.fresh_value();
        let missing = f.fresh_value();
        let deleted = f.fresh_value();
        let boundary = f.fresh_value();
        let boundary_stored = f.fresh_value();
        let stack = f.fresh_value();
        let stack_stored = f.fresh_value();
        let statement = f.fresh_value();
        let deferred = f.fresh_value();
        let iterator = f.fresh_value();
        let conditional_value = f.fresh_value();
        let conditional_done = f.fresh_value();
        let conditional_stored = f.fresh_value();
        let entry = f.blocks.get_mut(&f.entry_block).unwrap();
        let mut bound_op = op(OpCode::ObjectNewBound, vec![], vec![bound]);
        bound_op
            .attrs
            .insert("bound_local".into(), AttrValue::Bool(true));
        entry.ops.push(bound_op);
        entry
            .ops
            .push(original_kind_copy("store_var", vec![bound], vec![stored]));
        entry
            .ops
            .push(original_kind_copy("load_var", vec![stored], vec![loaded]));
        entry.ops.push(op(OpCode::DecRef, vec![explicit], vec![]));
        entry
            .ops
            .push(op(OpCode::DeleteVar, vec![missing, loaded], vec![deleted]));
        entry.ops.push(del_op(boundary));
        entry.ops.push(original_kind_copy(
            "store_var",
            vec![boundary],
            vec![boundary_stored],
        ));
        entry.ops.push(op(OpCode::StackAlloc, vec![], vec![stack]));
        entry.ops.push(original_kind_copy(
            "store_var",
            vec![stack],
            vec![stack_stored],
        ));
        entry
            .ops
            .push(op(OpCode::ObjectNewBound, vec![], vec![statement]));
        let mut deferred_op = op(OpCode::ObjectNewBound, vec![], vec![deferred]);
        deferred_op
            .attrs
            .insert("bound_local".into(), AttrValue::Bool(true));
        entry.ops.push(deferred_op);
        let mut iter_next = op(
            OpCode::IterNextUnboxed,
            vec![iterator],
            vec![conditional_value, conditional_done],
        );
        iter_next
            .attrs
            .insert("bound_local".into(), AttrValue::Bool(true));
        entry.ops.push(iter_next);
        entry.ops.push(original_kind_copy(
            "store_var",
            vec![conditional_value],
            vec![conditional_stored],
        ));
        entry.terminator = Terminator::Return { values: vec![] };

        let aliases = build_alias_union_find(&f);
        let facts = PythonLifetimeFacts::compute(&f, &aliases);
        let bound_root = aliases.root(bound);
        let loaded_root = aliases.root(loaded);
        let boundary_root = aliases.root(boundary);
        let stack_root = aliases.root(stack);
        let statement_root = aliases.root(statement);
        let deferred_root = aliases.root(deferred);
        let conditional_root = aliases.root(conditional_value);
        assert!(
            facts.has_explicit_release_boundary(aliases.root(explicit))
                && facts.has_explicit_release_boundary(loaded_root),
            "DecRef and DeleteVar old-slot operands are explicit release roots"
        );
        let root_facts = OwnershipRootFacts::compute(&f, &aliases);
        let drop_eligibility = DropEligibility::new(&aliases, &root_facts, &HashSet::new());
        assert!(
            facts.is_statement_release_boundary_root(statement_root, &drop_eligibility),
            "droppable non-slot roots can release at statement finalizer boundaries"
        );
        assert!(
            !facts.is_statement_release_boundary_root(boundary_root, &drop_eligibility),
            "local-store roots defer to the Python boundary instead of statement release"
        );
        assert!(
            !facts.is_statement_release_boundary_root(bound_root, &drop_eligibility),
            "explicit release roots do not receive a second statement release"
        );
        assert!(
            !facts.is_statement_release_boundary_root(stack_root, &drop_eligibility),
            "stack/no-RC roots are not statement release roots"
        );
        assert!(
            facts.is_return_boundary_deferred_root(deferred_root, &drop_eligibility),
            "bound_local attrs define Python return-boundary deferral roots"
        );
        assert!(
            !facts.is_return_boundary_deferred_root(bound_root, &drop_eligibility)
                && !facts.is_return_boundary_deferred_root(loaded_root, &drop_eligibility),
            "slot-backed local roots keep their del/rebinding boundary"
        );
        assert!(
            !facts.is_return_boundary_deferred_root(conditional_root, &drop_eligibility),
            "conditionally-valid results are not total definitions and cannot defer to an unconditional return boundary"
        );
        let lat = OwnershipLattice::compute(&f, &aliases);
        let boundary_roots = facts.boundary_release_roots(&drop_eligibility, &lat);
        assert!(
            boundary_roots.contains(&boundary_root),
            "droppable local-store roots are Python boundary release roots"
        );
        assert!(
            !boundary_roots.contains(&bound_root),
            "explicitly released local-store roots must not get a second boundary release"
        );
        assert!(
            !boundary_roots.contains(&stack_root),
            "stack/no-RC local-store roots are not boundary release roots"
        );
        assert!(
            !boundary_roots.contains(&conditional_root),
            "conditionally-valid local-store roots must release only on valid paths"
        );
    }

    #[test]
    fn statement_release_plan_filters_and_sorts_boundary_roots() {
        let mut f = func();
        let statement_list = f.fresh_value();
        let statement = f.fresh_value();
        let local_list = f.fresh_value();
        let local = f.fresh_value();
        let local_slot = f.fresh_value();
        let explicit_list = f.fresh_value();
        let explicit = f.fresh_value();
        let entry_id = f.entry_block;
        let entry = f.blocks.get_mut(&entry_id).unwrap();
        entry
            .ops
            .push(op(OpCode::BuildList, vec![], vec![statement_list]));
        entry.ops.push(del_op(statement));
        entry.ops.push(original_kind_copy(
            "list_append",
            vec![statement_list, statement],
            vec![],
        ));
        entry
            .ops
            .push(op(OpCode::BuildList, vec![], vec![local_list]));
        entry.ops.push(del_op(local));
        entry.ops.push(original_kind_copy(
            "store_var",
            vec![local],
            vec![local_slot],
        ));
        entry.ops.push(original_kind_copy(
            "list_append",
            vec![local_list, local],
            vec![],
        ));
        entry
            .ops
            .push(op(OpCode::BuildList, vec![], vec![explicit_list]));
        entry.ops.push(del_op(explicit));
        entry.ops.push(op(OpCode::DecRef, vec![explicit], vec![]));
        entry.ops.push(original_kind_copy(
            "list_append",
            vec![explicit_list, explicit],
            vec![],
        ));
        entry.terminator = Terminator::Return { values: vec![] };

        let aliases = build_alias_union_find(&f);
        let root_facts = OwnershipRootFacts::compute(&f, &aliases);
        let lattice = OwnershipLattice::compute_with_root_facts(&f, &aliases, root_facts.clone());
        let lifetime_facts = PythonLifetimeFacts::compute(&f, &aliases);
        let drop_eligibility = DropEligibility::new(&aliases, &root_facts, &HashSet::new());
        let plan = StatementReleasePlan::compute(&lattice, &lifetime_facts, &drop_eligibility);
        let statement_root = aliases.root(statement);
        let local_root = aliases.root(local);
        let explicit_root = aliases.root(explicit);

        assert!(
            plan.contains_released_root(statement_root),
            "ordinary finalizer producer temps release at their storage statement"
        );
        assert!(
            !plan.contains_released_root(local_root),
            "slot/local-managed roots defer to their Python lifetime boundary"
        );
        assert!(
            !plan.contains_released_root(explicit_root),
            "explicit DecRef roots do not receive a second statement release"
        );
        assert_eq!(
            plan.after_op()
                .get(&entry_id)
                .and_then(|by_op| by_op.get(&2))
                .cloned(),
            Some(vec![statement_root]),
            "the release plan stores sorted root-space releases by exact op boundary"
        );
    }

    #[test]
    fn container_absorbing_finalizer_object_is_sensitive() {
        // The c_scope shape: `bag = [A()]` -> BuildList absorbs the __del__ object,
        // so the list value must also be finalizer-sensitive (releasing it fires A).
        let mut f = func();
        let a = f.fresh_value();
        let list = f.fresh_value();
        let entry = f.blocks.get_mut(&f.entry_block).unwrap();
        entry.ops.push(del_op(a));
        entry.ops.push(op(OpCode::BuildList, vec![a], vec![list]));
        entry.terminator = Terminator::Return { values: vec![] };

        let lat = lattice(&f);
        assert!(
            lat.is_finalizer_sensitive_root(a),
            "the __del__ object is sensitive"
        );
        assert!(
            lat.is_finalizer_sensitive_root(list),
            "the list absorbing the __del__ object must be sensitive (#58 c_scope)"
        );
        assert!(
            lat.statement_release_finalizer_roots().contains(&a),
            "the producer temp has a separate absorption-boundary release fact"
        );
    }

    #[test]
    fn copy_list_new_absorbing_finalizer_object_is_sensitive() {
        // Real SimpleIR lowering preserves `list_new` as Copy{_original_kind}
        // rather than canonicalizing it to BuildList. The generated
        // result-absorption fact must cover that spelling without aliasing it.
        let mut f = func();
        let a = f.fresh_value();
        let list = f.fresh_value();
        let entry = f.blocks.get_mut(&f.entry_block).unwrap();
        entry.ops.push(del_op(a));
        entry
            .ops
            .push(original_kind_copy("list_new", vec![a], vec![list]));
        entry.terminator = Terminator::Return { values: vec![] };

        let lat = lattice(&f);
        assert!(lat.is_finalizer_sensitive_root(a));
        assert!(
            lat.is_finalizer_sensitive_root(list),
            "Copy-preserved list_new must absorb the __del__ object's lifetime"
        );
        assert!(
            lat.statement_release_finalizer_roots().contains(&a),
            "Copy-preserved list_new must mark the absorbed producer"
        );
    }

    #[test]
    fn copy_class_def_absorbs_descriptor_into_class_owner() {
        let mut f = func();
        let name = f.fresh_value();
        let descriptor = f.fresh_value();
        let class_obj = f.fresh_value();
        let entry_id = f.entry_block;
        let entry = f.blocks.get_mut(&entry_id).unwrap();
        entry.ops.push(del_op(descriptor));
        entry.ops.push(original_kind_copy(
            "class_def",
            vec![name, descriptor],
            vec![class_obj],
        ));
        entry.terminator = Terminator::Return { values: vec![] };

        let aliases = build_alias_union_find(&f);
        let descriptor_root = aliases.root(descriptor);
        let class_obj_root = aliases.root(class_obj);
        let lat = OwnershipLattice::compute(&f, &aliases);
        assert!(lat.is_finalizer_sensitive_root(descriptor_root));
        assert!(
            lat.is_finalizer_sensitive_root(class_obj_root),
            "Copy-preserved class_def must keep class-body descriptor lifetime behind the class owner"
        );
        assert!(
            lat.statement_release_finalizer_roots()
                .contains(&descriptor_root),
            "Copy-preserved class_def must mark the absorbed descriptor temp"
        );
        assert!(
            lat.statement_release_finalizer_boundaries()
                .iter()
                .any(|boundary| {
                    boundary.block == entry_id
                        && boundary.op_index == 1
                        && boundary.root == descriptor_root
                }),
            "class_def must expose the exact class-construction absorption boundary"
        );
    }

    #[test]
    fn call_bind_defines_del_into_list_new_is_sensitive() {
        // Finalizer classes decline OBJECT_NEW_BOUND constructor folding, so the
        // real frontend shape is CALL_BIND(class_ref, callargs) -> list_new.
        let mut f = func();
        let a = f.fresh_value();
        let list = f.fresh_value();
        let entry = f.blocks.get_mut(&f.entry_block).unwrap();
        entry.ops.push(del_call_bind(a));
        entry
            .ops
            .push(original_kind_copy("list_new", vec![a], vec![list]));
        entry.terminator = Terminator::Return { values: vec![] };

        let lat = lattice(&f);
        assert!(
            lat.is_finalizer_sensitive_root(a),
            "defines_del call result is the owning finalizer root"
        );
        assert!(
            lat.is_finalizer_sensitive_root(list),
            "Copy-preserved list_new must absorb the call-created finalizer object"
        );
        assert!(
            lat.statement_release_finalizer_roots().contains(&a),
            "call-created finalizer temp must release at the list_new boundary"
        );
    }

    #[test]
    fn list_append_absorbs_producer_into_existing_container() {
        let mut f = func();
        let list = f.fresh_value();
        let a = f.fresh_value();
        let a_alias = f.fresh_value();
        let entry = f.blocks.get_mut(&f.entry_block).unwrap();
        entry.ops.push(op(OpCode::BuildList, vec![], vec![list]));
        entry.ops.push(del_op(a));
        entry
            .ops
            .push(original_kind_copy("copy", vec![a], vec![a_alias]));
        entry.ops.push(original_kind_copy(
            "list_append",
            vec![list, a_alias],
            vec![],
        ));
        entry.terminator = Terminator::Return { values: vec![] };

        let aliases = build_alias_union_find(&f);
        assert_eq!(
            aliases.root(a_alias),
            aliases.root(a),
            "the test fixture must prove finalizer facts are stored per root"
        );
        let lat = OwnershipLattice::compute(&f, &aliases);
        let a_root = aliases.root(a);
        assert!(
            lat.is_finalizer_sensitive_root(a_root),
            "the finalizer-sensitive producer root survives transparent aliases"
        );
        assert!(
            lat.is_finalizer_sensitive_root(list),
            "list_append must make the existing container finalizer-sensitive"
        );
        assert!(
            lat.statement_release_finalizer_roots().contains(&a_root),
            "the appended producer temp has an absorption-boundary release fact"
        );
        assert!(
            lat.statement_release_finalizer_boundaries()
                .iter()
                .any(|boundary| boundary.op_index == 3 && boundary.root == a_root),
            "list_append must expose the exact absorbed producer root boundary"
        );
    }

    #[test]
    fn module_set_attr_absorbs_value_into_module_storage() {
        let mut f = func();
        let module = f.fresh_value();
        let name = f.fresh_value();
        let a = f.fresh_value();
        let list = f.fresh_value();
        let entry_id = f.entry_block;
        let entry = f.blocks.get_mut(&entry_id).unwrap();
        entry.ops.push(del_op(a));
        entry
            .ops
            .push(original_kind_copy("list_new", vec![a], vec![list]));
        entry
            .ops
            .push(op(OpCode::ModuleSetAttr, vec![module, name, list], vec![]));
        entry.terminator = Terminator::Return { values: vec![] };

        let lat = lattice(&f);
        assert!(lat.is_finalizer_sensitive_root(a));
        assert!(
            lat.is_finalizer_sensitive_root(list),
            "list_new keeps the finalizer-bearing element behind the list owner"
        );
        assert!(
            lat.is_finalizer_sensitive_root(module),
            "module storage now owns a finalizer-sensitive value"
        );
        assert!(
            lat.statement_release_finalizer_roots().contains(&list),
            "module_set_attr must release the compiler-owned value ref at the storage boundary"
        );
        assert!(
            lat.statement_release_finalizer_boundaries()
                .iter()
                .any(|boundary| {
                    boundary.block == entry_id && boundary.op_index == 2 && boundary.root == list
                }),
            "module_set_attr must expose the exact storage absorption boundary"
        );
    }

    #[test]
    fn list_pop_result_inherits_finalizer_sensitivity_from_container() {
        let mut f = func();
        let a = f.fresh_value();
        let list = f.fresh_value();
        let popped = f.fresh_value();
        let entry = f.blocks.get_mut(&f.entry_block).unwrap();
        entry.ops.push(del_op(a));
        entry
            .ops
            .push(original_kind_copy("list_new", vec![a], vec![list]));
        entry
            .ops
            .push(original_kind_copy("list_pop", vec![list], vec![popped]));
        entry.terminator = Terminator::Return { values: vec![] };

        let lat = lattice(&f);
        assert!(lat.is_finalizer_sensitive_root(list));
        assert!(
            lat.is_finalizer_sensitive_root(popped),
            "list_pop result must inherit finalizer sensitivity from the source container"
        );
        assert!(
            lat.statement_release_finalizer_roots().contains(&popped),
            "discarded pop result is a statement-release temporary unless Python-bound"
        );
    }

    #[test]
    fn non_finalizer_function_has_empty_set() {
        let mut f = func();
        let a = f.fresh_value();
        let list = f.fresh_value();
        let entry = f.blocks.get_mut(&f.entry_block).unwrap();
        // A plain object with no __del__ + a list of it: nothing is sensitive.
        entry.ops.push(op(OpCode::ObjectNewBound, vec![], vec![a]));
        entry.ops.push(op(OpCode::BuildList, vec![a], vec![list]));
        entry.terminator = Terminator::Return { values: vec![] };

        let lat = lattice(&f);
        assert!(lat.finalizer_sensitive_roots().is_empty());
    }

    #[test]
    fn nested_container_propagates() {
        // `[[A()]]` — the inner and outer list are both sensitive (fixpoint).
        let mut f = func();
        let a = f.fresh_value();
        let inner = f.fresh_value();
        let outer = f.fresh_value();
        let entry = f.blocks.get_mut(&f.entry_block).unwrap();
        entry.ops.push(del_op(a));
        entry.ops.push(op(OpCode::BuildList, vec![a], vec![inner]));
        entry
            .ops
            .push(op(OpCode::BuildList, vec![inner], vec![outer]));
        entry.terminator = Terminator::Return { values: vec![] };

        let lat = lattice(&f);
        assert!(lat.is_finalizer_sensitive_root(inner));
        assert!(lat.is_finalizer_sensitive_root(outer));
    }
}
