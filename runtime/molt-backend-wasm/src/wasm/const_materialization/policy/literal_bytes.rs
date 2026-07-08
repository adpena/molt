use super::WasmConstOpPolicy;
use crate::OpIR;
use crate::wasm_abi_generated::WasmConstLiteralPayload;
use molt_tir::tir::ops::{AttrValue, TirOp};
use std::sync::Arc;

impl WasmConstOpPolicy {
    pub(in crate::wasm::const_materialization::policy) fn required_simple_ir_literal_bytes(
        self,
        op: &OpIR,
    ) -> Arc<[u8]> {
        match self.literal_payload() {
            WasmConstLiteralPayload::None => {
                panic!("const op {} has no literal payload", self.0.kind)
            }
            WasmConstLiteralPayload::String => {
                if let Some(bytes) = op.bytes.as_deref() {
                    Arc::from(bytes)
                } else {
                    Arc::from(
                        op.s_value
                            .as_ref()
                            .unwrap_or_else(|| panic!("const_str requires s_value or bytes"))
                            .as_bytes(),
                    )
                }
            }
            WasmConstLiteralPayload::BigintDecimal => Arc::from(
                op.s_value
                    .as_ref()
                    .unwrap_or_else(|| panic!("const_bigint requires decimal s_value"))
                    .as_bytes(),
            ),
            WasmConstLiteralPayload::Bytes => Arc::from(
                op.bytes
                    .as_deref()
                    .unwrap_or_else(|| panic!("const_bytes requires bytes payload")),
            ),
        }
    }

    pub(in crate::wasm::const_materialization::policy) fn required_tir_literal_bytes(
        self,
        op: &TirOp,
    ) -> Arc<[u8]> {
        match self.literal_payload() {
            WasmConstLiteralPayload::None => {
                panic!("const op {} has no literal payload", self.0.kind)
            }
            WasmConstLiteralPayload::String => match op.attrs.get("bytes") {
                Some(AttrValue::Bytes(bytes)) => Arc::from(bytes.as_slice()),
                _ => Arc::from(required_tir_str_attr(op, "s_value", self.0.kind).as_bytes()),
            },
            WasmConstLiteralPayload::BigintDecimal => {
                Arc::from(required_tir_str_attr(op, "s_value", self.0.kind).as_bytes())
            }
            WasmConstLiteralPayload::Bytes => {
                Arc::from(required_tir_bytes_attr(op, "bytes", self.0.kind))
            }
        }
    }
}

fn required_tir_str_attr<'a>(op: &'a TirOp, attr: &str, kind: &str) -> &'a str {
    match op.attrs.get(attr) {
        Some(AttrValue::Str(value)) => value.as_str(),
        _ => panic!("WASM const policy {kind} requires string attr {attr}"),
    }
}

fn required_tir_bytes_attr<'a>(op: &'a TirOp, attr: &str, kind: &str) -> &'a [u8] {
    match op.attrs.get(attr) {
        Some(AttrValue::Bytes(value)) => value.as_slice(),
        _ => panic!("WASM const policy {kind} requires bytes attr {attr}"),
    }
}
