use super::super::{WasmConstMaterialization, WasmConstMaterializationScratch};
use super::WasmConstOpPolicy;
use crate::OpIR;
use crate::wasm::WasmFrameLocals;
use crate::wasm_abi_generated::{
    WasmConstInlineSeed, WasmConstLiteralPayload, WasmConstScalarValue, WasmRuntimeImport,
};
use molt_tir::tir::ops::TirOp;

impl WasmConstOpPolicy {
    pub(in crate::wasm) fn simple_ir_materialization(
        self,
        op: &OpIR,
        locals: &WasmFrameLocals,
    ) -> WasmConstMaterialization {
        let out_name = op
            .out
            .as_ref()
            .unwrap_or_else(|| panic!("const op {} requires an output", self.0.kind));
        self.simple_ir_materialization_into(op, locals, locals[out_name])
    }

    pub(in crate::wasm) fn simple_ir_materialization_into(
        self,
        op: &OpIR,
        locals: &WasmFrameLocals,
        out_local: u32,
    ) -> WasmConstMaterialization {
        let out_name = op
            .out
            .as_ref()
            .unwrap_or_else(|| panic!("const op {} requires an output", self.0.kind));
        match self.literal_payload() {
            WasmConstLiteralPayload::None
                if matches!(self.inline_seed(), WasmConstInlineSeed::Int) =>
            {
                WasmConstMaterialization::scalar_i64(
                    self.required_materializer_import(),
                    out_local,
                    op.value.unwrap_or_else(|| {
                        panic!("const op {} requires an i64 payload", self.0.kind)
                    }),
                )
            }
            WasmConstLiteralPayload::None => WasmConstMaterialization::runtime_singleton(
                self.required_materializer_import(),
                out_local,
            ),
            payload => WasmConstMaterialization::literal(
                self.required_materializer_import(),
                out_local,
                payload,
                self.required_simple_ir_literal_bytes(op),
                locals.literal_scratch(out_name).into(),
            ),
        }
    }

    pub(in crate::wasm) fn tir_materialization(
        self,
        op: &TirOp,
        out_local: u32,
        scratch: Option<WasmConstMaterializationScratch>,
    ) -> WasmConstMaterialization {
        match self.literal_payload() {
            WasmConstLiteralPayload::None
                if matches!(self.inline_seed(), WasmConstInlineSeed::Int) =>
            {
                let value = match self.required_tir_scalar_value(op) {
                    WasmConstScalarValue::Int(value) => value,
                    other => panic!(
                        "const op {} requires an i64 payload, got {other:?}",
                        self.0.kind
                    ),
                };
                WasmConstMaterialization::scalar_i64(
                    self.required_materializer_import(),
                    out_local,
                    value,
                )
            }
            WasmConstLiteralPayload::None => WasmConstMaterialization::runtime_singleton(
                self.required_materializer_import(),
                out_local,
            ),
            payload => WasmConstMaterialization::literal(
                self.required_materializer_import(),
                out_local,
                payload,
                self.required_tir_literal_bytes(op),
                scratch.unwrap_or_else(|| {
                    panic!("const op {} requires literal scratch locals", self.0.kind)
                }),
            ),
        }
    }

    fn required_materializer_import(self) -> WasmRuntimeImport {
        self.materializer_import()
            .unwrap_or_else(|| panic!("const op {} has no materializer import", self.0.kind))
    }
}
