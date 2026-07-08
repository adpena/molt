use super::super::{WasmConstMaterialization, WasmConstMaterializationScratch};
use super::WasmConstOpPolicy;
use crate::OpIR;
use crate::wasm::WasmFrameLocals;
use crate::wasm_abi_generated::{WasmConstLiteralPayload, WasmRuntimeImport};
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
        let out_local = locals[out_name];
        match self.literal_payload() {
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
