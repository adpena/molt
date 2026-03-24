//! Sparse Conditional Constant Propagation (SCCP).
//!
//! Propagates constants through the SSA graph, folds constant operations,
//! and eliminates branches with known-constant conditions.
//!
//! This is a simplified single-pass forward scan that folds obvious constants.
//! An iterative fixpoint version can replace it later.

use std::collections::HashMap;

use super::PassStats;
use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, OpCode};
use crate::tir::values::ValueId;

/// A value in the constant-propagation lattice.
#[derive(Debug, Clone, PartialEq)]
enum LatticeValue {
    /// Unknown — may still be constant (not yet visited).
    Top,
    /// Known constant value.
    Constant(ConstVal),
    /// Overdefined — definitely not constant.
    Bottom,
}

/// Concrete constant values carried through the lattice.
#[derive(Debug, Clone, PartialEq)]
enum ConstVal {
    Int(i64),
    Float(f64),
    Bool(bool),
    None,
}

/// Run the SCCP pass on `func`, returning statistics.
pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "sccp",
        ..Default::default()
    };

    // Phase 1: Build the lattice from all existing ops.
    let mut lattice: HashMap<ValueId, LatticeValue> = HashMap::new();

    // Block arguments are Bottom (parameters / phi-like — not constant).
    for block in func.blocks.values() {
        for arg in &block.args {
            lattice.insert(arg.id, LatticeValue::Bottom);
        }
    }

    // Collect block ids for deterministic iteration (sorted).
    let mut block_ids: Vec<BlockId> = func.blocks.keys().copied().collect();
    block_ids.sort_by_key(|b| b.0);

    // First pass: seed constants from ConstInt/ConstFloat/ConstBool/ConstNone ops,
    // mark everything else as Top initially.
    for &bid in &block_ids {
        let block = &func.blocks[&bid];
        for op in &block.ops {
            for &res in &op.results {
                let val = match op.opcode {
                    OpCode::ConstInt => {
                        if let Some(AttrValue::Int(v)) = op.attrs.get("value") {
                            LatticeValue::Constant(ConstVal::Int(*v))
                        } else {
                            LatticeValue::Bottom
                        }
                    }
                    OpCode::ConstFloat => {
                        if let Some(AttrValue::Float(v)) = op.attrs.get("f_value") {
                            LatticeValue::Constant(ConstVal::Float(*v))
                        } else {
                            LatticeValue::Bottom
                        }
                    }
                    OpCode::ConstBool => {
                        if let Some(AttrValue::Bool(v)) = op.attrs.get("value") {
                            LatticeValue::Constant(ConstVal::Bool(*v))
                        } else {
                            LatticeValue::Bottom
                        }
                    }
                    OpCode::ConstNone => LatticeValue::Constant(ConstVal::None),
                    _ => LatticeValue::Top,
                };
                lattice.insert(res, val);
            }
        }
    }

    // Phase 2: Forward propagation — try to fold ops with all-constant operands.
    // Iterate until stable (bounded by number of values).
    let mut changed = true;
    while changed {
        changed = false;
        for &bid in &block_ids {
            let block = &func.blocks[&bid];
            for op in &block.ops {
                if op.results.is_empty() {
                    continue;
                }
                // Skip ops that are already resolved as Constant or Bottom.
                let result_id = op.results[0];
                match lattice.get(&result_id) {
                    Some(LatticeValue::Bottom) | Some(LatticeValue::Constant(_)) => continue,
                    _ => {}
                }

                // Gather operand lattice values.
                let operand_vals: Vec<Option<&ConstVal>> = op
                    .operands
                    .iter()
                    .map(|v| match lattice.get(v) {
                        Some(LatticeValue::Constant(c)) => Some(c),
                        _ => None,
                    })
                    .collect();

                // If any operand is Bottom, this result is Bottom.
                let any_bottom = op.operands.iter().any(|v| {
                    matches!(lattice.get(v), Some(LatticeValue::Bottom))
                });
                if any_bottom {
                    lattice.insert(result_id, LatticeValue::Bottom);
                    changed = true;
                    continue;
                }

                // If any operand is still Top, we can't fold yet.
                if operand_vals.iter().any(|v| v.is_none()) {
                    continue;
                }

                // All operands are Constant — try to evaluate.
                if let Some(result) = evaluate_op(op.opcode, &operand_vals) {
                    lattice.insert(result_id, LatticeValue::Constant(result));
                    changed = true;
                } else {
                    // Can't fold this opcode — mark Bottom.
                    lattice.insert(result_id, LatticeValue::Bottom);
                    changed = true;
                }
            }
        }
    }

    // Phase 3: Rewrite — replace constant-valued ops with ConstXxx ops.
    for &bid in &block_ids {
        let block = func.blocks.get_mut(&bid).unwrap();
        for op in &mut block.ops {
            if op.results.is_empty() {
                continue;
            }
            let result_id = op.results[0];
            // Don't rewrite ops that are already constant constructors.
            match op.opcode {
                OpCode::ConstInt | OpCode::ConstFloat | OpCode::ConstBool | OpCode::ConstNone => {
                    continue;
                }
                _ => {}
            }
            if let Some(LatticeValue::Constant(cv)) = lattice.get(&result_id) {
                match cv {
                    ConstVal::Int(v) => {
                        let mut attrs = AttrDict::new();
                        attrs.insert("value".into(), AttrValue::Int(*v));
                        op.opcode = OpCode::ConstInt;
                        op.operands.clear();
                        op.attrs = attrs;
                        stats.values_changed += 1;
                    }
                    ConstVal::Float(v) => {
                        let mut attrs = AttrDict::new();
                        attrs.insert("f_value".into(), AttrValue::Float(*v));
                        op.opcode = OpCode::ConstFloat;
                        op.operands.clear();
                        op.attrs = attrs;
                        stats.values_changed += 1;
                    }
                    ConstVal::Bool(v) => {
                        let mut attrs = AttrDict::new();
                        attrs.insert("value".into(), AttrValue::Bool(*v));
                        op.opcode = OpCode::ConstBool;
                        op.operands.clear();
                        op.attrs = attrs;
                        stats.values_changed += 1;
                    }
                    ConstVal::None => {
                        op.opcode = OpCode::ConstNone;
                        op.operands.clear();
                        op.attrs = AttrDict::new();
                        stats.values_changed += 1;
                    }
                }
            }
        }
    }

    // Phase 4: Fold constant conditional branches to unconditional branches.
    for &bid in &block_ids {
        let block = func.blocks.get_mut(&bid).unwrap();
        let new_term = match &block.terminator {
            Terminator::CondBranch {
                cond,
                then_block,
                then_args,
                else_block,
                else_args,
            } => {
                match lattice.get(cond) {
                    Some(LatticeValue::Constant(ConstVal::Bool(true))) => {
                        Some(Terminator::Branch {
                            target: *then_block,
                            args: then_args.clone(),
                        })
                    }
                    Some(LatticeValue::Constant(ConstVal::Bool(false))) => {
                        Some(Terminator::Branch {
                            target: *else_block,
                            args: else_args.clone(),
                        })
                    }
                    // Python truthiness: nonzero int is truthy
                    Some(LatticeValue::Constant(ConstVal::Int(v))) => {
                        if *v != 0 {
                            Some(Terminator::Branch {
                                target: *then_block,
                                args: then_args.clone(),
                            })
                        } else {
                            Some(Terminator::Branch {
                                target: *else_block,
                                args: else_args.clone(),
                            })
                        }
                    }
                    Some(LatticeValue::Constant(ConstVal::None)) => {
                        Some(Terminator::Branch {
                            target: *else_block,
                            args: else_args.clone(),
                        })
                    }
                    _ => None,
                }
            }
            _ => None,
        };
        if let Some(term) = new_term {
            block.terminator = term;
            stats.ops_removed += 1; // count branch simplification
        }
    }

    stats
}

