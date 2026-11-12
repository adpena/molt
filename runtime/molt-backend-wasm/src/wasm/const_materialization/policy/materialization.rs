use super::super::WasmConstMaterialization;
use super::WasmConstOpPolicy;
use crate::OpIR;
use crate::wasm::WasmFrameLocals;
use crate::wasm_abi_generated::{WasmConstInlineSeed, WasmRuntimeImport};

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
        self.simple_ir_materialization_into(op, locals.result_slot(out_name))
    }

    pub(in crate::wasm) fn simple_ir_materialization_into(
        self,
        op: &OpIR,
        out_local: u32,
    ) -> WasmConstMaterialization {
        match self.literal_payload() {
            None if matches!(self.inline_seed(), WasmConstInlineSeed::Int) => {
                WasmConstMaterialization::scalar_i64(
                    self.required_materializer_import(),
                    out_local,
                    op.value.unwrap_or_else(|| {
                        panic!("const op {} requires an i64 payload", self.0.kind)
                    }),
                )
            }
            None => WasmConstMaterialization::runtime_singleton(
                self.required_materializer_import(),
                out_local,
            ),
            Some(payload) => WasmConstMaterialization::literal(
                self.required_materializer_import(),
                out_local,
                payload,
                self.required_simple_ir_literal_bytes(op),
            ),
        }
    }

    fn required_materializer_import(self) -> WasmRuntimeImport {
        self.materializer_import()
            .unwrap_or_else(|| panic!("const op {} has no materializer import", self.0.kind))
    }
}
