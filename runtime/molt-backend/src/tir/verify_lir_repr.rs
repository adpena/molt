//! LIR representation verifier.
//!
//! Enforces that values with proven scalar types (I64, F64, Bool) are
//! always represented as their scalar LirRepr, never DynBox.  This is
//! the Mojo "register-passable" invariant: scalar values live in SSA
//! registers end-to-end, boxing only at module boundaries.

use super::blocks::BlockId;
use super::lir::{LirFunction, LirRepr};
use super::types::TirType;
use super::values::ValueId;

/// A single register-passable violation: a value whose proven type
/// maps to a scalar repr but was lowered as DynBox.
#[derive(Debug)]
pub struct ReprViolation {
    pub block: BlockId,
    pub value_id: ValueId,
    pub expected_type: TirType,
    pub expected_repr: LirRepr,
    pub actual_repr: LirRepr,
}

/// Verify that no value with a proven scalar type uses DynBox representation.
///
/// Returns a list of violations.  An empty vec means the function is clean.
pub fn verify_register_passable(func: &LirFunction) -> Vec<ReprViolation> {
    let mut violations = Vec::new();

    for (&bid, block) in &func.blocks {
        // Check block arguments.
        check_values(bid, block.args.iter(), &mut violations);

        // Check op result values.
        for op in &block.ops {
            check_values(bid, op.result_values.iter(), &mut violations);
        }
    }

    violations
}

fn check_values<'a>(
    block: BlockId,
    values: impl Iterator<Item = &'a super::lir::LirValue>,
    violations: &mut Vec<ReprViolation>,
) {
    for v in values {
        let expected = LirRepr::for_type(&v.ty);
        if expected != LirRepr::DynBox && v.repr == LirRepr::DynBox {
            violations.push(ReprViolation {
                block,
                value_id: v.id,
                expected_type: v.ty.clone(),
                expected_repr: expected,
                actual_repr: v.repr,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::BlockId;
    use crate::tir::lir::{LirBlock, LirFunction, LirOp, LirRepr, LirTerminator, LirValue};
    use crate::tir::ops::{AttrDict, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;
    use std::collections::HashMap;

    fn make_tir_op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands,
            results,
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    fn make_lir_function(blocks: HashMap<BlockId, LirBlock>) -> LirFunction {
        LirFunction {
            name: "test_fn".into(),
            param_names: vec![],
            param_types: vec![],
            return_types: vec![],
            blocks,
            label_id_map: HashMap::new(),
            entry_block: BlockId(0),
        }
    }

    #[test]
    fn no_violation_when_i64_uses_scalar_repr() {
        let entry = BlockId(0);
        let mut blocks = HashMap::new();
        blocks.insert(
            entry,
            LirBlock {
                id: entry,
                args: vec![],
                ops: vec![LirOp {
                    tir_op: make_tir_op(OpCode::ConstInt, vec![], vec![ValueId(0)]),
                    result_values: vec![LirValue {
                        id: ValueId(0),
                        ty: TirType::I64,
                        repr: LirRepr::I64,
                    }],
                }],
                terminator: LirTerminator::Return {
                    values: vec![ValueId(0)],
                },
            },
        );
        let func = make_lir_function(blocks);
        let violations = verify_register_passable(&func);
        assert!(violations.is_empty(), "expected no violations: {violations:?}");
    }

    #[test]
    fn violation_when_i64_uses_dynbox() {
        let entry = BlockId(0);
        let mut blocks = HashMap::new();
        blocks.insert(
            entry,
            LirBlock {
                id: entry,
                args: vec![],
                ops: vec![LirOp {
                    tir_op: make_tir_op(OpCode::ConstInt, vec![], vec![ValueId(0)]),
                    result_values: vec![LirValue {
                        id: ValueId(0),
                        ty: TirType::I64,
                        repr: LirRepr::DynBox, // Wrong!
                    }],
                }],
                terminator: LirTerminator::Return {
                    values: vec![ValueId(0)],
                },
            },
        );
        let func = make_lir_function(blocks);
        let violations = verify_register_passable(&func);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].value_id, ValueId(0));
        assert_eq!(violations[0].expected_repr, LirRepr::I64);
        assert_eq!(violations[0].actual_repr, LirRepr::DynBox);
    }

    #[test]
    fn no_violation_when_dynbox_typed_value_uses_dynbox() {
        let entry = BlockId(0);
        let mut blocks = HashMap::new();
        blocks.insert(
            entry,
            LirBlock {
                id: entry,
                args: vec![],
                ops: vec![LirOp {
                    tir_op: make_tir_op(OpCode::ConstNone, vec![], vec![ValueId(0)]),
                    result_values: vec![LirValue {
                        id: ValueId(0),
                        ty: TirType::DynBox,
                        repr: LirRepr::DynBox,
                    }],
                }],
                terminator: LirTerminator::Return {
                    values: vec![ValueId(0)],
                },
            },
        );
        let func = make_lir_function(blocks);
        let violations = verify_register_passable(&func);
        assert!(violations.is_empty(), "expected no violations: {violations:?}");
    }
}