/// Try to evaluate a binary/unary op on constant operands.
fn evaluate_op(opcode: OpCode, operands: &[Option<&ConstVal>]) -> Option<ConstVal> {
    match opcode {
        // Binary arithmetic
        // Use checked arithmetic to avoid panic on overflow in debug / silent wrap in release.
        // On overflow, return None → value stays as Bottom (unfoldable), matching Python's BigInt.
        OpCode::Add => eval_binary(operands, |a, b| a.checked_add(b), |a, b| Some(a + b)),
        OpCode::Sub => eval_binary(operands, |a, b| a.checked_sub(b), |a, b| Some(a - b)),
        OpCode::Mul => eval_binary(operands, |a, b| a.checked_mul(b), |a, b| Some(a * b)),
        OpCode::Div => eval_binary_div(operands),
        OpCode::FloorDiv => eval_binary_floordiv(operands),
        OpCode::Mod => eval_binary_mod(operands),
        OpCode::Pow => eval_binary_pow(operands),

        // Comparisons
        OpCode::Eq => eval_cmp(operands, |a, b| a == b, |a, b| a == b, |a, b| a == b),
        OpCode::Ne => eval_cmp(operands, |a, b| a != b, |a, b| a != b, |a, b| a != b),
        OpCode::Lt => eval_cmp(operands, |a, b| a < b, |a, b| a < b, |a, b| a < b),
        OpCode::Le => eval_cmp(operands, |a, b| a <= b, |a, b| a <= b, |a, b| a <= b),
        OpCode::Gt => eval_cmp(operands, |a, b| a > b, |a, b| a > b, |a, b| a > b),
        OpCode::Ge => eval_cmp(operands, |a, b| a >= b, |a, b| a >= b, |a, b| a >= b),

        // Unary
        OpCode::Neg => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Int(v) => v.checked_neg().map(ConstVal::Int),
                ConstVal::Float(v) => Some(ConstVal::Float(-v)),
                _ => None,
            }
        }
        OpCode::Not => {
            let a = operands.first().copied().flatten()?;
            match a {
                ConstVal::Bool(v) => Some(ConstVal::Bool(!v)),
                _ => None,
            }
        }

        _ => None,
    }
}

