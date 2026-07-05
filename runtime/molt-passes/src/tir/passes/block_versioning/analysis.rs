use std::collections::HashMap;

use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::opcode_operand_independent_result_tir_type;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

/// A type context: the guarded value and the type the guard proves.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct TypeContext {
    /// The SSA value being guarded (operands[0] of the TypeGuard).
    pub(super) guarded_value: ValueId,
    /// The type that the guard proves (parsed from the "ty" attr).
    pub(super) proven_type: TirType,
}

/// Information about a TypeGuard candidate in a block.
#[derive(Debug)]
pub(super) struct GuardCandidate {
    /// Index of the TypeGuard op within the block's ops vector.
    pub(super) op_index: usize,
    /// The type context this guard establishes.
    pub(super) context: TypeContext,
    /// The result ValueId of the TypeGuard (the bool flag).
    pub(super) result: ValueId,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse the proven type from a TypeGuard op's attributes.
/// Returns None if the attributes don't contain a parseable type string.
pub(super) fn parse_guard_type(op: &TirOp) -> Option<TirType> {
    // The type_guard_hoist tests use "ty", the deopt module uses "expected_type".
    // Check both, preferring "ty".
    let type_str = op.attrs.get("ty").or_else(|| op.attrs.get("expected_type"));

    match type_str {
        Some(AttrValue::Str(s)) => match s.to_uppercase().as_str() {
            "INT" | "I64" => Some(TirType::I64),
            "FLOAT" | "F64" => Some(TirType::F64),
            "BOOL" => Some(TirType::Bool),
            "STR" => Some(TirType::Str),
            "NONE" => Some(TirType::None),
            "BYTES" => Some(TirType::Bytes),
            _ => None,
        },
        _ => None,
    }
}

/// Build a map: ValueId -> OpCode that produced it (for ops, not block args).
pub(super) fn build_producing_op_map(func: &TirFunction) -> HashMap<ValueId, OpCode> {
    let mut map: HashMap<ValueId, OpCode> = HashMap::new();
    for block in func.blocks.values() {
        for op in &block.ops {
            for &result in &op.results {
                map.insert(result, op.opcode);
            }
        }
    }
    map
}

/// Returns true if a value is statically known to be of the given type,
/// based on the opcode that produced it.
pub(super) fn value_proves_type(
    value: ValueId,
    expected: &TirType,
    producing_ops: &HashMap<ValueId, OpCode>,
    block_arg_types: &HashMap<ValueId, TirType>,
) -> bool {
    // Check block argument types first.
    if let Some(ty) = block_arg_types.get(&value) {
        return ty == expected;
    }

    // Check the producing opcode against the generated intrinsic-result table.
    // Operand-dependent arithmetic stays out of this fast proof path; type_refine
    // owns those proofs after it has operand facts.
    let opcode = match producing_ops.get(&value) {
        Some(op) => op,
        None => return false,
    };

    opcode_operand_independent_result_tir_type(*opcode).is_some_and(|ty| &ty == expected)
}
