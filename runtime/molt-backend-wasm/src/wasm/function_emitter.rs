mod fallback;
mod plain;
mod registration;

use super::WasmBackend;
use super::context::CompileFuncContext;
use crate::FunctionIR;
use crate::wasm::lir_fast::try_emit_planned_lir_fast_body;

impl WasmBackend {
    pub(super) fn compile_func(
        &mut self,
        func_ir: &FunctionIR,
        type_idx: u32,
        ctx: &CompileFuncContext<'_>,
    ) {
        let reloc_enabled = ctx.reloc_enabled;
        let func_index = registration::register_function(self, func_ir, type_idx, reloc_enabled);
        if try_emit_planned_lir_fast_body(self, func_ir, func_index, reloc_enabled, ctx) {
            return;
        }
        fallback::emit_fallback_function_body(self, func_ir, func_index, ctx);
    }
}