/// Evaluate a binary arithmetic op on int or float operands.
/// Int operations use checked arithmetic — returns None on overflow
/// (matching Python's BigInt promotion behavior: we can't fold it, so leave it unfoldable).
fn eval_binary(
    operands: &[Option<&ConstVal>],
    int_op: impl Fn(i64, i64) -> Option<i64>,
    float_op: impl Fn(f64, f64) -> Option<f64>,
) -> Option<ConstVal> {
    let a = operands.first().copied().flatten()?;
    let b = operands.get(1).copied().flatten()?;
    match (a, b) {
        (ConstVal::Int(x), ConstVal::Int(y)) => int_op(*x, *y).map(ConstVal::Int),
        (ConstVal::Float(x), ConstVal::Float(y)) => float_op(*x, *y).map(ConstVal::Float),
        _ => None,
    }
}

fn eval_binary_div(operands: &[Option<&ConstVal>]) -> Option<ConstVal> {
    let a = operands.first().copied().flatten()?;
    let b = operands.get(1).copied().flatten()?;
    match (a, b) {
        (ConstVal::Int(x), ConstVal::Int(y)) if *y != 0 => {
            // Python `/` on ints returns float
            Some(ConstVal::Float(*x as f64 / *y as f64))
        }
        (ConstVal::Float(x), ConstVal::Float(y)) if *y != 0.0 => {
            Some(ConstVal::Float(*x / *y))
        }
        _ => None,
    }
}

fn eval_binary_floordiv(operands: &[Option<&ConstVal>]) -> Option<ConstVal> {
    let a = operands.first().copied().flatten()?;
    let b = operands.get(1).copied().flatten()?;
    match (a, b) {
        (ConstVal::Int(x), ConstVal::Int(y)) if *y != 0 => {
            // Python floor division: rounds towards negative infinity.
            // Rust's div_euclid rounds towards zero for negative divisors — WRONG.
            // Use explicit floor division: q = x/y, adjust if signs differ and not exact.
            let q = x / y;
            let r = x % y;
            let result = if r != 0 && ((*x ^ *y) < 0) { q - 1 } else { q };
            Some(ConstVal::Int(result))
        }
        (ConstVal::Float(x), ConstVal::Float(y)) if *y != 0.0 => {
            Some(ConstVal::Float((*x / *y).floor()))
        }
        _ => None,
    }
}

fn eval_binary_mod(operands: &[Option<&ConstVal>]) -> Option<ConstVal> {
    let a = operands.first().copied().flatten()?;
    let b = operands.get(1).copied().flatten()?;
    match (a, b) {
        (ConstVal::Int(x), ConstVal::Int(y)) if *y != 0 => {
            // Python modulo: result has the sign of the divisor.
            // C/Rust rem_euclid always returns non-negative — WRONG for negative divisors.
            let r = *x % *y;
            let result = if r != 0 && ((r ^ *y) < 0) { r + *y } else { r };
            Some(ConstVal::Int(result))
        }
        (ConstVal::Float(x), ConstVal::Float(y)) if *y != 0.0 => {
            // Python modulo semantics
            let r = *x % *y;
            let result = if r != 0.0 && r.signum() != y.signum() {
                r + *y
            } else {
                r
            };
            Some(ConstVal::Float(result))
        }
        _ => None,
    }
}

fn eval_binary_pow(operands: &[Option<&ConstVal>]) -> Option<ConstVal> {
    let a = operands.first().copied().flatten()?;
    let b = operands.get(1).copied().flatten()?;
    match (a, b) {
        (ConstVal::Int(base), ConstVal::Int(exp)) => {
            if *exp >= 0 && *exp <= 63 {
                // Safe small exponent — use checked pow to avoid overflow panic.
                base.checked_pow(*exp as u32).map(ConstVal::Int)
            } else {
                None
            }
        }
        (ConstVal::Float(x), ConstVal::Float(y)) => Some(ConstVal::Float(x.powf(*y))),
        _ => None,
    }
}

