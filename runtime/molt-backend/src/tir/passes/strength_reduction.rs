//! Strength Reduction Pass.
//!
//! Replaces expensive operations (Mul, Div, Mod, Pow) with cheaper equivalents
//! when one operand is a compile-time constant that is a power of two.
//! Only applies to I64-typed operands.

use std::collections::HashMap;

use super::PassStats;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

/// Run the strength reduction pass on `func`, returning statistics.
pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "strength_reduction",
        ..Default::default()
    };

    // Phase 1: Build a map from ValueId -> (ConstInt value, TirType) for all constant ops.
    let mut const_map: HashMap<ValueId, i64> = HashMap::new();
    // Also build a map from ValueId -> TirType for type checking.
    let mut type_map: HashMap<ValueId, TirType> = HashMap::new();

    // Seed types from block arguments.
    for block in func.blocks.values() {
        for arg in &block.args {
            type_map.insert(arg.id, arg.ty.clone());
        }
    }

    // Collect constant values and infer result types from constant ops.
    for block in func.blocks.values() {
        for op in &block.ops {
            match op.opcode {
                OpCode::ConstInt => {
                    if let Some(AttrValue::Int(v)) = op.attrs.get("value") {
                        for &res in &op.results {
                            const_map.insert(res, *v);
                            type_map.insert(res, TirType::I64);
                        }
                    }
                }
                OpCode::ConstFloat => {
                    for &res in &op.results {
                        type_map.insert(res, TirType::F64);
                    }
                }
                OpCode::ConstBool => {
                    for &res in &op.results {
                        type_map.insert(res, TirType::Bool);
                    }
                }
                _ => {}
            }
        }
    }

    // Phase 2: Scan all blocks and rewrite eligible ops.
    let block_ids: Vec<_> = func.blocks.keys().copied().collect();
    for bid in block_ids {
        let block = func.blocks.get_mut(&bid).unwrap();
        for op in &mut block.ops {
            if op.results.is_empty() || op.operands.len() < 2 {
                continue;
            }

            let lhs = op.operands[0];
            let rhs = op.operands[1];
            let results = op.results.clone();
            let source_span = op.source_span;

            // Only reduce I64 operations. Check that operands are I64-typed.
            // We use a heuristic: if the constant operand is from ConstInt, it's I64.
            // For the non-constant operand, check our type map.
            match op.opcode {
                OpCode::Mul => {
                    // x * 2 => x + x  (special case for exactly 2)
                    // x * 2^k => x << k
                    // Also handle 2 * x (commutative).
                    if let Some(mut rewrite) =
                        try_mul_reduce(lhs, rhs, &const_map, &type_map, &mut stats)
                    {
                        rewrite.results = results;
                        rewrite.source_span = source_span;
                        *op = rewrite;
                    }
                }
                OpCode::Pow => {
                    // x ** 2 => x * x
                    if let Some(&exp) = const_map.get(&rhs)
                        && exp == 2 && is_i64(&lhs, &type_map) {
                            op.opcode = OpCode::Mul;
                            op.operands = vec![lhs, lhs];
                            stats.values_changed += 1;
                        }
                }
                OpCode::FloorDiv => {
                    // x // 2^k => x >> k — deferred to Phase 3 (requires inserting
                    // a new ConstInt op for the shift amount, which needs an op
                    // insertion API not yet available).
                }
                OpCode::Mod => {
                    // x % 2^k => x & (2^k - 1) — same complexity issue as FloorDiv.
                    // Deferred to Phase 3.
                }
                _ => {}
            }
        }
    }

    stats
}

/// Attempt to reduce a Mul operation. Returns a replacement TirOp if applicable.
fn try_mul_reduce(
    lhs: ValueId,
    rhs: ValueId,
    const_map: &HashMap<ValueId, i64>,
    type_map: &HashMap<ValueId, TirType>,
    stats: &mut PassStats,
) -> Option<TirOp> {
    // Try rhs as constant first, then lhs (commutative).
    let (var_operand, const_val) = if let Some(&v) = const_map.get(&rhs) {
        if !is_i64(&lhs, type_map) {
            return None;
        }
        (lhs, v)
    } else if let Some(&v) = const_map.get(&lhs) {
        if !is_i64(&rhs, type_map) {
            return None;
        }
        (rhs, v)
    } else {
        return None;
    };

    if const_val <= 0 {
        return None;
    }

    if const_val == 2 {
        // x * 2 => x + x
        stats.values_changed += 1;
        return Some(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Add,
            operands: vec![var_operand, var_operand],
            results: vec![], // caller patches results
            attrs: AttrDict::new(),
            source_span: None,
        });
    }

    if (const_val as u64).is_power_of_two() {
        let k = const_val.trailing_zeros() as i64;
        // x * 2^k => x << k
        // We need a ConstInt(k) as the shift operand. We reuse the existing
        // constant ValueId (rhs or lhs) but we need to change its value from
        // 2^k to k. That would corrupt other uses.
        //
        // For Phase 2: we create the Shl op pointing to the same const operand.
        // The caller's SCCP pass (or a later cleanup) will handle the const value.
        // Actually, let's use `rhs` (the const operand) directly — but its value
        // is 2^k, not k. We need a new ValueId with value k.
        //
        // Since we can't easily insert a new op here, we use a simpler strategy:
        // return a Shl with the const operand, and note that the const value
        // needs to be k. We'll do a post-pass fixup.
        //
        // Even simpler: for the shift reduction, we store the shift amount as an
        // attribute on the Shl op itself, and the backend can read it from there.
        // But that's non-standard for the TIR.
        //
        // Practical approach: don't reduce x * (2^k) for k > 1 in this phase.
        // Only reduce x * 2 => x + x and x ** 2 => x * x.
        // We'll add Shl reduction in Phase 3 when we have a proper op insertion API.
        let _ = k;
        return None;
    }

    None
}

