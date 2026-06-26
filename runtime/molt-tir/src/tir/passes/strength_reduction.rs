//! Strength Reduction Pass.
//!
//! Replaces expensive operations (Mul, Div, Mod, Pow) with cheaper equivalents
//! when one operand is a compile-time constant that is a power of two.
//! Only applies to I64-typed operands.

use std::collections::HashMap;

use super::PassStats;
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    StrengthReductionRule, opcode_operand_independent_result_tir_type,
    opcode_strength_reduction_rule_table,
};
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

    // Collect constant values and infer operand-independent result types.
    for block in func.blocks.values() {
        for op in &block.ops {
            if op.opcode == OpCode::ConstInt
                && let Some(AttrValue::Int(v)) = op.attrs.get("value")
            {
                for &res in &op.results {
                    const_map.insert(res, *v);
                }
            }
            if let Some(ty) = opcode_operand_independent_result_tir_type(op.opcode) {
                for &res in &op.results {
                    type_map.insert(res, ty.clone());
                }
            }
        }
    }

    // Phase 2: Scan all blocks and rewrite eligible ops.
    let block_ids: Vec<_> = func.blocks.keys().copied().collect();
    for bid in block_ids {
        let old_ops = {
            let block = func.blocks.get_mut(&bid).unwrap();
            std::mem::take(&mut block.ops)
        };
        let mut new_ops = Vec::with_capacity(old_ops.len());

        for op in old_ops {
            if op.results.is_empty() || op.operands.len() < 2 {
                new_ops.push(op);
                continue;
            }

            let lhs = op.operands[0];
            let rhs = op.operands[1];
            let results = op.results.clone();
            let old = op.clone();

            // Only reduce I64 operations. Check that operands are I64-typed.
            // We use a heuristic: if the constant operand is from ConstInt, it's I64.
            // For the non-constant operand, check our type map.
            let rewrite = match opcode_strength_reduction_rule_table(op.opcode) {
                StrengthReductionRule::MulByTwo => {
                    try_mul_reduce(lhs, rhs, &const_map, &type_map, &mut stats)
                }
                StrengthReductionRule::PowSquare => {
                    try_pow_square_reduce(lhs, rhs, &const_map, &type_map, &mut stats)
                }
                StrengthReductionRule::PowerTwoFloorDiv => try_power_two_rhs_reduce(
                    lhs,
                    const_map.get(&rhs).copied(),
                    is_i64(&lhs, &type_map),
                    OpCode::Shr,
                    |divisor| positive_power_of_two_shift(divisor),
                    &old,
                    func,
                    &mut const_map,
                    &mut type_map,
                    &mut stats,
                ),
                StrengthReductionRule::PowerTwoMod => try_power_two_rhs_reduce(
                    lhs,
                    const_map.get(&rhs).copied(),
                    is_i64(&lhs, &type_map),
                    OpCode::BitAnd,
                    |divisor| {
                        positive_power_of_two_shift(divisor)?;
                        Some(divisor - 1)
                    },
                    &old,
                    func,
                    &mut const_map,
                    &mut type_map,
                    &mut stats,
                ),
                StrengthReductionRule::None => None,
            };

            if let Some(mut rewrite) = rewrite {
                rewrite.replacement.results = results;
                rewrite.replacement.inherit_source_from(&old);
                new_ops.extend(rewrite.prefix.drain(..));
                new_ops.push(rewrite.replacement);
            } else {
                new_ops.push(op);
            }
        }

        let block = func.blocks.get_mut(&bid).unwrap();
        block.ops = new_ops;
    }

    stats
}

struct StrengthRewrite {
    prefix: Vec<TirOp>,
    replacement: TirOp,
}

/// Attempt to reduce a Mul operation. Returns a replacement TirOp if applicable.
fn try_mul_reduce(
    lhs: ValueId,
    rhs: ValueId,
    const_map: &HashMap<ValueId, i64>,
    type_map: &HashMap<ValueId, TirType>,
    stats: &mut PassStats,
) -> Option<StrengthRewrite> {
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

    if const_val == 2 {
        // x * 2 => x + x
        stats.values_changed += 1;
        return Some(StrengthRewrite {
            prefix: Vec::new(),
            replacement: TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Add,
                operands: vec![var_operand, var_operand],
                results: vec![], // caller patches results
                attrs: AttrDict::new(),
                source_span: None,
            },
        });
    }

    None
}

fn try_pow_square_reduce(
    lhs: ValueId,
    rhs: ValueId,
    const_map: &HashMap<ValueId, i64>,
    type_map: &HashMap<ValueId, TirType>,
    stats: &mut PassStats,
) -> Option<StrengthRewrite> {
    if let Some(&exp) = const_map.get(&rhs)
        && exp == 2
        && is_i64(&lhs, type_map)
    {
        stats.values_changed += 1;
        return Some(StrengthRewrite {
            prefix: Vec::new(),
            replacement: TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Mul,
                operands: vec![lhs, lhs],
                results: vec![],
                attrs: AttrDict::new(),
                source_span: None,
            },
        });
    }
    None
}

