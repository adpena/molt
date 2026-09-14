use super::op_kinds_generated::{SimpleIrCallTargetRole, simpleir_call_target_role};
use super::ops::{AttrValue, OpCode, TirOp};
use super::types::TirType;

/// One identity/result authority for lifting and every TIR GPU call consumer.
const GPU_RUNTIME_INTRINSICS: &[(&str, &str, TirType)] = &[
    ("gpu_thread_id", "molt_gpu_thread_id", TirType::I64),
    ("gpu_block_id", "molt_gpu_block_id", TirType::I64),
    ("gpu_block_dim", "molt_gpu_block_dim", TirType::I64),
    ("gpu_grid_dim", "molt_gpu_grid_dim", TirType::I64),
    ("gpu_barrier", "molt_gpu_barrier", TirType::None),
];

/// Runtime helper symbol produced when SimpleIR GPU intrinsics are lifted into
/// first-class TIR `Call` ops.
pub fn gpu_runtime_symbol_for_simple_kind(kind: &str) -> Option<&'static str> {
    GPU_RUNTIME_INTRINSICS
        .iter()
        .find_map(|(source, symbol, _)| (*source == kind).then_some(*symbol))
}

/// A statically named call target, never an opaque call's incidental string.
/// Absent transport metadata denotes a canonical first-class TIR `Call`.
/// Preserved SimpleIR spellings must prove a direct role through the generated
/// schema, or identify the matching fixed GPU intrinsic. A registered direct
/// call transported through `Copy` retains its target, but a plain Copy never
/// gains call identity.
pub fn direct_call_symbol_for_op(op: &TirOp) -> Option<&str> {
    if !matches!(op.opcode, OpCode::Call | OpCode::Copy) || !op.has_valid_shape() {
        return None;
    }
    let AttrValue::Str(symbol) = op.attrs.get("s_value")? else {
        return None;
    };
    if symbol.is_empty() {
        return None;
    }
    match op.attrs.get("_original_kind") {
        None => (op.opcode == OpCode::Call).then_some(symbol.as_str()),
        Some(AttrValue::Str(kind)) => match simpleir_call_target_role(kind) {
            Some(
                SimpleIrCallTargetRole::ExternalOrRuntime
                | SimpleIrCallTargetRole::InternalRequired,
            ) => Some(symbol),
            Some(SimpleIrCallTargetRole::Opaque) => None,
            None => (op.opcode == OpCode::Call
                && gpu_runtime_symbol_for_simple_kind(kind) == Some(symbol.as_str()))
            .then_some(symbol.as_str()),
        },
        Some(_) => None,
    }
}

