//! Deoptimization framework for speculative optimization.
//!
//! When type speculation fails at runtime (e.g., PGO predicted int but got str),
//! the deopt mechanism transfers execution to a generic (unoptimized) version.

use super::blocks::BlockId;
use super::function::TirFunction;
use super::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use super::values::ValueId;

/// Deoptimization point descriptor.
#[derive(Debug, Clone)]
pub struct DeoptPoint {
    /// Name of the generic (fallback) function to transfer to.
    pub fallback_func: String,
    /// Live SSA values that must be materialized in the fallback.
    pub live_values: Vec<ValueId>,
    /// Mapping: SSA value -> variable slot in the fallback function.
    pub var_mapping: Vec<(ValueId, String)>,
    /// Why this deopt exists (for diagnostics).
    pub reason: DeoptReason,
}

/// Reason a deoptimization point exists.
#[derive(Debug, Clone)]
pub enum DeoptReason {
    TypeMismatch { expected: String, actual: String },
    OverflowTooBig,
    GuardFailure,
}

/// Generate a deopt handler that materializes live values and jumps to fallback.
/// Returns the handler as a sequence of TIR ops.
pub fn generate_deopt_handler(point: &DeoptPoint) -> Vec<TirOp> {
    // For each live value: store to the materialization buffer
    // Then: call the fallback function with the buffer
    let mut ops = Vec::new();
    // In a full implementation, this would:
    // 1. Allocate a DeoptState struct on the stack
    // 2. Store each live value into the struct at the mapped slot
    // 3. Call @molt_deopt_transfer(fallback_func, deopt_state)
    // For now: emit a Call to the fallback with live values as args
    ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Call,
        operands: point.live_values.clone(),
        results: vec![],
        attrs: {
            let mut a = AttrDict::new();
            a.insert("callee".into(), AttrValue::Str(point.fallback_func.clone()));
            a.insert("deopt".into(), AttrValue::Bool(true));
            a
        },
        source_span: None,
    });
    ops
}

/// Analyze a function to find deopt points (TypeGuard ops that could fail).
pub fn find_deopt_points(func: &TirFunction) -> Vec<(BlockId, usize, DeoptPoint)> {
    let mut points = Vec::new();
    for (bid, block) in &func.blocks {
        for (i, op) in block.ops.iter().enumerate() {
            if op.opcode == OpCode::TypeGuard {
                if let Some(AttrValue::Str(expected)) = op.attrs.get("expected_type") {
                    points.push((
                        *bid,
                        i,
                        DeoptPoint {
                            fallback_func: format!("{}_generic", func.name),
                            live_values: op.operands.clone(),
                            var_mapping: vec![],
                            reason: DeoptReason::TypeMismatch {
                                expected: expected.clone(),
                                actual: "unknown".into(),
                            },
                        },
                    ));
                }
            }
        }
    }
    points
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::{BlockId, Terminator};
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;

    fn make_func_with_ops(name: &str, ops: Vec<TirOp>) -> TirFunction {
        let mut func = TirFunction::new(name.into(), vec![], TirType::None);
        let entry = func.blocks.get_mut(&BlockId(0)).unwrap();
        entry.ops = ops;
        entry.terminator = Terminator::Return { values: vec![] };
        func
    }

    #[test]
    fn find_deopt_points_with_type_guard() {
        let guard_op = TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::TypeGuard,
            operands: vec![ValueId(0)],
            results: vec![ValueId(1)],
            attrs: {
                let mut a = AttrDict::new();
                a.insert("expected_type".into(), AttrValue::Str("int".into()));
                a
            },
            source_span: None,
        };
        let func = make_func_with_ops("test_fn", vec![guard_op]);
        let points = find_deopt_points(&func);
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].1, 0); // op index
        assert_eq!(points[0].2.fallback_func, "test_fn_generic");
        assert!(matches!(
            &points[0].2.reason,
            DeoptReason::TypeMismatch { expected, .. } if expected == "int"
        ));
    }

    #[test]
    fn find_deopt_points_without_type_guard_is_empty() {
        let add_op = TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Add,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![ValueId(2)],
            attrs: AttrDict::new(),
            source_span: None,
        };
        let func = make_func_with_ops("no_guards", vec![add_op]);
        let points = find_deopt_points(&func);
        assert!(points.is_empty());
    }

    #[test]
    fn generate_deopt_handler_produces_call_with_deopt_attr() {
        let point = DeoptPoint {
            fallback_func: "fallback_fn".into(),
            live_values: vec![ValueId(0), ValueId(1)],
            var_mapping: vec![],
            reason: DeoptReason::GuardFailure,
        };
        let ops = generate_deopt_handler(&point);
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].opcode, OpCode::Call);
        assert_eq!(
            ops[0].attrs.get("callee"),
            Some(&AttrValue::Str("fallback_fn".into()))
        );
        assert_eq!(
            ops[0].attrs.get("deopt"),
            Some(&AttrValue::Bool(true))
        );
        assert_eq!(ops[0].operands.len(), 2);
    }
}