/// Check if a ValueId is known to have type I64.
fn is_i64(val: &ValueId, type_map: &HashMap<ValueId, TirType>) -> bool {
    matches!(type_map.get(val), Some(TirType::I64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::Terminator;
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;

    fn make_const_int(result: u32, value: i64) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Int(value));
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![ValueId(result)],
            attrs,
            source_span: None,
        }
    }

    fn make_const_float(result: u32, value: f64) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert("f_value".into(), AttrValue::Float(value));
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstFloat,
            operands: vec![],
            results: vec![ValueId(result)],
            attrs,
            source_span: None,
        }
    }

    fn make_binop(opcode: OpCode, result: u32, lhs: u32, rhs: u32) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: vec![ValueId(lhs), ValueId(rhs)],
            results: vec![ValueId(result)],
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    /// Helper: create a function with one I64 param and given ops, run strength reduction.
    fn run_sr(ops: Vec<TirOp>, next_value: u32) -> Vec<TirOp> {
        let mut func = TirFunction::new("test".into(), vec![TirType::I64], TirType::I64);
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops = ops;
            entry.terminator = Terminator::Return { values: vec![] };
        }
        func.next_value = next_value;
        run(&mut func);
        func.blocks[&func.entry_block].ops.clone()
    }

    #[test]
    fn mul_by_2_becomes_add() {
        // param x = ValueId(0), const 2 = ValueId(1), x * 2 = ValueId(2)
        let ops = vec![make_const_int(1, 2), make_binop(OpCode::Mul, 2, 0, 1)];
        let result = run_sr(ops, 3);
        assert_eq!(result[1].opcode, OpCode::Add);
        assert_eq!(result[1].operands, vec![ValueId(0), ValueId(0)]);
    }

    #[test]
    fn pow_2_becomes_mul() {
        // param x = ValueId(0), const 2 = ValueId(1), x ** 2 = ValueId(2)
        let ops = vec![make_const_int(1, 2), make_binop(OpCode::Pow, 2, 0, 1)];
        let result = run_sr(ops, 3);
        assert_eq!(result[1].opcode, OpCode::Mul);
        assert_eq!(result[1].operands, vec![ValueId(0), ValueId(0)]);
    }

    #[test]
    fn mul_by_3_unchanged() {
        // 3 is not a power of 2 — should not be rewritten.
        let ops = vec![make_const_int(1, 3), make_binop(OpCode::Mul, 2, 0, 1)];
        let result = run_sr(ops, 3);
        assert_eq!(result[1].opcode, OpCode::Mul);
        // Operands unchanged.
        assert_eq!(result[1].operands, vec![ValueId(0), ValueId(1)]);
    }

    #[test]
    fn mul_float_unchanged() {
        // x * 2.0 where x is I64 param but 2.0 is F64 — should not reduce.
        let ops = vec![make_const_float(1, 2.0), make_binop(OpCode::Mul, 2, 0, 1)];
        let result = run_sr(ops, 3);
        // The Mul should remain because rhs is F64, not I64.
        assert_eq!(result[1].opcode, OpCode::Mul);
    }

    #[test]
    fn mul_by_8_unchanged_phase2() {
        // x * 8 — ideally x << 3, but deferred to Phase 3 (no op insertion API).
        // For Phase 2, this remains a Mul.
        let ops = vec![make_const_int(1, 8), make_binop(OpCode::Mul, 2, 0, 1)];
        let result = run_sr(ops, 3);
        // Phase 2 doesn't reduce x * 8 yet.
        assert_eq!(result[1].opcode, OpCode::Mul);
    }
}
