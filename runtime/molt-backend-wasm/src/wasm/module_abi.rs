use super::WasmBackend;
use super::context::CompileFuncContext;
use super::trampoline_analysis::WasmTrampolineAnalysis;
use imports::WasmRuntimeImportEmission;
use native_callables::WasmNativeCallableImportEmission;
use runtime_surface::WasmRuntimeSurfacePlan;

use crate::SimpleIR;
use crate::wasm_abi::emit_static_type_section;

mod callable_table;
mod finalize;
mod host_surface;
mod imports;
mod native_callables;
mod poll_table;
mod runtime_surface;
mod type_layout;

pub(in crate::wasm) use callable_table::WasmCallableCallSiteAbi;
use finalize::WasmModuleFinalizationInput;
pub(in crate::wasm) use native_callables::{WasmNativeCallableImport, WasmNativeCallableImports};
use type_layout::WasmModuleTypeLayout;

impl WasmBackend {
    pub(super) fn emit_wasm_module(
        mut self,
        ir: SimpleIR,
        lir_lowering_plans: crate::wasm::lir_fast::WasmFunctionLoweringPlans,
        analysis: WasmTrampolineAnalysis,
    ) -> Vec<u8> {
        let WasmTrampolineAnalysis {
            escaped_callable_targets,
            task_kinds,
            task_closure_sizes,
            default_trampoline_spec,
            function_has_ret,
            multi_return_candidates,
        } = analysis;

        emit_static_type_section(&mut self.types);

        let reloc_enabled = self.options.reloc_enabled;
        let WasmRuntimeImportEmission {
            runtime_surface,
            next_type_idx: next_type_idx_after_runtime,
        } = self.emit_runtime_import_surface(&ir);
        let WasmNativeCallableImportEmission {
            imports: native_callable_imports,
            next_type_idx,
        } = self.emit_native_callable_import_surface(&ir, next_type_idx_after_runtime);
        let WasmRuntimeSurfacePlan {
            max_func_arity,
            max_call_arity,
            max_class_def_words,
            builtin_trampoline_specs,
            direct_import_call_specs,
            manifest_intrinsic_names,
            ..
        } = runtime_surface;

        let host_surface = self.prepare_module_host_surface(
            reloc_enabled,
            manifest_intrinsic_names,
            max_call_arity,
            max_class_def_words,
        );

        let type_layout = WasmModuleTypeLayout::build(
            &mut self,
            &ir,
            next_type_idx,
            max_func_arity,
            max_call_arity,
            &multi_return_candidates,
        );
        let sentinel_func_idx =
            type_layout.emit_call_indirect_exports_and_sentinel(&mut self, reloc_enabled);

        // Callable table ABI: function indices, table slots, trampolines,
        // and relocatable element payloads share one layout authority.
        let callable_table = self.build_table_abi(
            &ir,
            &builtin_trampoline_specs,
            &direct_import_call_specs,
            &default_trampoline_spec,
            &task_kinds,
            &task_closure_sizes,
            &function_has_ret,
            &multi_return_candidates,
            type_layout.user_type_map(),
            reloc_enabled,
            sentinel_func_idx,
        );

        let import_ids = self.import_ids.clone();
        let return_alias_summaries = crate::passes::compute_return_alias_summaries(&ir.functions);

        let compile_ctx = CompileFuncContext {
            call_site_abi: callable_table.call_site_abi(
                &escaped_callable_targets,
                host_surface.call_func_spill_offset,
                &return_alias_summaries,
            ),
            import_ids: &import_ids,
            native_callable_imports: &native_callable_imports,
            reloc_enabled,
            multi_return_candidates: &multi_return_candidates,
            class_def_spill_offset: host_surface.class_def_spill_offset,
            const_str_scratch_segment: host_surface.const_str_scratch_segment,
            lir_lowering_plans: &lir_lowering_plans,
        };
        for func_ir in &ir.functions {
            let type_idx = type_layout.type_idx_for_function(func_ir);
            self.compile_func(func_ir, type_idx, &compile_ctx);
        }

        self.emit_table_abi_trampolines(&callable_table, reloc_enabled);

        let callable_table_elements = self.emit_table_elements(
            &callable_table,
            reloc_enabled,
            host_surface.manifest_segment,
            host_surface.manifest_len,
        );

        self.finalize_wasm_module(WasmModuleFinalizationInput {
            functions: &ir.functions,
            callable_table_elements,
            reloc_enabled,
        })
    }
}
