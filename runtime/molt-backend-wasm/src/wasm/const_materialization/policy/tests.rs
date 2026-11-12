use super::WasmConstOpPolicy;
use crate::OpIR;
use crate::wasm_abi_generated::{WasmConstInlineSeed, WasmConstRawIntEffect, WasmRuntimeImport};
use crate::wasm_values::{box_bool, box_int, box_none};
use molt_codegen_abi::box_float_bits as box_float;
use molt_tir::tir::op_kinds_generated::OwnedLiteralPayloadKind;

fn op(kind: molt_ir::OpKind) -> OpIR {
    OpIR::new(kind)
}

#[test]
fn const_policy_classifies_inline_seed_bits() {
    let mut int_op = op(molt_ir::OpKind::Const);
    int_op.value = Some(7);
    let mut bool_op = op(molt_ir::OpKind::ConstBool);
    bool_op.value = Some(1);
    let mut float_op = op(molt_ir::OpKind::ConstFloat);
    float_op.f_value = Some(1.5);
    let none_op = op(molt_ir::OpKind::ConstNone);

    assert_eq!(
        WasmConstOpPolicy::for_op(&int_op).map(|policy| policy.inline_seed()),
        Some(WasmConstInlineSeed::Int)
    );
    assert_eq!(
        WasmConstOpPolicy::for_op(&int_op).and_then(|policy| policy.inline_seed_bits(&int_op)),
        Some(box_int(7))
    );
    assert_eq!(
        WasmConstOpPolicy::for_op(&int_op).map(|policy| policy.raw_int_effect()),
        Some(WasmConstRawIntEffect::SetInt)
    );
    assert_eq!(
        WasmConstOpPolicy::for_op(&bool_op).map(|policy| policy.inline_seed()),
        Some(WasmConstInlineSeed::Bool)
    );
    assert_eq!(
        WasmConstOpPolicy::for_op(&bool_op).and_then(|policy| policy.inline_seed_bits(&bool_op)),
        Some(box_bool(1))
    );
    assert_eq!(
        WasmConstOpPolicy::for_op(&float_op).map(|policy| policy.inline_seed()),
        Some(WasmConstInlineSeed::Float)
    );
    assert_eq!(
        WasmConstOpPolicy::for_op(&float_op).and_then(|policy| policy.inline_seed_bits(&float_op)),
        Some(box_float(1.5))
    );
    assert_eq!(
        WasmConstOpPolicy::for_op(&none_op).map(|policy| policy.inline_seed()),
        Some(WasmConstInlineSeed::NoneValue)
    );
    assert_eq!(
        WasmConstOpPolicy::for_op(&none_op).and_then(|policy| policy.inline_seed_bits(&none_op)),
        Some(box_none())
    );
}

/// A known raw int stands for an inline int (its readers skip the tag guard
/// and box it with `box_int`), so a constant outside the inline range, such as
/// a product TIR folded, must leave its local unknown.
#[test]
fn only_inline_constants_become_known_raw_ints() {
    let mut locals = crate::wasm::WasmFrameLocals::new();
    locals.insert("v".to_string(), 0);
    let mut known_raw_ints = std::collections::BTreeMap::new();
    for (value, known) in [
        (molt_codegen_abi::INT_MAX_INLINE, true),
        (molt_codegen_abi::INT_MIN_INLINE, true),
        (molt_codegen_abi::INT_MAX_INLINE + 1, false),
        (molt_codegen_abi::INT_MIN_INLINE - 1, false),
        (3_037_000_499 * 3_037_000_499, false),
    ] {
        let mut const_op = op(molt_ir::OpKind::Const);
        const_op.value = Some(value);
        const_op.out = Some("v".to_string());
        // A stale entry from an earlier binding of the same local.
        known_raw_ints.insert(0, 1);
        WasmConstOpPolicy::for_op(&const_op)
            .expect("const policy")
            .apply_raw_int_effect(&const_op, &locals, &mut known_raw_ints);
        assert_eq!(
            known_raw_ints.get(&0).copied(),
            known.then_some(value),
            "constant {value}"
        );
    }
}

#[test]
fn const_policy_routes_full_i64_outside_inline_payload_to_runtime_anchor() {
    let mut op = op(molt_ir::OpKind::Const);
    op.value = Some(i64::MAX);
    let policy = WasmConstOpPolicy::for_op(&op).expect("const policy");

    assert_eq!(policy.inline_seed_bits(&op), None);
    assert_eq!(
        policy.materializer_import(),
        Some(WasmRuntimeImport::IntFromI64)
    );
    assert!(policy.needs_runtime_anchor());
    assert!(policy.materialization_can_fail());
}

#[test]
fn const_policy_classifies_runtime_seed_and_literal_payload() {
    for (kind, payload, import) in [
        (
            molt_ir::OpKind::ConstStr,
            OwnedLiteralPayloadKind::String,
            WasmRuntimeImport::StringFromBytes,
        ),
        (
            molt_ir::OpKind::ConstBigint,
            OwnedLiteralPayloadKind::BigintBytes,
            WasmRuntimeImport::BigintFromBytes,
        ),
        (
            molt_ir::OpKind::ConstBytes,
            OwnedLiteralPayloadKind::Bytes,
            WasmRuntimeImport::BytesFromBytes,
        ),
    ] {
        let policy = WasmConstOpPolicy::for_kind(kind).expect("literal const policy");
        assert_eq!(policy.literal_payload(), Some(payload));
        assert_eq!(policy.materializer_import(), Some(import));
        assert!(
            policy.needs_runtime_anchor(),
            "{kind} must be materialized into a function anchor"
        );
    }

    for kind in [
        molt_ir::OpKind::ConstNotImplemented,
        molt_ir::OpKind::ConstEllipsis,
    ] {
        let policy = WasmConstOpPolicy::for_kind(kind).expect("runtime singleton policy");
        assert_eq!(policy.literal_payload(), None);
        assert!(policy.materializer_import().is_some());
        assert!(
            policy.needs_runtime_anchor(),
            "{kind} must be materialized into a function anchor"
        );
    }
}

#[test]
fn const_policy_rejects_non_const_ops() {
    assert_eq!(WasmConstOpPolicy::for_kind(molt_ir::OpKind::Add), None);
    assert!("parse_int".parse::<molt_ir::OpKind>().is_err());
}

#[test]
#[should_panic(expected = "WASM const policy const requires int scalar payload")]
fn const_policy_fails_closed_on_missing_scalar_payload() {
    let const_op = op(molt_ir::OpKind::Const);
    let policy = WasmConstOpPolicy::for_op(&const_op).expect("const policy");

    let _ = policy.inline_seed_bits(&const_op);
}
