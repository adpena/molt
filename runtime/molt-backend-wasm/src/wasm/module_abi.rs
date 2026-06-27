use super::context::CompileFuncContext;
use super::trampoline_analysis::WasmTrampolineAnalysis;
use super::*;
use runtime_surface::WasmRuntimeSurfacePlan;

mod callable_table;
mod runtime_surface;

impl WasmBackend {
    pub(super) fn emit_wasm_module(
        mut self,
        ir: SimpleIR,
        lir_fast_outputs: BTreeMap<String, crate::wasm::body::WasmBody>,
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

        // Build the set of import name prefixes to skip in "pure" profile mode.
        // In pure mode, IO/ASYNC/TIME imports are omitted entirely. Any code path
        // that references a skipped import will trigger a clear compile-time panic.
        let is_pure = self.options.wasm_profile == WasmProfile::Pure;
        let runtime_surface =
            WasmRuntimeSurfacePlan::build(&ir, &lir_fast_outputs, &task_kinds, &self.options);
        let auto_required = runtime_surface.auto_required_imports.clone();
        let is_skipped_import = |name: &str| -> bool {
            is_pure && crate::wasm_abi_generated::pure_profile_skips_import(name)
        };

        let mut import_idx = 0;
        let mut add_import = |name: &str, ty: u32, ids: &mut TrackedImportIds| {
            if matches!(
                std::env::var("MOLT_DEBUG_WASM_IMPORTS").ok().as_deref(),
                Some("1")
            ) && name == "task_new"
            {
                eprintln!(
                    "WASM_IMPORTS add_import name=task_new skipped_prefix={} auto_required_contains={}",
                    is_skipped_import(name),
                    auto_required
                        .as_ref()
                        .is_none_or(|required| required.contains(name))
                );
            }
            if is_skipped_import(name) {
                // In pure mode, skip IO/ASYNC/TIME imports entirely.
                // The import is not registered in the WASM module, so the
                // resulting binary has no dependency on these host functions.
                // Insert a sentinel value so that `import_ids["name"]` lookups
                // succeed (no panic), and `emit_call` emits `unreachable`.
                ids.insert(name.to_string(), u32::MAX);
                return;
            }
            // In auto mode, skip imports not in the required set.
            if let Some(ref required) = auto_required
                && !required.contains(name)
            {
                ids.insert(name.to_string(), u32::MAX);
                return;
            }
            self.imports
                .import("molt_runtime", name, EntityType::Function(ty));
            ids.insert(name.to_string(), import_idx);
            import_idx += 1;
        };
        let mut simple_i64_import_type_map: BTreeMap<usize, u32> = BTreeMap::from([
            (0, 0),
            (1, 2),
            (2, 3),
            (3, 5),
            (4, 7),
            (5, 12),
            (6, 9),
            (7, 10),
            (8, 28),
            (9, 35),
            (10, 36),
            (11, 37),
            (12, 38),
        ]);

        // Host Imports - driven by static registry (see wasm_imports.rs).
        for &(name, type_idx) in crate::wasm_imports::IMPORT_REGISTRY {
            add_import(name, type_idx, &mut self.import_ids);
        }

        let reloc_enabled = self.options.reloc_enabled;

        let auto_import_names = runtime_surface.auto_import_names(&self.import_ids);
        let mut next_type_idx = STATIC_TYPE_COUNT;
        for &arity in auto_import_names.iter().map(|(_, arity)| arity) {
            if let std::collections::btree_map::Entry::Vacant(entry) =
                simple_i64_import_type_map.entry(arity)
            {
                self.types.function(
                    std::iter::repeat_n(ValType::I64, arity),
                    std::iter::once(ValType::I64),
                );
                entry.insert(next_type_idx);
                next_type_idx += 1;
            }
        }
        for (import_name, arity) in auto_import_names {
            add_import(
                import_name.as_str(),
                *simple_i64_import_type_map
                    .get(&arity)
                    .unwrap_or_else(|| panic!("missing simple i64 import type for arity {arity}")),
                &mut self.import_ids,
            );
        }
        self.func_count = import_idx;
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

        let max_call_indirect = 13usize;
        let max_needed_arity = max_func_arity
            .max(max_call_arity.saturating_add(3))
            .max(max_call_indirect + 1);
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

        for arity in 0..=max_call_indirect {
            let sig_idx = *user_type_map.get(&(arity + 1)).unwrap_or_else(|| {
                panic!("missing call_indirect signature for arity {}", arity + 1)
            });
            let callee_idx = *user_type_map
                .get(&arity)
                .unwrap_or_else(|| panic!("missing call_indirect callee type for arity {}", arity));
            self.funcs.function(sig_idx);
            let export_name = format!("molt_call_indirect{arity}");
            self.exports
                .export(&export_name, ExportKind::Func, self.func_count);
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
            lir_fast_outputs: &lir_fast_outputs,
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

        let page_size: u64 = 64 * 1024;
        let required_pages = (self.data_segments.offset() as u64).div_ceil(page_size);
        let floor_pages = std::env::var("MOLT_WASM_MIN_PAGES")
            .ok()
            .and_then(|val| val.parse::<u64>().ok())
            .unwrap_or(64);
        let minimum_pages = required_pages.max(floor_pages);
        let memory_ty = MemoryType {
            minimum: minimum_pages,
            maximum: None,
            memory64: false,
            shared: false,
            page_size_log2: None,
        };
        self.imports
            .import("env", "memory", EntityType::Memory(memory_ty));
        self.exports.export("molt_memory", ExportKind::Memory, 0);

        // --- Import audit diagnostic (gated by MOLT_WASM_IMPORT_AUDIT=1) ---
        if std::env::var("MOLT_WASM_IMPORT_AUDIT").as_deref() == Ok("1") {
            let unused = self.import_ids.unused_names();
            let total = self.import_ids.len();
            let used = total - unused.len();
            let pct = if total > 0 {
                (unused.len() as f64 / total as f64) * 100.0
            } else {
                0.0
            };
            eprintln!(
                "[molt-wasm-import-audit] {used}/{total} imports used, {} unused ({pct:.1}% bloat)",
                unused.len()
            );
            if !unused.is_empty() {
                eprintln!("[molt-wasm-import-audit] unused imports:");
                for name in &unused {
                    eprintln!("  - {name}");
                }
            }

            // --- Exception-related host call audit (Section 3.6) ---
            let eh_imports = [
                "exception_push",
                "exception_pop",
                "exception_pending",
                "exception_clear",
                "exception_new",
                "exception_new_builtin",
                "exception_new_builtin_empty",
                "exception_new_builtin_one",
                "exception_new_from_class",
                "exception_match_builtin",
                "exception_kind",
                "exception_class",
                "exception_message",
                "exception_active",
                "exception_last",
                "exception_last_pending",
                "exception_stack_clear",
                "exception_set_cause",
                "exception_set_value",
                "exception_context_set",
                "exception_set_last",
                "raise",
            ];
            let eh_used: Vec<&str> = eh_imports
                .iter()
                .copied()
                .filter(|name| self.import_ids.is_used(name))
                .collect();
            let eh_eliminable: Vec<&str> = ["exception_push", "exception_pop", "exception_pending"]
                .iter()
                .copied()
                .filter(|name| self.import_ids.is_used(name))
                .collect();
            eprintln!(
                "[molt-wasm-import-audit] exception host calls: {}/{} used ({} eliminable by native EH: {})",
                eh_used.len(),
                eh_imports.len(),
                eh_eliminable.len(),
                eh_eliminable.join(", "),
            );
            if self.options.native_eh_enabled && !self.options.reloc_enabled {
                eprintln!("[molt-wasm-import-audit] native EH ENABLED: tag section emitted");
            } else if self.options.native_eh_enabled && self.options.reloc_enabled {
                eprintln!(
                    "[molt-wasm-import-audit] native EH requested but suppressed (reloc mode; wasm-ld doesn't support EH relocations)"
                );
            } else {
                eprintln!("[molt-wasm-import-audit] native EH disabled (MOLT_WASM_NATIVE_EH=0)");
            }

            // --- Tail call optimization audit (section 3.5) ---
            eprintln!(
                "[molt-wasm-import-audit] tail calls emitted: {} (return_call instructions)",
                self.tail_calls_emitted
            );

            // --- Data segment size audit ---
            let total_data_bytes = self.data_segments.total_data_bytes();
            let dedup_hits = self.data_segments.dedup_entry_count();
            eprintln!(
                "[molt-wasm-import-audit] data segments: {} segments, {} total bytes, {} dedup cache entries",
                self.data_segments.segment_count(),
                total_data_bytes,
                dedup_hits,
            );
        }

        self.module.section(&self.types);
        self.module.section(&self.imports);
        self.module.section(&self.funcs);
        self.module.section(&self.tables);
        self.module.section(&self.memories);

        // --- WASM EH Tag Section (Section 3.6) ---
        // Tag 0 = molt_exception with payload (i64) -> (), using type index 1.
        // Emitted between memory and export sections per WASM spec ordering.
        // Native EH requires non-relocatable output (wasm-ld doesn't support EH relocations)
        if self.options.native_eh_enabled && !self.options.reloc_enabled {
            let mut tags = TagSection::new();
            tags.tag(TagType {
                kind: TagKind::Exception,
                func_type_idx: TAG_EXCEPTION_FUNC_TYPE,
            });
            self.module.section(&tags);
        }

        self.module.section(&self.exports);
        if let Some(element_section) = callable_table_elements.element_section.as_ref() {
            self.module.section(element_section);
        }
        if let Some(payload) = callable_table_elements.element_payload.as_ref() {
            let raw_section = RawSection {
                id: 9,
                data: payload,
            };
            self.module.section(&raw_section);
        }
        self.module.section(&self.codes);
        self.module.section(self.data_segments.section());
        let module_finish_start = std::time::Instant::now();
        let mut bytes = self.module.finish();
        emit_wasm_stage_audit(
            "after-module-finish",
            simple_ir_stage_shape(&ir.functions),
            Some(bytes.len()),
            None,
            None,
            Some(module_finish_start.elapsed().as_millis()),
        );

        // --- Dead import elimination ---
        // After compilation, TrackedImportIds knows exactly which imports were
        // referenced during code emission.  Strip the unused ones from the
        // serialized module and remap all function indices.  Stripping is
        // attempted unconditionally; only the *result* is validated before
        // replacing the original binary.
        // Only applies to Auto profile in non-relocatable mode.
        // Full profile preserves all imports for maximum host compatibility;
        // Pure profile's import set is already curated and expected stable.
        // Relocatable modules are linked by wasm-ld --gc-sections instead.
        let strip_enabled = !reloc_enabled && self.options.wasm_profile == WasmProfile::Auto;
        if strip_enabled {
            let unused: BTreeSet<String> = self.import_ids.unused_names().into_iter().collect();
            if !unused.is_empty() {
                let before_len = bytes.len();
                emit_wasm_stage_audit(
                    "before-strip-unused-imports",
                    simple_ir_stage_shape(&ir.functions),
                    Some(before_len),
                    Some(unused.len()),
                    None,
                    None,
                );
                let strip_start = std::time::Instant::now();
                let stripped = strip_unused_imports(bytes.clone(), &unused);
                emit_wasm_stage_audit(
                    "after-strip-unused-imports",
                    simple_ir_stage_shape(&ir.functions),
                    Some(stripped.len()),
                    Some(unused.len()),
                    None,
                    Some(strip_start.elapsed().as_millis()),
                );
                if validate_wasm_sections(&stripped) {
                    eprintln!(
                        "[molt-wasm-strip] eliminated {} unused imports, \
                         {} -> {} bytes (saved {})",
                        unused.len(),
                        before_len,
                        stripped.len(),
                        before_len.saturating_sub(stripped.len()),
                    );
                    bytes = stripped;
                } else {
                    eprintln!(
                        "[molt-wasm-strip] stripping {} unused imports produced \
                         invalid WASM; keeping original ({} bytes)",
                        unused.len(),
                        before_len,
                    );
                }
            }
        }

        if reloc_enabled {
            bytes = add_reloc_sections(
                bytes,
                self.data_segments.segments(),
                self.data_segments.relocs(),
            );
        }
        bytes
    }
}
