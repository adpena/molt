use super::context::CompileFuncContext;
use super::trampoline_analysis::WasmTrampolineAnalysis;
use super::*;
use imports::WasmRuntimeImportEmission;
use runtime_surface::WasmRuntimeSurfacePlan;

mod callable_table;
mod finalize;
mod imports;
mod runtime_surface;
mod trampoline_emit;

use finalize::WasmModuleFinalizationInput;

impl WasmBackend {
    pub(super) fn emit_wasm_module(
        mut self,
        ir: SimpleIR,
        lir_lowering_plans: crate::wasm_plan::WasmFunctionLoweringPlans,
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
            mut next_type_idx,
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

        let mut user_type_map: BTreeMap<usize, u32> = BTreeMap::new();
        // Types 0-40 are static above; additional simple-i64 import signatures
        // may have extended the type section before user arity signatures.
        for func_ir in &ir.functions {
            if func_ir.name.ends_with("_poll") {
                continue;
            }
            let arity = func_ir.params.len();
            if let std::collections::btree_map::Entry::Vacant(entry) = user_type_map.entry(arity) {
                self.types.function(
                    std::iter::repeat_n(ValType::I64, arity),
                    std::iter::once(ValType::I64),
                );
                entry.insert(next_type_idx);
                next_type_idx += 1;
            }
        }

        // Multi-value return type signatures for candidate functions.
        // Maps (param_count, return_count) -> type index.
        let mut multi_return_type_map: BTreeMap<(usize, usize), u32> = BTreeMap::new();
        {
            // Collect unique (param_count, return_count) pairs from candidates.
            let func_param_counts: BTreeMap<&str, usize> = ir
                .functions
                .iter()
                .map(|f| (f.name.as_str(), f.params.len()))
                .collect();
            let mut needed: Vec<(usize, usize)> = Vec::new();
            for (name, ret_count) in &multi_return_candidates {
                if let Some(&param_count) = func_param_counts.get(name.as_str()) {
                    let key = (param_count, *ret_count);
                    if let std::collections::btree_map::Entry::Vacant(e) =
                        multi_return_type_map.entry(key)
                    {
                        e.insert(next_type_idx);
                        needed.push(key);
                        next_type_idx += 1;
                    }
                }
            }
            // Sort for deterministic type section ordering.
            needed.sort();
            // Re-assign indices in sorted order.
            let base = next_type_idx - needed.len() as u32;
            for (i, key) in needed.iter().enumerate() {
                multi_return_type_map.insert(*key, base + i as u32);
            }
            for (param_count, ret_count) in &needed {
                self.types.function(
                    std::iter::repeat_n(ValType::I64, *param_count),
                    std::iter::repeat_n(ValType::I64, *ret_count),
                );
            }
        }

        let max_needed_arity = max_func_arity
            .max(max_call_arity.saturating_add(3))
            .max(CALL_INDIRECT_MAX_ARITY + 1);
        for arity in 0..=max_needed_arity {
            if let std::collections::btree_map::Entry::Vacant(entry) = user_type_map.entry(arity) {
                self.types.function(
                    std::iter::repeat_n(ValType::I64, arity),
                    std::iter::once(ValType::I64),
                );
                entry.insert(next_type_idx);
                next_type_idx += 1;
            }
        }

        for spec in CALL_INDIRECT_IMPORTS {
            let arity = spec.arity;
            let sig_idx = *user_type_map.get(&(arity + 1)).unwrap_or_else(|| {
                panic!("missing call_indirect signature for arity {}", arity + 1)
            });
            let callee_idx = *user_type_map
                .get(&arity)
                .unwrap_or_else(|| panic!("missing call_indirect callee type for arity {}", arity));
            self.funcs.function(sig_idx);
            self.exports
                .export(spec.import_name, ExportKind::Func, self.func_count);
            let mut call_indirect = Function::new_with_locals_types(Vec::new());
            for idx in 0..arity {
                call_indirect.instruction(&Instruction::LocalGet((idx + 1) as u32));
            }
            call_indirect.instruction(&Instruction::LocalGet(0));
            call_indirect.instruction(&Instruction::I32WrapI64);
            emit_call_indirect(&mut call_indirect, reloc_enabled, callee_idx, 0);
            call_indirect.instruction(&Instruction::End);
            self.codes.function(&call_indirect);
            self.func_count += 1;
        }

        let sentinel_func_idx = self.func_count;
        self.funcs.function(2);
        let mut sentinel = Function::new_with_locals_types(Vec::new());
        sentinel.instruction(&Instruction::Unreachable);
        sentinel.instruction(&Instruction::End);
        self.codes.function(&sentinel);
        self.func_count += 1;

        // Callable table ABI: function indices, table slots, trampolines,
        // and relocatable element payloads share one layout authority.
        let callable_table = self.build_table_abi(
            &ir,
            &builtin_trampoline_specs,
            &direct_import_call_specs,
            &default_trampoline_spec,
            &user_type_map,
            reloc_enabled,
            sentinel_func_idx,
        );

        let import_ids = self.import_ids.clone();
        let return_alias_summaries = crate::passes::compute_return_alias_summaries(&ir.functions);

        let compile_ctx = CompileFuncContext {
            func_map: &callable_table.func_to_table_idx,
            func_indices: &callable_table.func_to_index,
            trampoline_map: &callable_table.func_to_trampoline_idx,
            import_ids: &import_ids,
            reloc_enabled,
            table_base: callable_table.table_base,
            multi_return_candidates: &multi_return_candidates,
            closure_functions: &callable_table.closure_functions,
            escaped_callable_targets: &escaped_callable_targets,
            call_func_spill_offset,
            class_def_spill_offset,
            const_str_scratch_segment,
            lir_lowering_plans: &lir_lowering_plans,
            return_alias_summaries: &return_alias_summaries,
        };
        for func_ir in &ir.functions {
            let type_idx = if func_ir.name.ends_with("_poll") {
                2
            } else if let Some(&ret_count) = multi_return_candidates.get(&func_ir.name) {
                let key = (func_ir.params.len(), ret_count);
                *multi_return_type_map
                    .get(&key)
                    .unwrap_or(user_type_map.get(&func_ir.params.len()).unwrap_or(&0))
            } else {
                *user_type_map.get(&func_ir.params.len()).unwrap_or(&0)
            };
            self.compile_func(func_ir, type_idx, &compile_ctx);
        }

        self.emit_table_abi_trampolines(
            &callable_table,
            &ir,
            reloc_enabled,
            &default_trampoline_spec,
            &task_kinds,
            &task_closure_sizes,
            &function_has_ret,
            &multi_return_candidates,
        );

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
