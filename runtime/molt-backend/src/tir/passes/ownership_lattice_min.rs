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

use std::collections::HashSet;

use crate::tir::blocks::BlockId;
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    kind_container_absorbed_operand_table, kind_result_absorbs_operand_ownership_table,
    kind_result_finalizer_source_operand_table, opcode_container_absorbed_operand,
    opcode_result_absorbs_operand_ownership_table,
};
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::values::ValueId;

use super::escape_analysis::finalizer_alloc_roots;

fn original_kind(op: &TirOp) -> Option<&str> {
    match op.attrs.get("_original_kind") {
        Some(AttrValue::Str(kind)) => Some(kind.as_str()),
        _ => None,
    }
}

/// True when the result owns the operand lifetimes. This is generated fact-plane
/// authority, split by representation: first-class TIR opcodes read the opcode
/// table; Copy-preserved SimpleIR spellings read the `_original_kind` table.
fn op_result_absorbs_operand_ownership(op: &TirOp) -> bool {
    opcode_result_absorbs_operand_ownership_table(op.opcode)
        || (op.opcode == OpCode::Copy
            && original_kind(op).is_some_and(kind_result_absorbs_operand_ownership_table))
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

/// The minimal ownership-lattice slice for finalizer ordering (#58).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StatementReleaseFinalizerBoundary {
    pub block: BlockId,
    pub op_index: usize,
    pub value: ValueId,
}

pub struct OwnershipLattice {
    finalizer_sensitive: HashSet<ValueId>,
    statement_release_finalizer_values: HashSet<ValueId>,
    statement_release_finalizer_boundaries: Vec<StatementReleaseFinalizerBoundary>,
}

