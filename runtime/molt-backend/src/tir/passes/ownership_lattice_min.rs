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
//! STATUS — INERT. This commit computes the FinalizerSensitive set (the rung the
//! release-ordering fix consumes) and unit-tests it. NO backend yet consults it, so
//! codegen is byte-identical. The behaviour flip — extending a FinalizerSensitive
//! value's `last_use`/release to the function's `Terminator::Return` boundary, in the
//! shared `liveness`/value-tracking path so BOTH the dormant-native value-tracking
//! and `drop_insertion` honor it — is the next commit (Commit 3). Non-finalizer
//! values KEEP SSA-last-use release (no perf loss); the gate is exactly this set.

use std::collections::HashSet;

use crate::tir::function::TirFunction;
use crate::tir::ops::OpCode;
use crate::tir::values::ValueId;

use super::escape_analysis::finalizer_alloc_roots;

/// A constructor whose RESULT takes ownership of its element operands — releasing
/// the result releases the elements. So a finalizer-sensitive element makes the
/// result finalizer-sensitive (the release of the container is what fires the
/// element's `__del__`). Mutation absorption (`StoreAttr`/`StoreIndex`/`list.append`)
/// is a later rung — `c_scope` is pure construction (`[A()]` = `BuildList`).
fn is_absorbing_constructor(opcode: OpCode) -> bool {
    matches!(
        opcode,
        OpCode::BuildList | OpCode::BuildTuple | OpCode::BuildDict | OpCode::BuildSet
    )
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
                    if !is_absorbing_constructor(op.opcode) {
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
    use crate::tir::blocks::{TirBlock, Terminator};
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
        assert!(lat.is_finalizer_sensitive(a), "the __del__ object is sensitive");
        assert!(
            lat.is_finalizer_sensitive(list),
            "the list absorbing the __del__ object must be sensitive (#58 c_scope)"
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
        entry.ops.push(op(OpCode::BuildList, vec![inner], vec![outer]));
        entry.terminator = Terminator::Return { values: vec![] };

        let lat = OwnershipLattice::compute(&f);
        assert!(lat.is_finalizer_sensitive(inner));
        assert!(lat.is_finalizer_sensitive(outer));
    }
}
