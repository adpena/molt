//! Fast-Math Mode Annotation Pass.
//!
//! When a function carries `attrs["fast_math"] = true` (set by the frontend
//! for a `@fast_math`-decorated function), this pass walks every op in the
//! function and annotates all floating-point arithmetic ops with the same
//! flag.  The LLVM lowering layer reads this attr and emits the `fast` flag
//! on the corresponding LLVM instruction, enabling:
//!
//! - FMA contraction:  `a * b + c → fma(a, b, c)`
//! - Reassociation:    `(a + b) + c → a + (b + c)`  (enables vectorised reductions)
//! - Reciprocal approx: `a / b → a * approx_recip(b)`
//! - Suppression of NaN / Inf guard checks
//!
//! The pass is **opt-in only**: without the function-level attr nothing is
//! changed, preserving full CPython parity by default.
//!
//! Only `F64`-typed operands are eligible.  `I64` arithmetic and untyped
//! (DynBox) ops are never marked.

use std::collections::HashMap;

use super::PassStats;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

/// Arithmetic opcodes that can carry the fast-math flag when their operands
/// are `F64`-typed.
const FP_ARITH_OPS: &[OpCode] = &[
    OpCode::Add,
    OpCode::Sub,
    OpCode::Mul,
    OpCode::Div,
    OpCode::Mod,
];

/// Run the fast-math annotation pass on `func`.
///
/// Returns statistics reflecting how many ops were annotated.
pub fn run(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "fast_math",
        ..Default::default()
    };

    // Only proceed if the function is tagged for fast-math.
    match func.attrs.get("fast_math") {
        Some(AttrValue::Bool(true)) => {}
        _ => return stats,
    }

    // Build a ValueId → TirType map so we can check operand types.
    let mut type_map: HashMap<ValueId, TirType> = HashMap::new();

    for block in func.blocks.values() {
        // Block arguments carry explicit types.
        for arg in &block.args {
            type_map.insert(arg.id, arg.ty.clone());
        }
        // Infer result types from constant-producing ops.
        for op in &block.ops {
            match op.opcode {
                OpCode::ConstFloat => {
                    for &res in &op.results {
                        type_map.insert(res, TirType::F64);
                    }
                }
                OpCode::ConstInt => {
                    for &res in &op.results {
                        type_map.insert(res, TirType::I64);
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

    // Walk all ops and annotate eligible floating-point arithmetic.
    let block_ids: Vec<_> = func.blocks.keys().copied().collect();
    for bid in block_ids {
        let block = func.blocks.get_mut(&bid).unwrap();
        for op in &mut block.ops {
            if !FP_ARITH_OPS.contains(&op.opcode) {
                continue;
            }
            // Require at least one operand typed F64.  For binary ops both
            // operands should match; we check the first non-empty operand.
            let is_fp = op
                .operands
                .iter()
                .any(|v| matches!(type_map.get(v), Some(TirType::F64)));

            // Additionally accept result-typed-as-F64 for ops that produce F64
            // but whose operands aren't yet tracked (e.g. after unboxing).
            let result_is_fp = op
                .results
                .iter()
                .any(|v| matches!(type_map.get(v), Some(TirType::F64)));

            if is_fp || result_is_fp {
                op.attrs.insert("fast_math".into(), AttrValue::Bool(true));
                stats.values_changed += 1;
            }
        }
    }

    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::Terminator;
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;

    fn make_const_float(result: u32) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert("f_value".into(), AttrValue::Float(1.0));
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstFloat,
            operands: vec![],
            results: vec![ValueId(result)],
            attrs,
            source_span: None,
        }
    }

    fn make_const_int(result: u32) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Int(1));
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
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

    /// Run the pass on a function whose entry block contains `ops`.
    /// The function-level fast_math attr is set iff `mark_func` is true.
    fn run_pass(ops: Vec<TirOp>, mark_func: bool) -> Vec<TirOp> {
        let mut func = TirFunction::new("test".into(), vec![], TirType::None);
        if mark_func {
            func.attrs.insert("fast_math".into(), AttrValue::Bool(true));
        }
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops = ops;
            entry.terminator = Terminator::Return { values: vec![] };
        }
        run(&mut func);
        func.blocks[&func.entry_block].ops.clone()
    }

    #[test]
    fn fast_math_func_marks_f64_ops() {
        // ConstFloat(v0=1.0), ConstFloat(v1=2.0), Add(v2 = v0 + v1)
        let ops = vec![
            make_const_float(0),
            make_const_float(1),
            make_binop(OpCode::Add, 2, 0, 1),
        ];
        let result = run_pass(ops, true);
        assert_eq!(
            result[2].attrs.get("fast_math"),
            Some(&AttrValue::Bool(true)),
            "Add on F64 operands should be marked fast_math"
        );
    }

    #[test]
    fn no_fast_math_func_leaves_ops_unchanged() {
        let ops = vec![
            make_const_float(0),
            make_const_float(1),
            make_binop(OpCode::Add, 2, 0, 1),
        ];
        let result = run_pass(ops, false);
        assert!(
            result[2].attrs.get("fast_math").is_none(),
            "without function fast_math attr, ops must not be annotated"
        );
    }

    #[test]
    fn i64_ops_in_fast_math_func_not_marked() {
        // ConstInt(v0), ConstInt(v1), Add(v2 = v0 + v1) — all I64
        let ops = vec![
            make_const_int(0),
            make_const_int(1),
            make_binop(OpCode::Add, 2, 0, 1),
        ];
        let result = run_pass(ops, true);
        assert!(
            result[2].attrs.get("fast_math").is_none(),
            "I64 Add in fast_math function must NOT be marked"
        );
    }

    #[test]
    fn mixed_i64_f64_only_f64_marked() {
        // v0 = ConstFloat, v1 = ConstInt, v2 = Add(F64), v3 = Add(I64)
        let ops = vec![
            make_const_float(0),
            make_const_int(1),
            make_binop(OpCode::Add, 2, 0, 0), // F64 + F64
            make_binop(OpCode::Add, 3, 1, 1), // I64 + I64
        ];
        let result = run_pass(ops, true);
        assert_eq!(
            result[2].attrs.get("fast_math"),
            Some(&AttrValue::Bool(true)),
            "F64 Add should be marked"
        );
        assert!(
            result[3].attrs.get("fast_math").is_none(),
            "I64 Add must NOT be marked"
        );
    }
}
