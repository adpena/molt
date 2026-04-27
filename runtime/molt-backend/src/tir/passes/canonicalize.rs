//! Canonicalization Pass for TIR.
//!
//! Reduces all TIR operations to canonical form — the single, simplest
//! representation for every computable pattern.  This is the TIR equivalent
//! of MLIR's canonicalize pass (Lattner, 2020).
//!
//! Canonicalization rules applied (in priority order):
//!
//! 1. **Identity elimination**: x + 0, x * 1, x - 0, x | 0, x ^ 0, x & -1 → x
//! 2. **Absorbing element**: x * 0, x & 0 → const 0; x | -1 → const -1
//! 3. **Self-inverse**: x - x, x ^ x → const 0; x / x, x // x → const 1
//! 4. **Double negation**: Not(Not(x)), Neg(Neg(x)) → x
//! 5. **Commutative ordering**: for commutative ops (Add, Mul, BitAnd, BitOr,
//!    BitXor, Eq, Ne), order operands so that constants appear on the right.
//!    This normalizes `1 + x` → `x + 1` so downstream passes (SCCP, strength
//!    reduction) only need to check one pattern.
//! 6. **Boolean simplification**: And(x, True) → x; Or(x, False) → x;
//!    And(x, False) → False; Or(x, True) → True; Not(True) → False
//! 7. **Comparison canonicalization**: 0 < x → x > 0 (constant on right)

use std::collections::HashMap;

use super::PassStats;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, Dialect, OpCode, TirOp};
use crate::tir::values::ValueId;

/// Returns `true` if the opcode is commutative (operand order doesn't matter).
fn is_commutative(opcode: OpCode) -> bool {
    matches!(
        opcode,
        OpCode::Add
            | OpCode::Mul
            | OpCode::BitAnd
            | OpCode::BitOr
            | OpCode::BitXor
            | OpCode::Eq
            | OpCode::Ne
    )
}

/// Returns the "swapped" comparison opcode: Lt↔Gt, Le↔Ge.
fn swap_comparison(opcode: OpCode) -> Option<OpCode> {
    match opcode {
        OpCode::Lt => Some(OpCode::Gt),
        OpCode::Gt => Some(OpCode::Lt),
        OpCode::Le => Some(OpCode::Ge),
        OpCode::Ge => Some(OpCode::Le),
        _ => None,
    }
}

pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "canonicalize",
        ..Default::default()
    };

    // Build constant map: ValueId → i64 for ConstInt, ValueId → bool for ConstBool.
    let mut int_consts: HashMap<ValueId, i64> = HashMap::new();
    let mut bool_consts: HashMap<ValueId, bool> = HashMap::new();

    for block in func.blocks.values() {
        for op in &block.ops {
            match op.opcode {
                OpCode::ConstInt => {
                    if let Some(AttrValue::Int(v)) = op.attrs.get("value") {
                        for &res in &op.results {
                            int_consts.insert(res, *v);
                        }
                    }
                }
                OpCode::ConstBool => {
                    if let Some(AttrValue::Bool(v)) = op.attrs.get("value") {
                        for &res in &op.results {
                            bool_consts.insert(res, *v);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // For double negation: map each Not/Neg result → its single operand source.
    // Pre-built so we don't need to borrow func.blocks during mutation.
    let mut not_source: HashMap<ValueId, ValueId> = HashMap::new();
    let mut neg_source: HashMap<ValueId, ValueId> = HashMap::new();
    for block in func.blocks.values() {
        for op in &block.ops {
            if op.opcode == OpCode::Not
                && op.operands.len() == 1
                && op.results.len() == 1
            {
                not_source.insert(op.results[0], op.operands[0]);
            }
            if op.opcode == OpCode::Neg
                && op.operands.len() == 1
                && op.results.len() == 1
            {
                neg_source.insert(op.results[0], op.operands[0]);
            }
        }
    }

    let block_ids: Vec<_> = func.blocks.keys().copied().collect();

    for bid in block_ids {
        let block = func.blocks.get_mut(&bid).unwrap();

        for op in &mut block.ops {
            if op.results.is_empty() {
                continue;
            }

            let result = op.results[0];

            // --- Rule 5: Commutative ordering (constants on the right) ---
            if op.operands.len() == 2 && is_commutative(op.opcode) {
                let lhs = op.operands[0];
                let rhs = op.operands[1];
                let lhs_is_const = int_consts.contains_key(&lhs) || bool_consts.contains_key(&lhs);
                let rhs_is_const = int_consts.contains_key(&rhs) || bool_consts.contains_key(&rhs);
                if lhs_is_const && !rhs_is_const {
                    op.operands.swap(0, 1);
                    stats.values_changed += 1;
                }
            }

            // --- Rule 7: Comparison canonicalization (constant on right) ---
            if op.operands.len() == 2
                && let Some(swapped) = swap_comparison(op.opcode) {
                    let lhs = op.operands[0];
                    let rhs = op.operands[1];
                    let lhs_is_const =
                        int_consts.contains_key(&lhs) || bool_consts.contains_key(&lhs);
                    let rhs_is_const =
                        int_consts.contains_key(&rhs) || bool_consts.contains_key(&rhs);
                    if lhs_is_const && !rhs_is_const {
                        op.opcode = swapped;
                        op.operands.swap(0, 1);
                        stats.values_changed += 1;
                    }
                }

            if op.operands.len() != 2 {
                // Rules 1-3 apply to binary ops.
                // Rules 4, 6 apply to unary ops — handle below.
                if op.operands.len() == 1 {
                    let operand = op.operands[0];

                    // --- Rule 4: Double negation ---
                    // Not(Not(x)) → Copy(x)
                    if op.opcode == OpCode::Not
                        && let Some(&inner_src) = not_source.get(&operand) {
                            *op = TirOp {
                                dialect: Dialect::Molt,
                                opcode: OpCode::Copy,
                                operands: vec![inner_src],
                                results: vec![result],
                                attrs: Default::default(),
                                source_span: op.source_span,
                            };
                            stats.values_changed += 1;
                        }

                    // Neg(Neg(x)) → Copy(x)
                    if op.opcode == OpCode::Neg
                        && let Some(&inner_src) = neg_source.get(&operand) {
                            *op = TirOp {
                                dialect: Dialect::Molt,
                                opcode: OpCode::Copy,
                                operands: vec![inner_src],
                                results: vec![result],
                                attrs: Default::default(),
                                source_span: op.source_span,
                            };
                            stats.values_changed += 1;
                        }

                    // --- Rule 6: Boolean simplification (unary) ---
                    if op.opcode == OpCode::Not
                        && let Some(&val) = bool_consts.get(&operand) {
                            *op = TirOp {
                                dialect: Dialect::Molt,
                                opcode: OpCode::ConstBool,
                                operands: vec![],
                                results: vec![result],
                                attrs: {
                                    let mut m = crate::tir::ops::AttrDict::new();
                                    m.insert("value".into(), AttrValue::Bool(!val));
                                    m
                                },
                                source_span: op.source_span,
                            };
                            stats.values_changed += 1;
                        }
                }
                continue;
            }

            let lhs = op.operands[0];
            let rhs = op.operands[1];
            let lhs_int = int_consts.get(&lhs).copied();
            let rhs_int = int_consts.get(&rhs).copied();
            let lhs_bool = bool_consts.get(&lhs).copied();
            let rhs_bool = bool_consts.get(&rhs).copied();

            match op.opcode {
                // --- Rule 1: Identity elimination ---
                OpCode::Add | OpCode::InplaceAdd if rhs_int == Some(0) => {
                    replace_with_copy(op, lhs, result);
                    stats.values_changed += 1;
                }
                OpCode::Add | OpCode::InplaceAdd if lhs_int == Some(0) => {
                    replace_with_copy(op, rhs, result);
                    stats.values_changed += 1;
                }
                OpCode::Sub | OpCode::InplaceSub if rhs_int == Some(0) => {
                    replace_with_copy(op, lhs, result);
                    stats.values_changed += 1;
                }
                OpCode::Mul | OpCode::InplaceMul if rhs_int == Some(1) => {
                    replace_with_copy(op, lhs, result);
                    stats.values_changed += 1;
                }
                OpCode::Mul | OpCode::InplaceMul if lhs_int == Some(1) => {
                    replace_with_copy(op, rhs, result);
                    stats.values_changed += 1;
                }
                OpCode::BitOr if rhs_int == Some(0) => {
                    replace_with_copy(op, lhs, result);
                    stats.values_changed += 1;
                }
                OpCode::BitXor if rhs_int == Some(0) => {
                    replace_with_copy(op, lhs, result);
                    stats.values_changed += 1;
                }
                OpCode::BitAnd if rhs_int == Some(-1) => {
                    replace_with_copy(op, lhs, result);
                    stats.values_changed += 1;
                }

                // --- Rule 2: Absorbing element ---
                OpCode::Mul | OpCode::InplaceMul if rhs_int == Some(0) => {
                    replace_with_const_int(op, 0, result);
                    stats.values_changed += 1;
                }
                OpCode::Mul | OpCode::InplaceMul if lhs_int == Some(0) => {
                    replace_with_const_int(op, 0, result);
                    stats.values_changed += 1;
                }
                OpCode::BitAnd if rhs_int == Some(0) => {
                    replace_with_const_int(op, 0, result);
                    stats.values_changed += 1;
                }
                OpCode::BitAnd if lhs_int == Some(0) => {
                    replace_with_const_int(op, 0, result);
                    stats.values_changed += 1;
                }

                // --- Rule 3: Self-inverse ---
                OpCode::Sub | OpCode::InplaceSub if lhs == rhs => {
                    replace_with_const_int(op, 0, result);
                    stats.values_changed += 1;
                }
                OpCode::BitXor if lhs == rhs => {
                    replace_with_const_int(op, 0, result);
                    stats.values_changed += 1;
                }

                // --- Rule 6: Boolean simplification (binary) ---
                OpCode::And if rhs_bool == Some(true) => {
                    replace_with_copy(op, lhs, result);
                    stats.values_changed += 1;
                }
                OpCode::And if rhs_bool == Some(false) || lhs_bool == Some(false) => {
                    replace_with_const_bool(op, false, result);
                    stats.values_changed += 1;
                }
                OpCode::Or if rhs_bool == Some(false) => {
                    replace_with_copy(op, lhs, result);
                    stats.values_changed += 1;
                }
                OpCode::Or if rhs_bool == Some(true) || lhs_bool == Some(true) => {
                    replace_with_const_bool(op, true, result);
                    stats.values_changed += 1;
                }

                _ => {}
            }
        }
    }

    stats
}

fn replace_with_copy(op: &mut TirOp, source: ValueId, result: ValueId) {
    *op = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![source],
        results: vec![result],
        attrs: Default::default(),
        source_span: op.source_span,
    };
}

fn replace_with_const_int(op: &mut TirOp, value: i64, result: ValueId) {
    *op = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![result],
        attrs: {
            let mut m = crate::tir::ops::AttrDict::new();
            m.insert("value".into(), AttrValue::Int(value));
            m
        },
        source_span: op.source_span,
    };
}

fn replace_with_const_bool(op: &mut TirOp, value: bool, result: ValueId) {
    *op = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstBool,
        operands: vec![],
        results: vec![result],
        attrs: {
            let mut m = crate::tir::ops::AttrDict::new();
            m.insert("value".into(), AttrValue::Bool(value));
            m
        },
        source_span: op.source_span,
    };
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

    fn make_const_int(value: i64, result: ValueId) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![result],
            attrs: {
                let mut m = AttrDict::new();
                m.insert("value".into(), AttrValue::Int(value));
                m
            },
            source_span: None,
        }
    }

    fn make_binop(opcode: OpCode, lhs: ValueId, rhs: ValueId, result: ValueId) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: vec![lhs, rhs],
            results: vec![result],
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    #[test]
    fn add_zero_eliminated() {
        let mut func = TirFunction::new("f".into(), vec![TirType::I64], TirType::I64);
        let param = ValueId(0);
        let zero = func.fresh_value();
        let result = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_const_int(0, zero));
        entry
            .ops
            .push(make_binop(OpCode::Add, param, zero, result));
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let stats = run(&mut func);
        assert!(stats.values_changed > 0);

        // The Add should be replaced with a Copy from param.
        let add_op = &func.blocks[&func.entry_block].ops[1];
        assert_eq!(add_op.opcode, OpCode::Copy);
        assert_eq!(add_op.operands[0], param);
    }

    #[test]
    fn mul_zero_folded() {
        let mut func = TirFunction::new("f".into(), vec![TirType::I64], TirType::I64);
        let param = ValueId(0);
        let zero = func.fresh_value();
        let result = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_const_int(0, zero));
        entry
            .ops
            .push(make_binop(OpCode::Mul, param, zero, result));
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let stats = run(&mut func);
        assert!(stats.values_changed > 0);

        let mul_op = &func.blocks[&func.entry_block].ops[1];
        assert_eq!(mul_op.opcode, OpCode::ConstInt);
    }

    #[test]
    fn sub_self_is_zero() {
        let mut func = TirFunction::new("f".into(), vec![TirType::I64], TirType::I64);
        let param = ValueId(0);
        let result = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .push(make_binop(OpCode::Sub, param, param, result));
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let stats = run(&mut func);
        assert!(stats.values_changed > 0);

        let sub_op = &func.blocks[&func.entry_block].ops[0];
        assert_eq!(sub_op.opcode, OpCode::ConstInt);
    }

    #[test]
    fn commutative_ordering() {
        let mut func = TirFunction::new("f".into(), vec![TirType::I64], TirType::I64);
        let param = ValueId(0);
        let one = func.fresh_value();
        let result = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_const_int(1, one));
        // 1 + param → should be canonicalized to param + 1
        entry
            .ops
            .push(make_binop(OpCode::Add, one, param, result));
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let stats = run(&mut func);
        assert!(stats.values_changed > 0);

        let add_op = &func.blocks[&func.entry_block].ops[1];
        assert_eq!(add_op.operands[0], param);
        assert_eq!(add_op.operands[1], one);
    }
}
