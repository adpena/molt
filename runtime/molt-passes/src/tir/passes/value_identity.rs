//! Shared value-identity facts for TIR passes.
//!
//! `OpCode::Copy` is both a structural SSA move and the fallback carrier for
//! SimpleIR spellings that have not become first-class TIR opcodes. This module
//! owns the fail-closed predicate for the subset of Copy ops that truly forward
//! one SSA value unchanged, so loop recognition, range analysis, representation
//! planning, and loop transforms cannot drift.

use std::collections::HashMap;

use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::values::ValueId;

/// `Some(src)` when `op` is a `Copy` that holds the same value as one source.
///
/// Accepted cases are a plain SSA copy or a Copy carrying a known
/// value-forwarding SimpleIR spelling. Every operand must name the same source
/// value, so multi-operand stack-machine spellings such as `Copy(v, v)` remain
/// transparent while opaque or aggregating Copy fallbacks fail closed.
pub(crate) fn copy_value_source(op: &TirOp) -> Option<ValueId> {
    if op.opcode != OpCode::Copy || op.results.len() != 1 || op.operands.is_empty() {
        return None;
    }
    let kind_ok = match op.attrs.get("_original_kind") {
        None => true,
        Some(AttrValue::Str(k)) => matches!(
            k.as_str(),
            "copy" | "copy_var" | "store_var" | "load_var" | "identity_alias"
        ),
        Some(_) => false,
    };
    if !kind_ok {
        return None;
    }
    let src = op.operands[0];
    op.operands
        .iter()
        .all(|&operand| operand == src)
        .then_some(src)
}

/// Build a transitive copy-resolution map over value-forwarding Copy ops.
pub(crate) fn build_copy_map(func: &TirFunction) -> HashMap<ValueId, ValueId> {
    let mut copy_of: HashMap<ValueId, ValueId> = HashMap::new();
    for block in func.blocks.values() {
        for op in &block.ops {
            if let Some(src) = copy_value_source(op) {
                copy_of.insert(op.results[0], src);
            }
        }
    }
    flatten_copy_map(&mut copy_of);
    copy_of
}

fn flatten_copy_map(copy_of: &mut HashMap<ValueId, ValueId>) {
    for _ in 0..64 {
        let mut changed = false;
        let keys: Vec<ValueId> = copy_of.keys().copied().collect();
        for key in keys {
            let value = copy_of[&key];
            if let Some(&deeper) = copy_of.get(&value)
                && deeper != value
            {
                copy_of.insert(key, deeper);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

/// Resolve `value` through a copy map to its root source.
pub(crate) fn resolve_copy(copy_of: &HashMap<ValueId, ValueId>, value: ValueId) -> ValueId {
    copy_of.get(&value).copied().unwrap_or(value)
}