fn try_power_two_rhs_reduce(
    lhs: ValueId,
    rhs_const: Option<i64>,
    lhs_is_i64: bool,
    replacement_opcode: OpCode,
    derived_const: impl FnOnce(i64) -> Option<i64>,
    source: &TirOp,
    func: &mut TirFunction,
    const_map_mut: &mut HashMap<ValueId, i64>,
    type_map_mut: &mut HashMap<ValueId, TirType>,
    stats: &mut PassStats,
) -> Option<StrengthRewrite> {
    let divisor = rhs_const?;
    let const_value = derived_const(divisor)?;
    if !lhs_is_i64 {
        return None;
    }
    let const_id = func.fresh_value();
    func.value_types.insert(const_id, TirType::I64);
    const_map_mut.insert(const_id, const_value);
    type_map_mut.insert(const_id, TirType::I64);
    stats.values_changed += 1;
    let mut const_op = const_int_op(const_id, const_value);
    const_op.inherit_source_from(source);
    Some(StrengthRewrite {
        prefix: vec![const_op],
        replacement: TirOp {
            dialect: Dialect::Molt,
            opcode: replacement_opcode,
            operands: vec![lhs, const_id],
            results: vec![], // caller patches results
            attrs: AttrDict::new(),
            source_span: None,
        },
    })
}

fn positive_power_of_two_shift(value: i64) -> Option<i64> {
    if value > 0 && (value as u64).is_power_of_two() {
        Some(value.trailing_zeros() as i64)
    } else {
        None
    }
}

fn const_int_op(result: ValueId, value: i64) -> TirOp {
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
    fn run_sr(ops: Vec<TirOp>, next_value: u32) -> TirFunction {
        let mut func = TirFunction::new("test".into(), vec![TirType::I64], TirType::I64);
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops = ops;
            entry.terminator = Terminator::Return { values: vec![] };
        }
        func.next_value = next_value;
        run(&mut func);
        func
    }

    fn entry_ops(func: &TirFunction) -> &[TirOp] {
        &func.blocks[&func.entry_block].ops
    }

    #[test]
    fn mul_by_2_becomes_add() {
        // param x = ValueId(0), const 2 = ValueId(1), x * 2 = ValueId(2)
        let ops = vec![make_const_int(1, 2), make_binop(OpCode::Mul, 2, 0, 1)];
        let func = run_sr(ops, 3);
        let result = entry_ops(&func);
        assert_eq!(result[1].opcode, OpCode::Add);
        assert_eq!(result[1].operands, vec![ValueId(0), ValueId(0)]);
    }

    #[test]
    fn pow_2_becomes_mul() {
        // param x = ValueId(0), const 2 = ValueId(1), x ** 2 = ValueId(2)
        let ops = vec![make_const_int(1, 2), make_binop(OpCode::Pow, 2, 0, 1)];
        let func = run_sr(ops, 3);
        let result = entry_ops(&func);
        assert_eq!(result[1].opcode, OpCode::Mul);
        assert_eq!(result[1].operands, vec![ValueId(0), ValueId(0)]);
    }

    #[test]
    fn mul_by_3_unchanged() {
        // 3 is not a power of 2 — should not be rewritten.
        let ops = vec![make_const_int(1, 3), make_binop(OpCode::Mul, 2, 0, 1)];
        let func = run_sr(ops, 3);
        let result = entry_ops(&func);
        assert_eq!(result[1].opcode, OpCode::Mul);
        // Operands unchanged.
        assert_eq!(result[1].operands, vec![ValueId(0), ValueId(1)]);
    }

    #[test]
    fn mul_float_unchanged() {
        // x * 2.0 where x is I64 param but 2.0 is F64 — should not reduce.
        let ops = vec![make_const_float(1, 2.0), make_binop(OpCode::Mul, 2, 0, 1)];
        let func = run_sr(ops, 3);
        let result = entry_ops(&func);
        // The Mul should remain because rhs is F64, not I64.
        assert_eq!(result[1].opcode, OpCode::Mul);
    }

    #[test]
    fn mul_by_8_stays_mul_without_range_proof() {
        // Shl is not an overflow-custody replacement for Mul unless a separate
        // range proof exists, so the semantics-preserving reduction remains
        // limited to x * 2 => x + x.
        let ops = vec![make_const_int(1, 8), make_binop(OpCode::Mul, 2, 0, 1)];
        let func = run_sr(ops, 3);
        let result = entry_ops(&func);
        assert_eq!(result[1].opcode, OpCode::Mul);
    }

    #[test]
    fn floordiv_by_power_two_becomes_shift_with_fresh_const() {
        let ops = vec![make_const_int(1, 8), make_binop(OpCode::FloorDiv, 2, 0, 1)];
        let func = run_sr(ops, 3);
        let result = entry_ops(&func);
        assert_eq!(result.len(), 3);
        assert_eq!(result[1].opcode, OpCode::ConstInt);
        assert_eq!(result[1].attrs.get("value"), Some(&AttrValue::Int(3)));
        let fresh_const = result[1].results[0];
        assert_eq!(func.value_types.get(&fresh_const), Some(&TirType::I64));
        assert_eq!(result[2].opcode, OpCode::Shr);
        assert_eq!(result[2].operands, vec![ValueId(0), fresh_const]);
        assert_eq!(result[2].results, vec![ValueId(2)]);
    }

    #[test]
    fn mod_by_power_two_becomes_bitand_with_fresh_mask() {
        let ops = vec![make_const_int(1, 8), make_binop(OpCode::Mod, 2, 0, 1)];
        let func = run_sr(ops, 3);
        let result = entry_ops(&func);
        assert_eq!(result.len(), 3);
        assert_eq!(result[1].opcode, OpCode::ConstInt);
        assert_eq!(result[1].attrs.get("value"), Some(&AttrValue::Int(7)));
        let fresh_const = result[1].results[0];
        assert_eq!(func.value_types.get(&fresh_const), Some(&TirType::I64));
        assert_eq!(result[2].opcode, OpCode::BitAnd);
        assert_eq!(result[2].operands, vec![ValueId(0), fresh_const]);
        assert_eq!(result[2].results, vec![ValueId(2)]);
    }
}