/// Evaluate a comparison op.
fn eval_cmp(
    operands: &[Option<&ConstVal>],
    int_cmp: impl Fn(i64, i64) -> bool,
    float_cmp: impl Fn(f64, f64) -> bool,
    bool_cmp: impl Fn(bool, bool) -> bool,
) -> Option<ConstVal> {
    let a = operands.first().copied().flatten()?;
    let b = operands.get(1).copied().flatten()?;
    match (a, b) {
        (ConstVal::Int(x), ConstVal::Int(y)) => Some(ConstVal::Bool(int_cmp(*x, *y))),
        (ConstVal::Float(x), ConstVal::Float(y)) => Some(ConstVal::Bool(float_cmp(*x, *y))),
        (ConstVal::Bool(x), ConstVal::Bool(y)) => Some(ConstVal::Bool(bool_cmp(*x, *y))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::TirBlock;
    use crate::tir::ops::{Dialect, TirOp};
    use crate::tir::types::TirType;

    /// Helper: create a function with a single block, apply SCCP, return the block's ops.
    fn run_sccp_on_ops(ops: Vec<TirOp>, next_value: u32) -> (Vec<TirOp>, Terminator) {
        let mut func = TirFunction::new("test".into(), vec![], TirType::None);
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops = ops;
            entry.terminator = Terminator::Return {
                values: vec![],
            };
        }
        func.next_value = next_value;
        run(&mut func);
        let entry = &func.blocks[&func.entry_block];
        (entry.ops.clone(), entry.terminator.clone())
    }

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

    fn make_const_bool(result: u32, value: bool) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Bool(value));
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstBool,
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

    #[test]
    fn fold_int_addition() {
        // 1 + 2 => 3
        let ops = vec![
            make_const_int(0, 1),
            make_const_int(1, 2),
            make_binop(OpCode::Add, 2, 0, 1),
        ];
        let (result_ops, _) = run_sccp_on_ops(ops, 3);
        // The Add op should be rewritten to ConstInt(3).
        assert_eq!(result_ops[2].opcode, OpCode::ConstInt);
        assert_eq!(result_ops[2].attrs.get("value"), Some(&AttrValue::Int(3)));
    }

    #[test]
    fn fold_comparison_gt() {
        // 5 > 3 => true
        let ops = vec![
            make_const_int(0, 5),
            make_const_int(1, 3),
            make_binop(OpCode::Gt, 2, 0, 1),
        ];
        let (result_ops, _) = run_sccp_on_ops(ops, 3);
        assert_eq!(result_ops[2].opcode, OpCode::ConstBool);
        assert_eq!(
            result_ops[2].attrs.get("value"),
            Some(&AttrValue::Bool(true))
        );
    }

    #[test]
    fn fold_constant_cond_branch_true() {
        // if true: goto bb1, else: goto bb2 => Branch to bb1
        let mut func = TirFunction::new("test".into(), vec![], TirType::None);
        let then_id = func.fresh_block();
        let else_id = func.fresh_block();

        let const_true = make_const_bool(0, true);
        func.next_value = 1;

        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(const_true);
            entry.terminator = Terminator::CondBranch {
                cond: ValueId(0),
                then_block: then_id,
                then_args: vec![],
                else_block: else_id,
                else_args: vec![],
            };
        }

        // Add stub blocks so iteration doesn't miss them.
        func.blocks.insert(
            then_id,
            TirBlock {
                id: then_id,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.blocks.insert(
            else_id,
            TirBlock {
                id: else_id,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );

        let stats = run(&mut func);
        let entry = &func.blocks[&func.entry_block];
        match &entry.terminator {
            Terminator::Branch { target, .. } => {
                assert_eq!(*target, then_id);
            }
            other => panic!("expected Branch, got {:?}", other),
        }
        assert!(stats.ops_removed > 0);
    }

    #[test]
    fn no_fold_parameter_plus_const() {
        // x + 1 where x is a function parameter => no folding
        let mut func = TirFunction::new("test".into(), vec![TirType::I64], TirType::I64);
        // param is ValueId(0)
        let const_one = make_const_int(1, 1);
        let add = make_binop(OpCode::Add, 2, 0, 1);
        func.next_value = 3;
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(const_one);
            entry.ops.push(add);
            entry.terminator = Terminator::Return {
                values: vec![ValueId(2)],
            };
        }

        let stats = run(&mut func);
        let entry = &func.blocks[&func.entry_block];
        // The Add should remain an Add (not folded).
        assert_eq!(entry.ops[1].opcode, OpCode::Add);
        assert_eq!(stats.values_changed, 0);
    }

    #[test]
    fn fold_float_multiplication() {
        // 1.0 * 2.0 => 2.0
        let ops = vec![
            make_const_float(0, 1.0),
            make_const_float(1, 2.0),
            make_binop(OpCode::Mul, 2, 0, 1),
        ];
        let (result_ops, _) = run_sccp_on_ops(ops, 3);
        assert_eq!(result_ops[2].opcode, OpCode::ConstFloat);
        assert_eq!(
            result_ops[2].attrs.get("f_value"),
            Some(&AttrValue::Float(2.0))
        );
    }
}
