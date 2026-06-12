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
//!     through OWNERSHIP-TRANSFERRING ops (container constructors that absorb a
//!     finalizer-bearing element): releasing such a value fires a `__del__`.
//!   * python_lifetime_boundary / ordered_release_obligation — a FinalizerSensitive
//!     value's release must land at the Python boundary (scope exit / `del`), not
//!     SSA last-use. (Computed/consumed by the NEXT commit; see below.)
//!
//! STATUS — ACTIVE. DropInsertion consumes this lattice to extend a
//! FinalizerSensitive value's release to the Python lifetime boundary. Non-
//! finalizer values KEEP SSA-last-use release (no perf loss); the gate is
//! exactly this generated fact-plane set.

use std::collections::HashSet;

use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    kind_result_absorbs_operand_ownership_table, opcode_result_absorbs_operand_ownership_table,
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

/// The minimal ownership-lattice slice for finalizer ordering (#58).
pub struct OwnershipLattice {
    finalizer_sensitive: HashSet<ValueId>,
}

impl OwnershipLattice {
    /// Compute the FinalizerSensitive set: every value whose release would
    /// (transitively) fire a `__del__`.
    pub fn compute(func: &TirFunction) -> Self {
        // Rung: seed with the direct finalizer-bearing allocations (already folded
        // across pure-move copies by `finalizer_alloc_roots`).
        let mut finalizer_sensitive = finalizer_alloc_roots(func);
        if finalizer_sensitive.is_empty() {
            return Self {
                finalizer_sensitive,
            };
        }
        // Rung: ownership-transfer closure. A container constructor that absorbs a
        // finalizer-sensitive element yields a finalizer-sensitive result. Forward
        // fixpoint so a constructor can feed another (`[[A()]]`).
        let mut changed = true;
        while changed {
            changed = false;
            for block in func.blocks.values() {
                for op in &block.ops {
                    if !op_result_absorbs_operand_ownership(op) {
                        continue;
                    }
                    let absorbs_sensitive = op
                        .operands
                        .iter()
                        .any(|operand| finalizer_sensitive.contains(operand));
                    if absorbs_sensitive {
                        for &result in &op.results {
                            if finalizer_sensitive.insert(result) {
                                changed = true;
                            }
                        }
                    }
                }
            }
        }
        Self {
            finalizer_sensitive,
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
