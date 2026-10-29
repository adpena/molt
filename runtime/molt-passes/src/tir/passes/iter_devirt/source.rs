use std::collections::{HashMap, HashSet};

use crate::tir::blocks::BlockId;
use crate::tir::function::TirFunction;
use crate::tir::ops::OpCode;
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

/// Infer the semantic list container shape for a value.
///
/// Input authority is restricted to function-owned TIR type facts and
/// structural list producers. Legacy `container_type` attrs are deliberately
/// ignored here: they are preserved as output metadata for downstream
/// compatibility, not accepted as proof that iterator devirtualization is
/// sound.
pub(super) fn infer_list_container_type(
    func: &TirFunction,
    source_val: ValueId,
    block_ids: &[BlockId],
) -> Option<String> {
    if matches!(func.value_types.get(&source_val), Some(TirType::List(_))) {
        return Some("list".to_string());
    }

    // Resolve each operand in O(1), rather than rescanning every operation at
    // each depth of a repeat chain. Preserve the supplied block/operation order:
    // the first definition wins, as in the original lookup.
    let mut definitions = HashMap::new();
    for &bid in block_ids {
        let Some(block) = func.blocks.get(&bid) else {
            continue;
        };
        for op in &block.ops {
            for &result in &op.results {
                definitions.entry(result).or_insert(op);
            }
        }
    }

    let mut seen = HashSet::new();
    let mut pending = vec![source_val];
    while let Some(value) = pending.pop() {
        if !seen.insert(value) {
            continue;
        }
        if matches!(func.value_types.get(&value), Some(TirType::List(_))) {
            return Some("list".to_string());
        }
        let Some(op) = definitions.get(&value) else {
            continue;
        };
        // BuildList always produces a generic list. A two-operand Mul inherits
        // its list shape from either operand; visit the left operand first and
        // retain the same global cycle guard as the recursive depth-first walk.
        if op.opcode == OpCode::BuildList {
            return Some("list".to_string());
        }
        if op.opcode == OpCode::Mul && op.operands.len() == 2 {
            pending.push(op.operands[1]);
            pending.push(op.operands[0]);
        }
    }
    None
}
