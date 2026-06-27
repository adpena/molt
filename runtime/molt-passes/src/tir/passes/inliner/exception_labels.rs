use std::collections::{BTreeSet, HashMap};

use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::opcode_has_exception_label_attr_table;
use crate::tir::ops::{AttrDict, AttrValue, OpCode, TirOp};

/// The opcodes whose `"value"` attribute is a SimpleIR **label id** naming an
/// exception/handler target block (read by [`crate::tir::dominators::exception_successors`]
/// and re-emitted by `lower_to_simple`). These are the only in-block ops that
/// reference a block by label rather than by `BlockId`, so they are the only ops
/// whose attrs need label remapping when a body is cloned.
fn is_exception_label_op(opcode: OpCode) -> bool {
    opcode_has_exception_label_attr_table(opcode)
}

/// Read the label id from an exception op's `"value"` attr, if present.
pub(super) fn exception_label_of(op: &TirOp) -> Option<i64> {
    if !is_exception_label_op(op.opcode) {
        return None;
    }
    match op.attrs.get("value") {
        Some(AttrValue::Int(label)) => Some(*label),
        _ => None,
    }
}

/// The set of SimpleIR label ids `func` uses: the union of every `label_id_map`
/// value and every exception op's `"value"` label. `label_id_map` already covers
/// every label-bearing block, but the exception-op attrs are unioned in so a
/// callee whose exception target is (defensively) missing from `label_id_map`
/// still gets a fresh remap rather than an accidental passthrough collision.
pub(super) fn function_label_ids(func: &TirFunction) -> BTreeSet<i64> {
    let mut labels: BTreeSet<i64> = func.label_id_map.values().copied().collect();
    for block in func.blocks.values() {
        for op in &block.ops {
            if let Some(label) = exception_label_of(op) {
                labels.insert(label);
            }
        }
    }
    labels
}

/// Build the callee→fresh label remap for one clone. Every label the callee uses
/// is reassigned to a fresh id strictly greater than every label currently in the
/// caller, so the cloned body's exception labels cannot collide with the caller's
/// (or with the fresh labels of callees already inlined into this caller — those
/// were inserted into `caller.label_id_map`, so the caller's max grows with each
/// clone). Deterministic: callee labels are processed in ascending order.
pub(super) fn build_label_remap(callee: &TirFunction, caller: &TirFunction) -> HashMap<i64, i64> {
    let callee_labels = function_label_ids(callee);
    if callee_labels.is_empty() {
        return HashMap::new();
    }
    let caller_max = function_label_ids(caller).iter().copied().max();
    // Start strictly above the caller's max (or at 0 if the caller has no labels).
    let start = caller_max.map(|m| m + 1).unwrap_or(0);
    let mut remap = HashMap::with_capacity(callee_labels.len());
    // Callee labels are processed in ascending order; each gets the next id
    // counting up from `start` (the `start..` range supplies the counter).
    for (label, next) in callee_labels.into_iter().zip(start..) {
        remap.insert(label, next);
    }
    remap
}

/// Rewrite a cloned exception op's `"value"` label attr through `label_remap`.
/// A non-exception op, or an exception label not present in the remap, is left
/// untouched (a missing remap entry can only happen for a label the callee did
/// not actually declare, which `function_label_ids` already folds in, so in
/// practice every cloned exception label is remapped).
pub(super) fn remap_exception_label_attr(
    opcode: OpCode,
    attrs: &mut AttrDict,
    label_remap: &HashMap<i64, i64>,
) {
    if !is_exception_label_op(opcode) {
        return;
    }
    if let Some(AttrValue::Int(old_label)) = attrs.get("value")
        && let Some(&new_label) = label_remap.get(old_label)
    {
        attrs.insert("value".into(), AttrValue::Int(new_label));
    }
}
