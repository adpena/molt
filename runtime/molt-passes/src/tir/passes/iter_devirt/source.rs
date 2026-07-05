use std::collections::HashSet;

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
    let mut seen = HashSet::new();
    infer_list_container_type_inner(func, source_val, block_ids, &mut seen)
}

fn infer_list_container_type_inner(
    func: &TirFunction,
    source_val: ValueId,
    block_ids: &[BlockId],
    seen: &mut HashSet<ValueId>,
) -> Option<String> {
    if !seen.insert(source_val) {
        return None;
    }

    if let Some(ty) = func.value_types.get(&source_val)
        && matches!(ty, TirType::List(_))
    {
        return Some("list".to_string());
    }

    for &bid in block_ids {
        let Some(block) = func.blocks.get(&bid) else {
            continue;
        };
        for op in &block.ops {
            if !op.results.contains(&source_val) {
                continue;
            }
            // BuildList always produces a generic list.
            if op.opcode == OpCode::BuildList {
                return Some("list".to_string());
            }
            // Mul(BuildList, count) is a list repeat — inherits from operand.
            if op.opcode == OpCode::Mul && op.operands.len() == 2 {
                let (a, b) = (op.operands[0], op.operands[1]);
                if let Some(ct) = infer_list_container_type_inner(func, a, block_ids, seen) {
                    return Some(ct);
                }
                if let Some(ct) = infer_list_container_type_inner(func, b, block_ids, seen) {
                    return Some(ct);
                }
            }
            return None;
        }
    }
    None
}

/// Determine if a value is known to be a list from typed facts or its defining op.
pub(super) fn is_list_source(
    func: &TirFunction,
    source_val: ValueId,
    block_ids: &[BlockId],
) -> bool {
    infer_list_container_type(func, source_val, block_ids).is_some()
}
