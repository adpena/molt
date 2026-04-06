//! Bounds Check Elimination (BCE) pass for TIR.
//!
//! Annotates `Index` operations that are provably bounds-check-safe by adding
//! a `"bce_safe"` attribute (set to `AttrValue::Bool(true)`).  Downstream
//! codegen can test for this attribute and skip the runtime bounds check.
//!
//! ## Current scope (Phase 2 — constant-index BCE)
//!
//! An `Index` op is marked safe when **all** of the following hold:
//!   1. The index operand was produced by a `ConstInt` operation.
//!   2. The constant value is **non-negative** (i.e. `value >= 0`).
//!
//! Negative constant indices still require a runtime wraparound (Python
//! semantics `lst[-1]`), so they are intentionally left unmarked.
//!
//! Non-constant indices are left for a future range-analysis phase.

use std::collections::HashMap;

use crate::tir::blocks::BlockId;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode};
use crate::tir::values::ValueId;

use super::PassStats;

// ---------------------------------------------------------------------------
// Pass implementation
// ---------------------------------------------------------------------------

/// Bounds Check Elimination pass.
///
/// Scans every `Index` op in `func`.  When the index operand is defined by a
/// `ConstInt` with a non-negative value, the op is annotated with
/// `bce_safe = true` so that codegen can elide the bounds check.
///
/// Returns [`PassStats`] describing how many ops were annotated
/// (`values_changed`).
pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "bce",
        ..Default::default()
    };

    // --- Phase 1: build a map from ValueId → constant integer value ---
    //
    // Walk every block once and record `ConstInt` results.  This is O(N) in
    // the total number of ops across all blocks.
    let mut const_int_value: HashMap<ValueId, i64> = HashMap::new();

    // Collect block ids first to avoid a long borrow on `func.blocks`.
    let block_ids: Vec<BlockId> = func.blocks.keys().copied().collect();

    for bid in &block_ids {
        if let Some(block) = func.blocks.get(bid) {
            for op in &block.ops {
                if op.opcode == OpCode::ConstInt
                    && let Some(AttrValue::Int(v)) = op.attrs.get("value")
                {
                    for &result in &op.results {
                        const_int_value.insert(result, *v);
                    }
                }
            }
        }
    }

    // --- Phase 2: annotate Index ops whose index is a non-negative constant ---
    //
    // An `Index` op has two operands: [container, index].
    // Only the index (operand[1]) is relevant for bounds-check elimination.
    for bid in &block_ids {
        if let Some(block) = func.blocks.get_mut(bid) {
            for op in block.ops.iter_mut() {
                if op.opcode != OpCode::Index {
                    continue;
                }
                // operands[1] is the index.
                let index_operand = match op.operands.get(1) {
                    Some(&v) => v,
                    None => continue, // malformed op — skip
                };
                if let Some(&const_val) = const_int_value.get(&index_operand)
                    && const_val >= 0
                {
                    op.attrs
                        .insert("bce_safe".to_string(), AttrValue::Bool(true));
                    stats.values_changed += 1;
                }
            }
        }
    }

    stats
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::Terminator;
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;

    // ------------------------------------------------------------------
    // Helpers
    // ------------------------------------------------------------------

    fn make_op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands,
            results,
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    fn make_const_int(result: ValueId, value: i64) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Int(value));
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![result],
            attrs,
            source_span: None,
        }
    }

    // Build a minimal function with a single entry block containing the
    // given ops, terminated by `Return { values: [] }`.
    fn func_with_ops(ops: Vec<TirOp>) -> TirFunction {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = ops;
        entry.terminator = Terminator::Return { values: vec![] };
        func
    }

    // ------------------------------------------------------------------
    // Test 1: constant index >= 0 → marked bce_safe
    // ------------------------------------------------------------------
    #[test]
    fn constant_zero_index_marked_safe() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let container = func.fresh_value();
        let idx = func.fresh_value();
        let result = func.fresh_value();

        let ops = vec![
            make_op(OpCode::BuildList, vec![], vec![container]),
            make_const_int(idx, 0),
            make_op(OpCode::Index, vec![container, idx], vec![result]),
        ];

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = ops;
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let stats = run(&mut func);

        assert_eq!(stats.values_changed, 1);
        let index_op = func.blocks[&func.entry_block]
            .ops
            .iter()
            .find(|o| o.opcode == OpCode::Index)
            .expect("Index op must be present");
        assert_eq!(
            index_op.attrs.get("bce_safe"),
            Some(&AttrValue::Bool(true)),
            "Index op with const 0 index must be marked bce_safe"
        );
    }

    // ------------------------------------------------------------------
    // Test 2: positive constant index → marked bce_safe
    // ------------------------------------------------------------------
    #[test]
    fn positive_constant_index_marked_safe() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let container = func.fresh_value();
        let idx = func.fresh_value();
        let result = func.fresh_value();

        let ops = vec![
            make_op(OpCode::BuildList, vec![], vec![container]),
            make_const_int(idx, 42),
            make_op(OpCode::Index, vec![container, idx], vec![result]),
        ];

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = ops;
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let stats = run(&mut func);
        assert_eq!(stats.values_changed, 1);
        let index_op = func.blocks[&func.entry_block]
            .ops
            .iter()
            .find(|o| o.opcode == OpCode::Index)
            .unwrap();
        assert_eq!(index_op.attrs.get("bce_safe"), Some(&AttrValue::Bool(true)));
    }

    // ------------------------------------------------------------------
    // Test 3: negative constant index → NOT marked
    // ------------------------------------------------------------------
    #[test]
    fn negative_constant_index_not_marked() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let container = func.fresh_value();
        let idx = func.fresh_value();
        let result = func.fresh_value();

        let ops = vec![
            make_op(OpCode::BuildList, vec![], vec![container]),
            make_const_int(idx, -1),
            make_op(OpCode::Index, vec![container, idx], vec![result]),
        ];

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = ops;
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let stats = run(&mut func);
        assert_eq!(
            stats.values_changed, 0,
            "Negative constant must not be marked bce_safe"
        );
        let index_op = func.blocks[&func.entry_block]
            .ops
            .iter()
            .find(|o| o.opcode == OpCode::Index)
            .unwrap();
        assert!(
            !index_op.attrs.contains_key("bce_safe"),
            "bce_safe must be absent for negative index"
        );
    }

    // ------------------------------------------------------------------
    // Test 4: non-constant index → NOT marked
    // ------------------------------------------------------------------
    #[test]
    fn non_constant_index_not_marked() {
        // Index operand comes from a function parameter — not a ConstInt.
        let mut func = TirFunction::new("f".into(), vec![TirType::I64], TirType::None);
        let container = func.fresh_value();
        let result = func.fresh_value();
        let param_idx = ValueId(0); // function parameter

        let ops = vec![
            make_op(OpCode::BuildList, vec![], vec![container]),
            make_op(OpCode::Index, vec![container, param_idx], vec![result]),
        ];

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = ops;
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let stats = run(&mut func);
        assert_eq!(stats.values_changed, 0);
        let index_op = func.blocks[&func.entry_block]
            .ops
            .iter()
            .find(|o| o.opcode == OpCode::Index)
            .unwrap();
        assert!(!index_op.attrs.contains_key("bce_safe"));
    }

    // ------------------------------------------------------------------
    // Test 5: no Index ops → no changes, no panic
    // ------------------------------------------------------------------
    #[test]
    fn no_index_ops_no_changes() {
        let ops = vec![]; // empty body
        let mut func = func_with_ops(ops);
        let stats = run(&mut func);
        assert_eq!(stats.values_changed, 0);
    }

    // ------------------------------------------------------------------
    // Test 6: mixed ops — only constant non-negative indices marked
    // ------------------------------------------------------------------
    #[test]
    fn mixed_indices_only_safe_ones_marked() {
        let mut func = TirFunction::new("f".into(), vec![TirType::I64], TirType::None);
        let container = func.fresh_value();
        let const_idx = func.fresh_value(); // ConstInt(5) → safe
        let neg_idx = func.fresh_value(); // ConstInt(-2) → unsafe
        let param_idx = ValueId(0); // parameter → unsafe

        let r0 = func.fresh_value();
        let r1 = func.fresh_value();
        let r2 = func.fresh_value();

        let ops = vec![
            make_op(OpCode::BuildList, vec![], vec![container]),
            make_const_int(const_idx, 5),
            make_const_int(neg_idx, -2),
            // Index with const non-negative → should be marked
            make_op(OpCode::Index, vec![container, const_idx], vec![r0]),
            // Index with const negative → should NOT be marked
            make_op(OpCode::Index, vec![container, neg_idx], vec![r1]),
            // Index with non-constant → should NOT be marked
            make_op(OpCode::Index, vec![container, param_idx], vec![r2]),
        ];

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = ops;
        entry.terminator = Terminator::Return {
            values: vec![r0, r1, r2],
        };

        let stats = run(&mut func);
        assert_eq!(
            stats.values_changed, 1,
            "Only one Index should be marked bce_safe"
        );

        let index_ops: Vec<_> = func.blocks[&func.entry_block]
            .ops
            .iter()
            .filter(|o| o.opcode == OpCode::Index)
            .collect();

        assert_eq!(index_ops.len(), 3);
        // First Index (const_idx = 5) → safe
        assert_eq!(
            index_ops[0].attrs.get("bce_safe"),
            Some(&AttrValue::Bool(true))
        );
        // Second Index (neg_idx = -2) → not safe
        assert!(!index_ops[1].attrs.contains_key("bce_safe"));
        // Third Index (param) → not safe
        assert!(!index_ops[2].attrs.contains_key("bce_safe"));
    }
}
