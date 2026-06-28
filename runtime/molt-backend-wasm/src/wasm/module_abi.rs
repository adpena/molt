use super::WasmBackend;
use super::call_site_abi::WasmCallSiteAbi;
use super::context::CompileFuncContext;
use super::trampoline_analysis::WasmTrampolineAnalysis;
use imports::WasmRuntimeImportEmission;
use runtime_surface::WasmRuntimeSurfacePlan;

use crate::SimpleIR;
use crate::wasm_abi::emit_static_type_section;
use crate::wasm_plan::DEFAULT_GPU_INTRINSIC_MANIFEST_NAMES;

mod callable_layout;
mod callable_table;
mod finalize;
mod imports;
mod runtime_callables;
mod runtime_surface;
mod table_init;
mod trampoline_emit;
mod type_layout;

use finalize::WasmModuleFinalizationInput;
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
            next_type_idx,
        } = self.emit_runtime_import_surface(&ir, &lir_lowering_plans, &task_kinds);
        let WasmRuntimeSurfacePlan {
            max_func_arity,
            max_call_arity,
            max_class_def_words,
            builtin_trampoline_specs,
            direct_import_call_specs,
            mut manifest_intrinsic_names,
            ..
        } = runtime_surface;

        // Per-app intrinsic manifest: serialize used intrinsic names as a
        // NUL-separated data segment so the runtime only registers these.
        manifest_intrinsic_names.extend(
            DEFAULT_GPU_INTRINSIC_MANIFEST_NAMES
                .iter()
                .map(|name| (*name).to_string()),
        );
        let manifest_bytes: Vec<u8> = {
            let mut buf = Vec::new();
            for (i, name) in manifest_intrinsic_names.iter().enumerate() {
                if i > 0 {
                    buf.push(0);
                }
                buf.extend_from_slice(name.as_bytes());
            }
            buf
        };
        let manifest_segment = self.add_data_segment(reloc_enabled, &manifest_bytes);
        let manifest_len = manifest_bytes.len();

        // Allocate a scratch buffer in linear memory for spilling call_func args.
        // Size: max(max_call_arity, 1) * 8 bytes (one i64 per arg).
        // SAFETY: This single-segment spill buffer is safe under reentrant calls
        // because `molt_call_func_dispatch` copies args into a Rust Vec<u64>
        // before dispatching, so nested WASM->runtime->WASM calls never observe
        // stale data in this buffer.
        let spill_slots = max_call_arity.max(1);
        let spill_bytes = vec![0u8; spill_slots * 8];
        let spill_segment = self.add_data_segment_mutable(reloc_enabled, &spill_bytes);
        let call_func_spill_offset = spill_segment.offset;

        // Shared outlined class_def spill buffer. The runtime helper snapshots the
        // bases/attrs payload before nested calls, so reentrant wasm->runtime->wasm
        // execution cannot observe stale scratch contents.
        let class_def_words = max_class_def_words.max(2);
        let class_def_bytes = vec![0u8; class_def_words * 8];
        let class_def_segment = self.add_data_segment_mutable(reloc_enabled, &class_def_bytes);
        let class_def_spill_offset = class_def_segment.offset;

        // Allocate an 8-byte scratch buffer in linear memory for const_str
        // operations.  Previously each const_str allocated a fresh 8-byte
        // heap object via `alloc(8)` to serve as the `out` parameter for
        // `string_from_bytes`, then leaked it (never dec-refed).  For large
        // modules with hundreds of string constants this wasted significant
        // heap space, bringing the heap closer to the output data region in
        // the split-runtime layout and contributing to heap-into-data
        // corruption.  Using a fixed scratch slot eliminates both the leak
        // and the per-string alloc call overhead.
        let const_str_scratch_bytes = vec![0u8; 8];
        let const_str_scratch_segment =
            self.add_data_segment_mutable(reloc_enabled, &const_str_scratch_bytes);

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
            call_site_abi: WasmCallSiteAbi::new(
                &callable_table.func_to_table_idx,
                &callable_table.func_to_index,
                &callable_table.func_to_trampoline_idx,
                callable_table.table_base,
                &callable_table.closure_functions,
                &escaped_callable_targets,
                call_func_spill_offset,
                &return_alias_summaries,
            ),
            import_ids: &import_ids,
            reloc_enabled,
            multi_return_candidates: &multi_return_candidates,
            class_def_spill_offset,
            const_str_scratch_segment,
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
            manifest_segment,
            manifest_len,
        );

        self.finalize_wasm_module(WasmModuleFinalizationInput {
            functions: &ir.functions,
            callable_table_elements,
            reloc_enabled,
        })
    }
}
