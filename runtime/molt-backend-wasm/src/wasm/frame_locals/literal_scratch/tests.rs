use super::{WasmFrameLocalKind, WasmFrameLocals};
use crate::wasm::const_materialization::WasmConstOpPolicy;
use crate::wasm_abi_generated::WasmConstLiteralPayload;
use wasm_encoder::ValType;

#[test]
fn literal_scratch_locals_are_owned_and_reused_by_frame_locals() {
    let mut locals = WasmFrameLocals::new();
    let mut local_types = Vec::new();
    let mut local_count = 0;

    let first = locals.ensure_literal_scratch(
        "payload",
        WasmConstLiteralPayload::String,
        true,
        &mut local_types,
        &mut local_count,
    );
    let second = locals.ensure_literal_scratch(
        "payload",
        WasmConstLiteralPayload::String,
        true,
        &mut local_types,
        &mut local_count,
    );
    let looked_up = locals.literal_scratch("payload");
    let maybe_lookup = locals.try_literal_scratch("payload");
    let parse_lookup = locals.try_parse_scalar_literal_scratch("payload");

    assert_eq!(first.ptr_local(), 0);
    assert_eq!(first.len_local(), 1);
    assert_eq!(first.payload(), WasmConstLiteralPayload::String);
    assert!(first.parse_scalar_eligible());
    assert_eq!(second.ptr_local(), first.ptr_local());
    assert_eq!(second.len_local(), first.len_local());
    assert_eq!(looked_up.ptr_local(), first.ptr_local());
    assert_eq!(looked_up.len_local(), first.len_local());
    assert_eq!(maybe_lookup.map(|scratch| scratch.ptr_local()), Some(0));
    assert_eq!(parse_lookup.map(|scratch| scratch.len_local()), Some(1));
    assert!(locals.try_literal_scratch("missing").is_none());
    assert!(locals.try_parse_scalar_literal_scratch("missing").is_none());
    assert_eq!(
        locals.local_kind("payload_ptr"),
        Some(WasmFrameLocalKind::LiteralScratchPtr)
    );
    assert_eq!(
        locals.local_kind("payload_len"),
        Some(WasmFrameLocalKind::LiteralScratchLen)
    );
    assert!(
        locals
            .named_locals()
            .find(|local| local.name() == "payload_ptr")
            .is_some_and(|local| local.kind().is_call_retention_exempt())
    );
    assert!(
        locals
            .named_locals()
            .find(|local| local.name() == "payload_len")
            .is_some_and(|local| local.kind().is_call_retention_exempt())
    );
    assert_eq!(local_types, vec![ValType::I64, ValType::I64]);
    assert_eq!(local_count, 2);
}

#[test]
fn literal_scratch_policy_controls_scalar_parse_eligibility() {
    let mut locals = WasmFrameLocals::new();
    let mut local_types = Vec::new();
    let mut local_count = 0;

    let string_scratch = locals
        .ensure_literal_scratch_for_policy(
            "text",
            WasmConstOpPolicy::for_kind("const_str").expect("const_str policy"),
            &mut local_types,
            &mut local_count,
        )
        .expect("const_str should allocate literal scratch");
    let bigint_scratch = locals
        .ensure_literal_scratch_for_policy(
            "digits",
            WasmConstOpPolicy::for_kind("const_bigint").expect("const_bigint policy"),
            &mut local_types,
            &mut local_count,
        )
        .expect("const_bigint should allocate literal scratch");
    let bytes_scratch = locals
        .ensure_literal_scratch_for_policy(
            "blob",
            WasmConstOpPolicy::for_kind("const_bytes").expect("const_bytes policy"),
            &mut local_types,
            &mut local_count,
        )
        .expect("const_bytes should allocate literal scratch");
    let none_scratch = locals.ensure_literal_scratch_for_policy(
        "none",
        WasmConstOpPolicy::for_kind("const_none").expect("const_none policy"),
        &mut local_types,
        &mut local_count,
    );

    assert_eq!(string_scratch.payload(), WasmConstLiteralPayload::String);
    assert!(string_scratch.parse_scalar_eligible());
    assert_eq!(
        bigint_scratch.payload(),
        WasmConstLiteralPayload::BigintDecimal
    );
    assert!(!bigint_scratch.parse_scalar_eligible());
    assert_eq!(bytes_scratch.payload(), WasmConstLiteralPayload::Bytes);
    assert!(bytes_scratch.parse_scalar_eligible());
    assert!(none_scratch.is_none());
    assert!(locals.try_parse_scalar_literal_scratch("text").is_some());
    assert!(locals.try_parse_scalar_literal_scratch("blob").is_some());
    assert!(locals.try_literal_scratch("digits").is_some());
    assert!(locals.try_parse_scalar_literal_scratch("digits").is_none());
    assert_eq!(
        locals
            .try_literal_scratch("digits")
            .map(|scratch| scratch.payload()),
        Some(WasmConstLiteralPayload::BigintDecimal)
    );
    assert_eq!(
        local_types,
        vec![
            ValType::I64,
            ValType::I64,
            ValType::I64,
            ValType::I64,
            ValType::I64,
            ValType::I64,
        ]
    );
    assert_eq!(local_count, 6);
}
