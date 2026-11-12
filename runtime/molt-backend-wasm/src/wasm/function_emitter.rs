mod fallback;
mod plain;
mod registration;

use super::WasmBackend;
use super::context::CompileFuncContext;
use crate::FunctionIR;

impl WasmBackend {
    pub(super) fn compile_func(
        &mut self,
        func_ir: &FunctionIR,
        facts: super::final_functions::WasmFunctionFacts,
        type_idx: u32,
        ctx: &CompileFuncContext<'_>,
    ) {
        let reloc_enabled = ctx.reloc_enabled;
        let func_index = registration::register_function(self, func_ir, type_idx, reloc_enabled);
        fallback::emit_fallback_function_body(self, func_ir, facts.representation, func_index, ctx);
    }
}
