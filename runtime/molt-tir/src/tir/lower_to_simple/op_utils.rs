use std::collections::HashMap;

use super::names::value_var;
use crate::ir::OpIR;
use crate::tir::ops::{AttrDict, AttrValue, OpCode, TirOp};

// ---------------------------------------------------------------------------
// Structural annotation propagation
// ---------------------------------------------------------------------------

/// Annotate a SimpleIR [`OpIR`] with non-semantic transport metadata that is
/// still required by specific backend consumers.
pub(super) fn annotate_type_flags(opir: &mut OpIR, tir_op: &TirOp) {
    // Propagate StackAlloc: if the TIR op is StackAlloc, mark the SimpleIR op
    // so the native backend can emit stack allocation instead of heap allocation.
    // Also mark it as arena-eligible for the scope arena integration.
    if tir_op.opcode == OpCode::StackAlloc {
        opir.stack_eligible = Some(true);
        opir.arena_eligible = Some(true);
    }

    // Restore source-site coordinates for source/binary attribution and
    // traceback caret annotations. SourceSite is the only decoder for the
    // line/column attr family, so this boundary cannot drift on raw keys.
    if let Some(site) = tir_op.source_site() {
        opir.source_line.get_or_insert(site.line as i64);
        if let Some(col) = site.col {
            opir.col_offset.get_or_insert(col as i64);
        }
        if let Some(end_col) = site.end_col {
            opir.end_col_offset.get_or_insert(end_col as i64);
        }
    }
}

pub(super) fn annotate_lowered_op(
    opir: &mut OpIR,
    tir_op: &TirOp,
    original_to_new_label: &HashMap<i64, i64>,
) {
    annotate_type_flags(opir, tir_op);
    // Result-lifetime facts are TIR attrs, not opcode-local syntax. Preserve
    // them through every TIR -> SimpleIR custody boundary so native's
    // optimize-roundtrip -> terminal-drop relift sees the same finalizer facts
    // the frontend/SSA path proved. This covers object_new_bound, call_bind, and
    // any future result producer whose runtime class is proven to define __del__.
    if !tir_op.results.is_empty()
        && matches!(tir_op.attrs.get("defines_del"), Some(AttrValue::Bool(true)))
    {
        opir.defines_del = Some(true);
    }
    if matches!(
        opir.kind.as_str(),
        "check_exception" | "try_start" | "try_end"
    ) && let Some(orig_id) = opir.value
        && let Some(&new_id) = original_to_new_label.get(&orig_id)
    {
        opir.value = Some(new_id);
    }
}

// ---------------------------------------------------------------------------
// Helper utilities
// ---------------------------------------------------------------------------

pub(super) fn binary_op(kind: &str, op: &TirOp, out: Option<String>) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        args: Some(operand_args(op)),
        out,
        ..OpIR::default()
    }
}

pub(super) fn unary_op(kind: &str, op: &TirOp, out: Option<String>) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        args: Some(operand_args(op)),
        out,
        ..OpIR::default()
    }
}

pub(super) fn operand_args(op: &TirOp) -> Vec<String> {
    op.operands.iter().map(|v| value_var(*v)).collect()
}

pub(super) fn attr_int(attrs: &AttrDict, key: &str) -> Option<i64> {
    match attrs.get(key) {
        Some(AttrValue::Int(i)) => Some(*i),
        _ => None,
    }
}

pub(super) fn attr_float(attrs: &AttrDict, key: &str) -> Option<f64> {
    match attrs.get(key) {
        Some(AttrValue::Float(f)) => Some(*f),
        _ => None,
    }
}

pub(super) fn attr_str(attrs: &AttrDict, key: &str) -> Option<String> {
    match attrs.get(key) {
        Some(AttrValue::Str(s)) => Some(s.clone()),
        _ => None,
    }
}

/// TIR results and SimpleIR stream outputs are separate: zero-result
/// side-effect ops may still carry `_simple_out` for round-trip fidelity.
pub(super) fn result_or_stream_out(op: &TirOp, result_out: Option<String>) -> Option<String> {
    result_out.or_else(|| attr_str(&op.attrs, "_simple_out"))
}

pub(super) fn attr_bool(attrs: &AttrDict, key: &str) -> Option<bool> {
    match attrs.get(key) {
        Some(AttrValue::Bool(b)) => Some(*b),
        _ => None,
    }
}

pub(super) fn attr_bytes(attrs: &AttrDict, key: &str) -> Option<Vec<u8>> {
    match attrs.get(key) {
        Some(AttrValue::Bytes(b)) => Some(b.clone()),
        _ => None,
    }
}
