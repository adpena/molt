use super::context::CompileFuncContext;
use super::trampoline_analysis::WasmTrampolineAnalysis;
use super::*;
use runtime_surface::WasmRuntimeSurfacePlan;

mod runtime_surface;

impl WasmBackend {
    pub(super) fn emit_wasm_module(
        mut self,
        ir: SimpleIR,
        lir_fast_outputs: BTreeMap<String, crate::tir::lower_to_wasm::WasmFunctionOutput>,
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

        // Memory & Table (imported for shared-instance linking)

        let builtin_table_funcs = RUNTIME_CALLABLE_IMPORTS;
        let reserved_runtime_callable_names: BTreeSet<&str> = RESERVED_RUNTIME_CALLABLE_SPECS
            .iter()
            .map(|spec| spec.runtime_name)
            .collect();
        let hardcoded_builtin_runtime_names: BTreeSet<&str> = builtin_table_funcs
            .iter()
            .map(|spec| spec.runtime_name)
            .collect();
        let mut auto_builtin_table_funcs: Vec<(String, String, usize)> = builtin_trampoline_specs
            .iter()
            .filter(|(runtime_name, _)| {
                !hardcoded_builtin_runtime_names.contains(runtime_name.as_str())
                    && !reserved_runtime_callable_names.contains(runtime_name.as_str())
            })
            .map(|(runtime_name, arity)| {
                let import_name = runtime_name
                    .strip_prefix("molt_")
                    .unwrap_or(runtime_name.as_str())
                    .to_string();
                (runtime_name.clone(), import_name, *arity)
            })
            .collect();
        auto_builtin_table_funcs.sort_by(|a, b| a.0.cmp(&b.0));
        let mut compact_builtin_trampoline_funcs: Vec<(String, usize)> = Vec::new();
        let builtin_runtime_names: BTreeSet<&str> = builtin_table_funcs
            .iter()
            .map(|spec| spec.runtime_name)
            .chain(
                RESERVED_RUNTIME_CALLABLE_SPECS
                    .iter()
                    .map(|spec| spec.runtime_name),
            )
            .chain(
                auto_builtin_table_funcs
                    .iter()
                    .map(|(runtime_name, _, _)| runtime_name.as_str()),
            )
            .collect();
        for runtime_name in builtin_table_funcs
            .iter()
            .map(|spec| spec.runtime_name)
            .chain(
                RESERVED_RUNTIME_CALLABLE_SPECS
                    .iter()
                    .map(|spec| spec.runtime_name),
            )
            .chain(
                auto_builtin_table_funcs
                    .iter()
                    .map(|(runtime_name, _, _)| runtime_name.as_str()),
            )
        {
            if reserved_runtime_callable_names.contains(runtime_name) {
                continue;
            }
            if let Some(arity) = builtin_trampoline_specs.get(runtime_name) {
                compact_builtin_trampoline_funcs.push((runtime_name.to_string(), *arity));
            }
        }
        let mut builtin_wrapper_funcs: Vec<(String, String, usize, RuntimeCallableResult)> =
            RESERVED_RUNTIME_CALLABLE_SPECS
                .iter()
                .map(|spec| {
                    (
                        spec.runtime_name.to_string(),
                        spec.import_name.to_string(),
                        spec.arity,
                        RuntimeCallableResult::I64,
                    )
                })
                .collect();
        for (runtime_name, import_name, arity, result) in builtin_table_funcs
            .iter()
            .map(|spec| {
                (
                    spec.runtime_name.to_string(),
                    spec.import_name.to_string(),
                    spec.arity,
                    spec.result,
                )
            })
            .chain(
                auto_builtin_table_funcs
                    .iter()
                    .map(|(runtime_name, import_name, arity)| {
                        (
                            runtime_name.clone(),
                            import_name.clone(),
                            *arity,
                            RuntimeCallableResult::I64,
                        )
                    }),
            )
        {
            // Only generate wrappers for builtins that are actually referenced
            // by user code (present in builtin_trampoline_specs). With table
            // compaction, unreferenced builtins are omitted entirely - their
            // wrappers would be dead code wasting space in the code section.
            if builtin_trampoline_specs.contains_key(runtime_name.as_str()) {
                builtin_wrapper_funcs.push((runtime_name, import_name, arity, result));
            }
        }
        if builtin_trampoline_specs.len() != compact_builtin_trampoline_funcs.len() {
            for name in builtin_trampoline_specs.keys() {
                if !builtin_runtime_names.contains(name.as_str()) {
                    panic!("builtin {name} missing from wasm table");
                }
            }
        }
        let compact_builtin_table_len: usize = builtin_table_funcs
            .iter()
            .map(|spec| spec.runtime_name.to_string())
            .chain(auto_builtin_table_funcs.iter().map(|(rn, _, _)| rn.clone()))
            .filter(|rn| builtin_trampoline_specs.contains_key(rn.as_str()))
            .count();
        // Table compaction: only count referenced builtins for the table size.
        // Unreferenced builtins are omitted entirely (not sentinel-filled).
        let split_runtime_runtime_table_min = self.options.split_runtime_runtime_table_min;
        let table_base: u32 = split_runtime_runtime_table_min
            .map(|min| min.max(self.options.table_base))
            .unwrap_or(self.options.table_base);
        let split_runtime_owned_slot_start = split_runtime_runtime_table_min
            .map(|min| min.saturating_sub(table_base) as usize)
            .unwrap_or(0);
        // 1 sentinel slot + one slot per POLL_TABLE_FUNCS entry.
        // Derived dynamically so adding/removing poll functions cannot desync.
        let poll_table_prefix = (1 + POLL_TABLE_FUNCS.len()) as u32;
        let reserved_runtime_callable_table_len = RESERVED_RUNTIME_CALLABLE_COUNT as usize;
        let table_len = (poll_table_prefix as usize
            + reserved_runtime_callable_table_len * 2
            + compact_builtin_table_len
            + compact_builtin_trampoline_funcs.len()
            + ir.functions.len() * 2) as u32;
        let table_min = table_base + table_len;
        let table_ty = TableType {
            element_type: RefType::FUNCREF,
            table64: false,
            minimum: u64::from(table_min),
            maximum: None,
            shared: false,
        };
        self.imports.import(
            "env",
            "__indirect_function_table",
            EntityType::Table(table_ty),
        );
        self.exports.export("molt_table", ExportKind::Table, 0);

        let mut builtin_wrapper_indices = BTreeMap::new();
        for (runtime_name, import_name, arity, result) in &builtin_wrapper_funcs {
            let type_idx = *user_type_map
                .get(arity)
                .unwrap_or_else(|| panic!("missing builtin wrapper signature for arity {arity}"));
            let import_idx = *self
                .import_ids
                .get(import_name.as_str())
                .unwrap_or_else(|| panic!("missing builtin import for {import_name}"));
            self.funcs.function(type_idx);
            let func_index = self.func_count;
            self.func_count += 1;
            let mut func = Function::new_with_locals_types(Vec::new());
            for idx in 0..*arity {
                func.instruction(&Instruction::LocalGet(idx as u32));
            }
            emit_call(&mut func, reloc_enabled, import_idx);
            if *result == RuntimeCallableResult::Void {
                func.instruction(&Instruction::I64Const(box_none()));
            }
            func.instruction(&Instruction::End);
            self.codes.function(&func);
            builtin_wrapper_indices.insert(runtime_name.clone(), func_index);
        }

        let mut table_import_wrappers = BTreeMap::new();
        if reloc_enabled {
            for import_name in POLL_TABLE_FUNCS {
                let arity = 1usize; // all poll functions take 1 arg
                let type_idx = *user_type_map
                    .get(&arity)
                    .unwrap_or_else(|| panic!("missing wrapper signature for arity {arity}"));
                let import_idx = *self
                    .import_ids
                    .get(import_name)
                    .unwrap_or_else(|| panic!("missing import for {import_name}"));
                self.funcs.function(type_idx);
                let func_index = self.func_count;
                self.func_count += 1;
                let mut func = Function::new_with_locals_types(Vec::new());
                for idx in 0..arity {
                    func.instruction(&Instruction::LocalGet(idx as u32));
                }
                emit_call(&mut func, reloc_enabled, import_idx);
                func.instruction(&Instruction::End);
                self.codes.function(&func);
                table_import_wrappers.insert(import_name.to_string(), func_index);
            }
        }

        // Build poll-function table prefix from POLL_TABLE_FUNCS.
        // Replace sentinel u32::MAX indices with sentinel_func_idx so the
        // element section only contains valid function indices.
        let safe_idx = |idx: u32| -> u32 {
            if idx == u32::MAX {
                sentinel_func_idx
            } else {
                idx
            }
        };
        let mut table_indices = vec![sentinel_func_idx]; // slot 0 = sentinel
        for &name in POLL_TABLE_FUNCS {
            let idx = *table_import_wrappers
                .get(name)
                .unwrap_or(&self.import_ids[name]);
            table_indices.push(safe_idx(idx));
        }
        debug_assert_eq!(table_indices.len(), poll_table_prefix as usize);
        let mut func_to_table_idx = BTreeMap::new();
        let mut func_to_index = BTreeMap::new();
        func_to_index.insert(
            "molt_runtime_init".to_string(),
            self.import_ids["runtime_init"],
        );
        func_to_index.insert(
            "molt_runtime_shutdown".to_string(),
            self.import_ids["runtime_shutdown"],
        );
        func_to_index.insert(
            "molt_sys_set_version_info".to_string(),
            self.import_ids["sys_set_version_info"],
        );
        for (slot, import_name) in POLL_TABLE_FUNCS.iter().enumerate() {
            func_to_table_idx.insert(format!("molt_{import_name}"), (slot + 1) as u32);
        }

        let reserved_runtime_callable_table_start = poll_table_prefix;
        let reserved_runtime_trampoline_table_start =
            reserved_runtime_callable_table_start + RESERVED_RUNTIME_CALLABLE_COUNT;
        let compact_builtin_table_start =
            reserved_runtime_trampoline_table_start + RESERVED_RUNTIME_CALLABLE_COUNT;
        let split_runtime_shared_abi_slot_end = compact_builtin_table_start as usize;
        let compact_builtin_trampoline_table_start =
            compact_builtin_table_start + compact_builtin_table_len as u32;
        let user_func_table_start =
            compact_builtin_trampoline_table_start + compact_builtin_trampoline_funcs.len() as u32;
        let user_trampoline_table_start = user_func_table_start + ir.functions.len() as u32;

        for spec in RESERVED_RUNTIME_CALLABLE_SPECS {
            let runtime_name = spec.runtime_name.to_string();
            let wrapper_idx = *builtin_wrapper_indices
                .get(&runtime_name)
                .unwrap_or_else(|| panic!("reserved runtime wrapper missing for {runtime_name}"));
            func_to_table_idx.insert(
                runtime_name.clone(),
                reserved_runtime_callable_table_start + spec.index,
            );
            func_to_index.insert(runtime_name, wrapper_idx);
            table_indices.push(wrapper_idx);
        }

        let mut compact_builtin_entries: Vec<(String, u32)> = Vec::new();
        // Table compaction: only allocate slots for referenced builtins.
        // Unreferenced builtins are completely omitted from the element table.
        let mut compact_slot = 0u32;
        for (runtime_name, import_name) in builtin_table_funcs
            .iter()
            .map(|spec| (spec.runtime_name.to_string(), spec.import_name.to_string()))
            .chain(
                auto_builtin_table_funcs
                    .iter()
                    .map(|(runtime_name, import_name, _)| {
                        (runtime_name.clone(), import_name.clone())
                    }),
            )
        {
            let runtime_key = runtime_name;
            let is_referenced = builtin_trampoline_specs.contains_key(runtime_key.as_str());
            if !is_referenced {
                continue; // Omit - no slot allocated.
            }
            let idx = compact_slot + compact_builtin_table_start;
            func_to_table_idx.insert(runtime_key.clone(), idx);
            let target_index = if let Some(wrapper_idx) = builtin_wrapper_indices.get(&runtime_key)
            {
                func_to_index.insert(runtime_key, *wrapper_idx);
                *wrapper_idx
            } else {
                let import_idx = self
                    .import_ids
                    .get(&import_name)
                    .copied()
                    .unwrap_or(sentinel_func_idx);
                // Replace sentinel u32::MAX with sentinel_func_idx for element section validity.
                let safe = if import_idx == u32::MAX {
                    sentinel_func_idx
                } else {
                    import_idx
                };
                func_to_index.insert(runtime_key, safe);
                safe
            };
            compact_builtin_entries.push((import_name, target_index));
            compact_slot += 1;
        }
        debug_assert_eq!(
            compact_slot as usize, compact_builtin_table_len,
            "compact slot count must match pre-computed builtin_table_len"
        );

        let user_func_start = self.func_count;
        let user_func_count = ir.functions.len() as u32;
        let builtin_trampoline_count =
            RESERVED_RUNTIME_CALLABLE_COUNT + compact_builtin_trampoline_funcs.len() as u32;
        let builtin_trampoline_start = user_func_start + user_func_count;
        let user_trampoline_start = builtin_trampoline_start + builtin_trampoline_count;
        let reserved_runtime_trampoline_func_start = builtin_trampoline_start;
        let compact_builtin_trampoline_func_start =
            reserved_runtime_trampoline_func_start + RESERVED_RUNTIME_CALLABLE_COUNT;

        let mut func_to_trampoline_idx = BTreeMap::new();
        for spec in RESERVED_RUNTIME_CALLABLE_SPECS {
            let runtime_name = spec.runtime_name.to_string();
            func_to_trampoline_idx.insert(
                runtime_name,
                reserved_runtime_trampoline_table_start + spec.index,
            );
            table_indices.push(reserved_runtime_trampoline_func_start + spec.index);
        }
        for (_import_name, target_index) in &compact_builtin_entries {
            table_indices.push(*target_index);
        }
        for runtime_name in direct_import_call_specs.keys() {
            let import_name = runtime_name
                .strip_prefix("molt_")
                .unwrap_or(runtime_name.as_str());
            let import_idx = *self
                .import_ids
                .get(import_name)
                .unwrap_or_else(|| panic!("missing direct runtime import for {runtime_name}"));
            if import_idx == u32::MAX {
                panic!("direct runtime import unexpectedly stripped for {runtime_name}");
            }
            func_to_index.insert(runtime_name.clone(), import_idx);
        }
        for (i, (name, _)) in compact_builtin_trampoline_funcs.iter().enumerate() {
            let idx = compact_builtin_trampoline_table_start + i as u32;
            func_to_trampoline_idx.insert(name.clone(), idx);
            table_indices.push(compact_builtin_trampoline_func_start + i as u32);
        }
        for (i, func_ir) in ir.functions.iter().enumerate() {
            let idx = user_func_table_start + i as u32;
            func_to_table_idx.insert(func_ir.name.clone(), idx);
            func_to_index.insert(func_ir.name.clone(), user_func_start + i as u32);
            table_indices.push(user_func_start + i as u32);
        }
        for (i, func_ir) in ir.functions.iter().enumerate() {
            let idx = user_trampoline_table_start + i as u32;
            func_to_trampoline_idx.insert(func_ir.name.clone(), idx);
            table_indices.push(user_trampoline_start + i as u32);
        }

        for func_ir in &ir.functions {
            for (op_idx, op) in func_ir.ops.iter().enumerate() {
                if matches!(op.kind.as_str(), "call_async" | "alloc_task") {
                    let Some(target_name) = op.s_value.as_deref() else {
                        panic!(
                            "wasm {} target missing in func '{}' op {}",
                            op.kind, func_ir.name, op_idx
                        );
                    };
                    if !target_name.ends_with("_poll") {
                        panic!(
                            "wasm {} target '{}' in func '{}' op {} is not a poll function; expected *_poll table target",
                            op.kind, target_name, func_ir.name, op_idx
                        );
                    }
                    if !func_to_table_idx.contains_key(target_name) {
                        panic!(
                            "wasm {} target '{}' in func '{}' op {} is not table-addressable; expected poll function/table target",
                            op.kind, target_name, func_ir.name, op_idx
                        );
                    }
                }
            }
        }

        if let Ok(raw_slot) = std::env::var("MOLT_DEBUG_WASM_TABLE_SLOT")
            && let Ok(target_slot) = raw_slot.parse::<u32>()
        {
            for (name, slot) in &func_to_table_idx {
                if *slot == target_slot || table_base + *slot == target_slot {
                    eprintln!(
                        "[molt wasm table-slot] kind=function raw_slot={} table_index={} name={}",
                        slot,
                        table_base + *slot,
                        name
                    );
                }
            }
            for (name, slot) in &func_to_trampoline_idx {
                if *slot == target_slot || table_base + *slot == target_slot {
                    eprintln!(
                        "[molt wasm table-slot] kind=trampoline raw_slot={} table_index={} name={}",
                        slot,
                        table_base + *slot,
                        name
                    );
                }
            }
        }

        let import_ids = self.import_ids.clone();
        let return_alias_summaries = crate::passes::compute_return_alias_summaries(&ir.functions);

        // Build the set of functions whose WASM signature includes a leading
        // closure parameter.  The `call_guarded` fast path needs this to
        // extract the closure environment from the callee object and prepend
        // it when directly calling the target.
        let closure_functions: BTreeSet<String> = default_trampoline_spec
            .iter()
            .filter_map(|(name, &(_arity, has_closure))| {
                if has_closure {
                    Some(name.clone())
                } else {
                    None
                }
            })
            .collect();

        let compile_ctx = CompileFuncContext {
            func_map: &func_to_table_idx,
            func_indices: &func_to_index,
            trampoline_map: &func_to_trampoline_idx,
            import_ids: &import_ids,
            reloc_enabled,
            table_base,
            multi_return_candidates: &multi_return_candidates,
            closure_functions: &closure_functions,
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

        if self.func_count != builtin_trampoline_start {
            panic!(
                "wasm builtin trampoline index mismatch: expected {builtin_trampoline_start}, got {}",
                self.func_count
            );
        }
        for spec in RESERVED_RUNTIME_CALLABLE_SPECS {
            let name = spec.runtime_name;
            let arity = spec.arity;
            let target_idx = *func_to_index
                .get(name)
                .unwrap_or_else(|| panic!("reserved runtime trampoline target missing for {name}"));
            let table_slot = *func_to_table_idx.get(name).unwrap_or_else(|| {
                panic!("reserved runtime trampoline table slot missing for {name}")
            });
            let table_idx = table_base + table_slot;
            self.compile_trampoline(
                reloc_enabled,
                target_idx,
                table_idx,
                TrampolineSpec {
                    arity,
                    has_closure: false,
                    kind: TrampolineKind::Plain,
                    closure_size: 0,
                    target_has_ret: true,
                },
                None,
            );
        }
        if self.func_count != compact_builtin_trampoline_func_start {
            panic!(
                "wasm compact builtin trampoline index mismatch: expected {compact_builtin_trampoline_func_start}, got {}",
                self.func_count
            );
        }
        for (name, arity) in &compact_builtin_trampoline_funcs {
            let target_idx = *func_to_index
                .get(name)
                .unwrap_or_else(|| panic!("builtin trampoline target missing for {name}"));
            let table_slot = *func_to_table_idx
                .get(name)
                .unwrap_or_else(|| panic!("builtin trampoline table slot missing for {name}"));
            let table_idx = table_base + table_slot;
            self.compile_trampoline(
                reloc_enabled,
                target_idx,
                table_idx,
                TrampolineSpec {
                    arity: *arity,
                    has_closure: false,
                    kind: TrampolineKind::Plain,
                    closure_size: 0,
                    target_has_ret: true,
                },
                None,
            );
        }
        if self.func_count != user_trampoline_start {
            panic!(
                "wasm user trampoline index mismatch: expected {user_trampoline_start}, got {}",
                self.func_count
            );
        }
        for func_ir in &ir.functions {
            let (arity, has_closure) = *default_trampoline_spec
                .get(&func_ir.name)
                .unwrap_or_else(|| panic!("missing trampoline spec for {}", func_ir.name));
            let kind = task_kinds
                .get(&func_ir.name)
                .copied()
                .unwrap_or(TrampolineKind::Plain);
            let poll_name = if kind != TrampolineKind::Plain && !func_ir.name.ends_with("_poll") {
                format!("{}_poll", func_ir.name)
            } else {
                func_ir.name.clone()
            };
            let target_name = if kind != TrampolineKind::Plain {
                &poll_name
            } else {
                &func_ir.name
            };
            let target_idx = *func_to_index
                .get(target_name)
                .unwrap_or_else(|| panic!("trampoline target missing for {target_name}"));
            let table_slot = *func_to_table_idx
                .get(target_name)
                .unwrap_or_else(|| panic!("trampoline table slot missing for {target_name}"));
            let table_idx = table_base + table_slot;
            let closure_size = if kind == TrampolineKind::Plain {
                0
            } else {
                *task_closure_sizes
                    .get(&func_ir.name)
                    .unwrap_or_else(|| panic!("task closure size missing for {}", func_ir.name))
            };
            let mr_count = if kind == TrampolineKind::Plain {
                multi_return_candidates
                    .get(&func_ir.name)
                    .copied()
                    .filter(|&c| c > 1)
            } else {
                None
            };
            self.compile_trampoline(
                reloc_enabled,
                target_idx,
                table_idx,
                TrampolineSpec {
                    arity,
                    has_closure,
                    kind,
                    closure_size,
                    target_has_ret: *function_has_ret.get(target_name).unwrap_or(&true),
                },
                mr_count,
            );
        }

        let mut element_section = None;
        let mut element_payload = None;
        if reloc_enabled {
            let table_init_index = self.compile_table_init(
                reloc_enabled,
                table_base,
                &table_indices,
                split_runtime_owned_slot_start,
                split_runtime_shared_abi_slot_end,
            );
            self.exports
                .export("molt_table_init", ExportKind::Func, table_init_index);
            let main_index = self
                .molt_main_index
                .unwrap_or_else(|| panic!("molt_main missing for table init wrapper"));
            let wrapper_index = self.compile_molt_main_wrapper(
                reloc_enabled,
                main_index,
                table_init_index,
                manifest_segment,
                manifest_len as u32,
            );
            self.exports
                .export("molt_main", ExportKind::Func, wrapper_index);

            // Relocatable app modules must export table-ref symbols so wasm-ld
            // can relocate function-pointer table slots correctly. Monolithic
            // linked outputs strip these exports after linking; removing them
            // before wasm-ld leaves stale table-index constants that trap in
            // call_indirect at runtime.
            let mut ref_exported = BTreeSet::new();
            for (slot, func_index) in table_indices.iter().enumerate() {
                if slot < split_runtime_owned_slot_start
                    && slot >= split_runtime_shared_abi_slot_end
                {
                    continue;
                }
                let table_index = table_base + slot as u32;
                if ref_exported.insert(table_index) {
                    let name = format!("__molt_table_ref_{table_index}");
                    self.exports.export(&name, ExportKind::Func, *func_index);
                }
            }

            let mut payload = Vec::new();
            1u32.encode(&mut payload);
            payload.push(0x01);
            payload.push(0x00);
            (table_indices.len() as u32).encode(&mut payload);
            for func_index in &table_indices {
                encode_u32_leb128_padded(*func_index, &mut payload);
            }
            element_payload = Some(payload);
        } else {
            let mut section = ElementSection::new();
            let offset = ConstExpr::i32_const(table_base as i32);
            section.segment(ElementSegment {
                mode: ElementMode::Active {
                    table: None,
                    offset: &offset,
                },
                elements: Elements::Functions(Cow::Borrowed(&table_indices)),
            });
            element_section = Some(section);
        }

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
        if let Some(element_section) = element_section.as_ref() {
            self.module.section(element_section);
        }
        if let Some(payload) = element_payload.as_ref() {
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