impl OwnershipLattice {
    /// Compute the FinalizerSensitive set: every value whose release would
    /// (transitively) fire a `__del__`.
    pub fn compute(func: &TirFunction) -> Self {
        // Rung: seed with the direct finalizer-bearing allocations (already folded
        // across pure-move copies by `finalizer_alloc_roots`).
        let mut finalizer_sensitive = finalizer_alloc_roots(func);
        let mut statement_release_finalizer_values = HashSet::new();
        let mut statement_release_finalizer_boundaries = Vec::new();
        let mut statement_release_finalizer_boundary_keys = HashSet::new();
        if finalizer_sensitive.is_empty() {
            return Self {
                finalizer_sensitive,
                statement_release_finalizer_values,
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
                            .filter(|operand| finalizer_sensitive.contains(operand))
                            .collect();
                        if !absorbed_sensitive.is_empty() {
                            statement_release_finalizer_values
                                .extend(absorbed_sensitive.iter().copied());
                            for &absorbed in &absorbed_sensitive {
                                if statement_release_finalizer_boundary_keys
                                    .insert((block_id, op_index, absorbed))
                                {
                                    statement_release_finalizer_boundaries.push(
                                        StatementReleaseFinalizerBoundary {
                                            block: block_id,
                                            op_index,
                                            value: absorbed,
                                        },
                                    );
                                }
                            }
                            for &result in &op.results {
                                if finalizer_sensitive.insert(result) {
                                    changed = true;
                                }
                            }
                        }
                    }
                    if let Some(absorbed_idx) = op_container_absorbed_operand(op)
                        && let Some(&absorbed) = op.operands.get(absorbed_idx)
                        && finalizer_sensitive.contains(&absorbed)
                    {
                        statement_release_finalizer_values.insert(absorbed);
                        if statement_release_finalizer_boundary_keys
                            .insert((block_id, op_index, absorbed))
                        {
                            statement_release_finalizer_boundaries.push(
                                StatementReleaseFinalizerBoundary {
                                    block: block_id,
                                    op_index,
                                    value: absorbed,
                                },
                            );
                        }
                        if let Some(&owner) = op.operands.first()
                            && finalizer_sensitive.insert(owner)
                        {
                            changed = true;
                        }
                    }
                    if let Some(source_idx) = op_result_finalizer_source_operand(op)
                        && let Some(&source) = op.operands.get(source_idx)
                        && finalizer_sensitive.contains(&source)
                    {
                        for &result in &op.results {
                            statement_release_finalizer_values.insert(result);
                            if finalizer_sensitive.insert(result) {
                                changed = true;
                            }
                        }
                    }
                }
            }
        }
        statement_release_finalizer_boundaries
            .sort_by_key(|boundary| (boundary.block.0, boundary.op_index, boundary.value.0));
        Self {
            finalizer_sensitive,
            statement_release_finalizer_values,
            statement_release_finalizer_boundaries,
        }
    }

    /// True iff releasing `value` would (transitively) fire a `__del__`, so its
    /// release must land at the Python lifetime boundary, NOT its SSA last-use.
    pub fn is_finalizer_sensitive(&self, value: ValueId) -> bool {
        self.finalizer_sensitive.contains(&value)
    }

    /// The full FinalizerSensitive set (the gate the ordering fix consumes).
    pub fn finalizer_sensitive_values(&self) -> &HashSet<ValueId> {
        &self.finalizer_sensitive
    }

    /// Finalizer-sensitive values whose own producer/extraction reference should
    /// release at the statement boundary unless Python-bound. This includes
    /// producer refs retained by a container owner and fresh extracted results
    /// such as discarded `list_pop`.
    pub fn statement_release_finalizer_values(&self) -> &HashSet<ValueId> {
        &self.statement_release_finalizer_values
    }

    pub fn statement_release_finalizer_boundaries(&self) -> &[StatementReleaseFinalizerBoundary] {
        &self.statement_release_finalizer_boundaries
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::Terminator;
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
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

    #[test]
    fn direct_finalizer_object_is_sensitive() {
        let mut f = func();
        let a = f.fresh_value();
        let entry = f.blocks.get_mut(&f.entry_block).unwrap();
        entry.ops.push(del_op(a));
        entry.terminator = Terminator::Return { values: vec![] };

        let lat = OwnershipLattice::compute(&f);
        assert!(lat.is_finalizer_sensitive(a));
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

        let lat = OwnershipLattice::compute(&f);
        assert!(
            lat.is_finalizer_sensitive(a),
            "the __del__ object is sensitive"
        );
        assert!(
            lat.is_finalizer_sensitive(list),
            "the list absorbing the __del__ object must be sensitive (#58 c_scope)"
        );
        assert!(
            lat.statement_release_finalizer_values().contains(&a),
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

        let lat = OwnershipLattice::compute(&f);
        assert!(lat.is_finalizer_sensitive(a));
        assert!(
            lat.is_finalizer_sensitive(list),
            "Copy-preserved list_new must absorb the __del__ object's lifetime"
        );
        assert!(
            lat.statement_release_finalizer_values().contains(&a),
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

        let lat = OwnershipLattice::compute(&f);
        assert!(lat.is_finalizer_sensitive(descriptor));
        assert!(
            lat.is_finalizer_sensitive(class_obj),
            "Copy-preserved class_def must keep class-body descriptor lifetime behind the class owner"
        );
        assert!(
            lat.statement_release_finalizer_values()
                .contains(&descriptor),
            "Copy-preserved class_def must mark the absorbed descriptor temp"
        );
        assert!(
            lat.statement_release_finalizer_boundaries()
                .iter()
                .any(|boundary| {
                    boundary.block == entry_id
                        && boundary.op_index == 1
                        && boundary.value == descriptor
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

        let lat = OwnershipLattice::compute(&f);
        assert!(
            lat.is_finalizer_sensitive(a),
            "defines_del call result is the owning finalizer root"
        );
        assert!(
            lat.is_finalizer_sensitive(list),
            "Copy-preserved list_new must absorb the call-created finalizer object"
        );
        assert!(
            lat.statement_release_finalizer_values().contains(&a),
            "call-created finalizer temp must release at the list_new boundary"
        );
    }

    #[test]
    fn list_append_absorbs_producer_into_existing_container() {
        let mut f = func();
        let list = f.fresh_value();
        let a = f.fresh_value();
        let entry = f.blocks.get_mut(&f.entry_block).unwrap();
        entry.ops.push(op(OpCode::BuildList, vec![], vec![list]));
        entry.ops.push(del_op(a));
        entry
            .ops
            .push(original_kind_copy("list_append", vec![list, a], vec![]));
        entry.terminator = Terminator::Return { values: vec![] };

        let lat = OwnershipLattice::compute(&f);
        assert!(lat.is_finalizer_sensitive(a));
        assert!(
            lat.is_finalizer_sensitive(list),
            "list_append must make the existing container finalizer-sensitive"
        );
        assert!(
            lat.statement_release_finalizer_values().contains(&a),
            "the appended producer temp has an absorption-boundary release fact"
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

        let lat = OwnershipLattice::compute(&f);
        assert!(lat.is_finalizer_sensitive(a));
        assert!(
            lat.is_finalizer_sensitive(list),
            "list_new keeps the finalizer-bearing element behind the list owner"
        );
        assert!(
            lat.is_finalizer_sensitive(module),
            "module storage now owns a finalizer-sensitive value"
        );
        assert!(
            lat.statement_release_finalizer_values().contains(&list),
            "module_set_attr must release the compiler-owned value ref at the storage boundary"
        );
        assert!(
            lat.statement_release_finalizer_boundaries()
                .iter()
                .any(|boundary| {
                    boundary.block == entry_id && boundary.op_index == 2 && boundary.value == list
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

        let lat = OwnershipLattice::compute(&f);
        assert!(lat.is_finalizer_sensitive(list));
        assert!(
            lat.is_finalizer_sensitive(popped),
            "list_pop result must inherit finalizer sensitivity from the source container"
        );
        assert!(
            lat.statement_release_finalizer_values().contains(&popped),
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

        let lat = OwnershipLattice::compute(&f);
        assert!(lat.finalizer_sensitive_values().is_empty());
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

        let lat = OwnershipLattice::compute(&f);
        assert!(lat.is_finalizer_sensitive(inner));
        assert!(lat.is_finalizer_sensitive(outer));
    }
}