/// Exact result shape of a fixed GPU call, independent of annotations.
/// GPU helpers have at most one captured result; variable-result `Call` schema
/// admission alone cannot authorize extra slots. Opaque calls never gain
/// intrinsic identity from their symbol spelling.
pub fn gpu_runtime_result_type_for_op(op: &TirOp) -> Option<TirType> {
    if op.opcode != OpCode::Call || op.results.len() > 1 {
        return None;
    }
    // Every genuine GPU lift preserves its source spelling. Requiring that
    // explicit provenance prevents an ordinary module call whose function name
    // happens to equal a runtime helper from minting a fixed scalar result.
    let AttrValue::Str(kind) = op.attrs.get("_original_kind")? else {
        return None;
    };
    let symbol = direct_call_symbol_for_op(op)?;
    GPU_RUNTIME_INTRINSICS
        .iter()
        .find_map(|(source, runtime, ty)| {
            (*source == kind && *runtime == symbol).then(|| ty.clone())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::ops::{AttrDict, Dialect};
    use crate::tir::values::ValueId;

    fn call(symbol: &str, original: Option<&str>) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert("s_value".into(), AttrValue::Str(symbol.into()));
        if let Some(kind) = original {
            attrs.insert("_original_kind".into(), AttrValue::Str(kind.into()));
        }
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands: vec![],
            results: vec![ValueId(0)],
            attrs,
            source_span: None,
        }
    }

    #[test]
    fn gpu_simple_kinds_map_to_runtime_symbols() {
        for (kind, symbol, result_type) in [
            ("gpu_thread_id", "molt_gpu_thread_id", TirType::I64),
            ("gpu_block_id", "molt_gpu_block_id", TirType::I64),
            ("gpu_block_dim", "molt_gpu_block_dim", TirType::I64),
            ("gpu_grid_dim", "molt_gpu_grid_dim", TirType::I64),
            ("gpu_barrier", "molt_gpu_barrier", TirType::None),
        ] {
            assert_eq!(gpu_runtime_symbol_for_simple_kind(kind), Some(symbol));
            let intrinsic = call(symbol, Some(kind));
            assert_eq!(direct_call_symbol_for_op(&intrinsic), Some(symbol));
            assert_eq!(
                gpu_runtime_result_type_for_op(&intrinsic),
                Some(result_type)
            );
            for original in [None, Some("call"), Some("call_internal")] {
                let op = call(symbol, original);
                assert_eq!(direct_call_symbol_for_op(&op), Some(symbol));
                assert_eq!(gpu_runtime_result_type_for_op(&op), None);
            }
        }
        assert_eq!(gpu_runtime_symbol_for_simple_kind("call"), None);
        assert_eq!(
            gpu_runtime_result_type_for_op(&call("user_function", None)),
            None
        );
    }

    #[test]
    fn opaque_original_kinds_cannot_mint_fixed_gpu_facts() {
        for kind in [
            "call_func",
            "call_function",
            "call_indirect",
            "call_bind",
            "call_guarded",
            "invoke_ffi",
            "future_unknown_call",
        ] {
            for symbol in ["molt_gpu_thread_id", "molt_gpu_barrier", "module_function"] {
                let op = call(symbol, Some(kind));
                assert_eq!(direct_call_symbol_for_op(&op), None, "{kind}: {symbol}");
                assert_eq!(
                    gpu_runtime_result_type_for_op(&op),
                    None,
                    "{kind}: {symbol}"
                );
            }
        }
    }

    #[test]
    fn copy_transport_requires_a_registered_direct_call_role() {
        for original in [
            None,
            Some("copy"),
            Some("call_func"),
            Some("call"),
            Some("call_internal"),
            Some("gpu_thread_id"),
        ] {
            let mut op = call("molt_gpu_thread_id", original);
            op.opcode = OpCode::Copy;
            let direct = matches!(original, Some("call" | "call_internal"));
            assert_eq!(
                direct_call_symbol_for_op(&op).is_some(),
                direct,
                "{original:?}"
            );
            assert_eq!(gpu_runtime_result_type_for_op(&op), None, "{original:?}");
        }
    }

    #[test]
    fn gpu_identity_rejects_mismatched_metadata_opcode_and_result_shape() {
        let internal = call("molt_gpu_thread_id", Some("call_internal"));
        assert_eq!(
            direct_call_symbol_for_op(&internal),
            Some("molt_gpu_thread_id")
        );
        assert_eq!(gpu_runtime_result_type_for_op(&internal), None);
        let mut op = call("molt_gpu_thread_id", Some("gpu_barrier"));
        assert_eq!(gpu_runtime_result_type_for_op(&op), None);
        op.attrs.insert("_original_kind".into(), AttrValue::Int(1));
        assert_eq!(gpu_runtime_result_type_for_op(&op), None);
        op.attrs.remove("_original_kind");
        op.opcode = OpCode::Copy;
        assert_eq!(gpu_runtime_result_type_for_op(&op), None);
        op.opcode = OpCode::Call;
        op.results.push(ValueId(1));
        assert_eq!(gpu_runtime_result_type_for_op(&op), None);
        op.results.clear();
        assert_eq!(gpu_runtime_result_type_for_op(&op), None);
        op.attrs.insert(
            "_original_kind".into(),
            AttrValue::Str("gpu_thread_id".into()),
        );
        assert_eq!(gpu_runtime_result_type_for_op(&op), Some(TirType::I64));
    }
}
