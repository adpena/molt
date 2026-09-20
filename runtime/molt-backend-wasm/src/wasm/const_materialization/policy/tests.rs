use super::WasmConstOpPolicy;
use crate::OpIR;
use crate::wasm_abi_generated::{
    WasmConstInlineSeed, WasmConstLirFastPolicy, WasmConstLiteralPayload, WasmConstRawIntEffect,
    WasmRuntimeImport,
};
use crate::wasm_values::{box_bool, box_int, box_none};
use molt_codegen_abi::box_float_bits as box_float;

fn op(kind: &str) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        ..OpIR::default()
    }
}

#[test]
fn const_policy_classifies_inline_seed_bits() {
    let mut int_op = op("const");
    int_op.value = Some(7);
    let mut bool_op = op("const_bool");
    bool_op.value = Some(1);
    let mut float_op = op("const_float");
    float_op.f_value = Some(1.5);
    let none_op = op("const_none");

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

#[test]
fn const_policy_routes_full_i64_outside_inline_payload_to_runtime_anchor() {
    let mut op = op("const");
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
fn const_policy_classifies_runtime_seed_and_literal_scratch() {
    for (kind, payload, import, lir_policy) in [
        (
            "const_str",
            WasmConstLiteralPayload::String,
            WasmRuntimeImport::StringFromBytes,
            WasmConstLirFastPolicy::Materialize,
        ),
        (
            "const_bigint",
            WasmConstLiteralPayload::BigintDecimal,
            WasmRuntimeImport::BigintFromStr,
            WasmConstLirFastPolicy::Materialize,
        ),
        (
            "const_bytes",
            WasmConstLiteralPayload::Bytes,
            WasmRuntimeImport::BytesFromBytes,
            WasmConstLirFastPolicy::Materialize,
        ),
    ] {
        let policy = WasmConstOpPolicy::for_kind(kind).expect("literal const policy");
        assert!(
            policy.needs_literal_scratch(),
            "{kind} must allocate literal scratch"
        );
        assert_eq!(policy.literal_payload(), payload);
        assert_eq!(policy.materializer_import(), Some(import));
        assert_eq!(policy.lir_fast_policy(), lir_policy);
        assert!(
            policy.needs_runtime_anchor(),
            "{kind} must be materialized into a function anchor"
        );
    }

    for kind in ["const_not_implemented", "const_ellipsis"] {
        let policy = WasmConstOpPolicy::for_kind(kind).expect("runtime singleton policy");
        assert!(
            !policy.needs_literal_scratch(),
            "{kind} must not allocate literal scratch"
        );
        assert_eq!(policy.literal_payload(), WasmConstLiteralPayload::None);
        assert!(policy.materializer_import().is_some());
        assert_eq!(
            policy.lir_fast_policy(),
            WasmConstLirFastPolicy::Materialize
        );
        assert!(
            policy.needs_runtime_anchor(),
            "{kind} must be materialized into a function anchor"
        );
    }
}

#[test]
fn const_policy_rejects_non_const_ops() {
    assert_eq!(WasmConstOpPolicy::for_kind("add"), None);
    assert_eq!(WasmConstOpPolicy::for_kind("parse_int"), None);
}

#[test]
#[should_panic(expected = "WASM const policy const requires int scalar payload")]
fn const_policy_fails_closed_on_missing_scalar_payload() {
    let const_op = op("const");
    let policy = WasmConstOpPolicy::for_op(&const_op).expect("const policy");

    let _ = policy.inline_seed_bits(&const_op);
}
