use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use wasm_encoder::{
    ConstExpr, ElementMode, ElementSection, ElementSegment, Elements, Encode, EntityType,
    ExportKind, Function, Instruction, RefType, TableType,
};

use crate::wasm::WasmBackend;
use crate::wasm_abi::{
    POLL_TABLE_IMPORTS, RESERVED_RUNTIME_CALLABLE_COUNT, RESERVED_RUNTIME_CALLABLE_SPECS,
    RUNTIME_CALLABLE_IMPORTS, RuntimeCallableResult, poll_table_import_slot,
};
use crate::wasm_binary::{emit_call, encode_u32_leb128_padded};
use crate::wasm_data::DataSegmentRef;
use crate::wasm_values::box_none;
use crate::{SimpleIR, TrampolineKind, TrampolineSpec};

pub(super) struct WasmCallableTablePlan {
    pub(super) table_base: u32,
    pub(super) table_indices: Vec<u32>,
    pub(super) split_runtime_owned_slot_start: usize,
    pub(super) split_runtime_shared_abi_slot_end: usize,
    pub(super) func_to_table_idx: BTreeMap<String, u32>,
    pub(super) func_to_index: BTreeMap<String, u32>,
    pub(super) func_to_trampoline_idx: BTreeMap<String, u32>,
    pub(super) closure_functions: BTreeSet<String>,
    builtin_trampoline_start: u32,
    compact_builtin_trampoline_func_start: u32,
    user_trampoline_start: u32,
    compact_builtin_trampoline_funcs: Vec<(String, usize)>,
}

pub(super) struct WasmCallableTableElements {
    pub(super) element_section: Option<ElementSection>,
    pub(super) element_payload: Option<Vec<u8>>,
}

impl WasmBackend {
    pub(super) fn build_table_abi(
        &mut self,
        ir: &SimpleIR,
        builtin_trampoline_specs: &BTreeMap<String, usize>,
        direct_import_call_specs: &BTreeMap<String, usize>,
        default_trampoline_spec: &BTreeMap<String, (usize, bool)>,
        user_type_map: &BTreeMap<usize, u32>,
        reloc_enabled: bool,
        sentinel_func_idx: u32,
    ) -> WasmCallableTablePlan {
        let builtin_table_funcs = RUNTIME_CALLABLE_IMPORTS;
        let reserved_runtime_callable_names: BTreeSet<&str> = RESERVED_RUNTIME_CALLABLE_SPECS
            .iter()
            .map(|spec| spec.runtime_name)
            .collect();
        let generated_builtin_runtime_names: BTreeSet<&str> = builtin_table_funcs
            .iter()
            .map(|spec| spec.runtime_name)
            .chain(
                RESERVED_RUNTIME_CALLABLE_SPECS
                    .iter()
                    .map(|spec| spec.runtime_name),
            )
            .collect();
        let mut compact_builtin_trampoline_funcs: Vec<(String, usize)> = Vec::new();
        for runtime_name in builtin_table_funcs
            .iter()
            .map(|spec| spec.runtime_name)
            .chain(
                RESERVED_RUNTIME_CALLABLE_SPECS
                    .iter()
                    .map(|spec| spec.runtime_name),
            )
        {
            if reserved_runtime_callable_names.contains(runtime_name) {
                continue;
            }
            if let Some(spec) = builtin_table_funcs
                .iter()
                .find(|spec| spec.runtime_name == runtime_name)
                && builtin_trampoline_specs.contains_key(runtime_name)
            {
                compact_builtin_trampoline_funcs.push((runtime_name.to_string(), spec.arity));
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
        for (runtime_name, import_name, arity, result) in builtin_table_funcs.iter().map(|spec| {
            (
                spec.runtime_name.to_string(),
                spec.import_name.to_string(),
                spec.arity,
                spec.result,
            )
        }) {
            if builtin_trampoline_specs.contains_key(runtime_name.as_str()) {
                builtin_wrapper_funcs.push((runtime_name, import_name, arity, result));
            }
        }
        if builtin_trampoline_specs.len() != compact_builtin_trampoline_funcs.len() {
            for name in builtin_trampoline_specs.keys() {
                if !generated_builtin_runtime_names.contains(name.as_str()) {
                    panic!("builtin {name} missing from generated WASM callable table");
                }
            }
        }
        let compact_builtin_table_len: usize = builtin_table_funcs
            .iter()
            .map(|spec| spec.runtime_name.to_string())
            .filter(|rn| builtin_trampoline_specs.contains_key(rn.as_str()))
            .count();
        let split_runtime_runtime_table_min = self.options.split_runtime_runtime_table_min;
        let table_base: u32 = split_runtime_runtime_table_min
            .map(|min| min.max(self.options.table_base))
            .unwrap_or(self.options.table_base);
        let split_runtime_owned_slot_start = split_runtime_runtime_table_min
            .map(|min| min.saturating_sub(table_base) as usize)
            .unwrap_or(0);
        let poll_table_prefix = POLL_TABLE_IMPORTS
            .iter()
            .map(|spec| spec.table_slot)
            .max()
            .unwrap_or(0)
            + 1;
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
            for spec in POLL_TABLE_IMPORTS {
                let import_name = spec.import_name;
                let arity = 1usize;
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

        let safe_idx = |idx: u32| -> u32 {
            if idx == u32::MAX {
                sentinel_func_idx
            } else {
                idx
            }
        };
        let mut table_indices = vec![sentinel_func_idx; poll_table_prefix as usize];
        for spec in POLL_TABLE_IMPORTS {
            let name = spec.import_name;
            let idx = *table_import_wrappers
                .get(name)
                .unwrap_or(&self.import_ids[name]);
            let slot = spec.table_slot as usize;
            *table_indices
                .get_mut(slot)
                .unwrap_or_else(|| panic!("poll table slot {slot} outside poll table prefix")) =
                safe_idx(idx);
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
        for spec in POLL_TABLE_IMPORTS {
            let table_slot = poll_table_import_slot(spec.import_name).unwrap_or_else(|| {
                panic!("missing generated poll table slot for {}", spec.import_name)
            });
            func_to_table_idx.insert(format!("molt_{}", spec.import_name), table_slot);
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
        let mut compact_slot = 0u32;
        for (runtime_name, import_name) in builtin_table_funcs
            .iter()
            .map(|spec| (spec.runtime_name.to_string(), spec.import_name.to_string()))
        {
            let runtime_key = runtime_name;
            let is_referenced = builtin_trampoline_specs.contains_key(runtime_key.as_str());
            if !is_referenced {
                continue;
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

        let closure_functions = default_trampoline_spec
            .iter()
            .filter_map(|(name, &(_arity, has_closure))| {
                if has_closure {
                    Some(name.clone())
                } else {
                    None
                }
            })
            .collect();

        WasmCallableTablePlan {
            table_base,
            table_indices,
            split_runtime_owned_slot_start,
            split_runtime_shared_abi_slot_end,
            func_to_table_idx,
            func_to_index,
            func_to_trampoline_idx,
            closure_functions,
            builtin_trampoline_start,
            compact_builtin_trampoline_func_start,
            user_trampoline_start,
            compact_builtin_trampoline_funcs,
        }
    }

    pub(super) fn emit_table_abi_trampolines(
        &mut self,
        plan: &WasmCallableTablePlan,
        ir: &SimpleIR,
        reloc_enabled: bool,
        default_trampoline_spec: &BTreeMap<String, (usize, bool)>,
        task_kinds: &BTreeMap<String, TrampolineKind>,
        task_closure_sizes: &BTreeMap<String, i64>,
        function_has_ret: &BTreeMap<String, bool>,
        multi_return_candidates: &BTreeMap<String, usize>,
    ) {
        if self.func_count != plan.builtin_trampoline_start {
            panic!(
                "wasm builtin trampoline index mismatch: expected {}, got {}",
                plan.builtin_trampoline_start, self.func_count
            );
        }
        for spec in RESERVED_RUNTIME_CALLABLE_SPECS {
            let name = spec.runtime_name;
            let arity = spec.arity;
            let target_idx = *plan
                .func_to_index
                .get(name)
                .unwrap_or_else(|| panic!("reserved runtime trampoline target missing for {name}"));
            let table_slot = *plan.func_to_table_idx.get(name).unwrap_or_else(|| {
                panic!("reserved runtime trampoline table slot missing for {name}")
            });
            let table_idx = plan.table_base + table_slot;
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
        if self.func_count != plan.compact_builtin_trampoline_func_start {
            panic!(
                "wasm compact builtin trampoline index mismatch: expected {}, got {}",
                plan.compact_builtin_trampoline_func_start, self.func_count
            );
        }
        for (name, arity) in &plan.compact_builtin_trampoline_funcs {
            let target_idx = *plan
                .func_to_index
                .get(name)
                .unwrap_or_else(|| panic!("builtin trampoline target missing for {name}"));
            let table_slot = *plan
                .func_to_table_idx
                .get(name)
                .unwrap_or_else(|| panic!("builtin trampoline table slot missing for {name}"));
            let table_idx = plan.table_base + table_slot;
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
        if self.func_count != plan.user_trampoline_start {
            panic!(
                "wasm user trampoline index mismatch: expected {}, got {}",
                plan.user_trampoline_start, self.func_count
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
            let target_idx = *plan
                .func_to_index
                .get(target_name)
                .unwrap_or_else(|| panic!("trampoline target missing for {target_name}"));
            let table_slot = *plan
                .func_to_table_idx
                .get(target_name)
                .unwrap_or_else(|| panic!("trampoline table slot missing for {target_name}"));
            let table_idx = plan.table_base + table_slot;
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
    }

    pub(super) fn emit_table_elements(
        &mut self,
        plan: &WasmCallableTablePlan,
        reloc_enabled: bool,
        manifest_segment: DataSegmentRef,
        manifest_len: usize,
    ) -> WasmCallableTableElements {
        let mut element_section = None;
        let mut element_payload = None;
        if reloc_enabled {
            let table_init_index = self.compile_table_init(
                reloc_enabled,
                plan.table_base,
                &plan.table_indices,
                plan.split_runtime_owned_slot_start,
                plan.split_runtime_shared_abi_slot_end,
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

            let mut ref_exported = BTreeSet::new();
            for (slot, func_index) in plan.table_indices.iter().enumerate() {
                if slot < plan.split_runtime_owned_slot_start
                    && slot >= plan.split_runtime_shared_abi_slot_end
                {
                    continue;
                }
                let table_index = plan.table_base + slot as u32;
                if ref_exported.insert(table_index) {
                    let name = format!("__molt_table_ref_{table_index}");
                    self.exports.export(&name, ExportKind::Func, *func_index);
                }
            }

            let mut payload = Vec::new();
            1u32.encode(&mut payload);
            payload.push(0x01);
            payload.push(0x00);
            (plan.table_indices.len() as u32).encode(&mut payload);
            for func_index in &plan.table_indices {
                encode_u32_leb128_padded(*func_index, &mut payload);
            }
            element_payload = Some(payload);
        } else {
            let mut section = ElementSection::new();
            let offset = ConstExpr::i32_const(plan.table_base as i32);
            section.segment(ElementSegment {
                mode: ElementMode::Active {
                    table: None,
                    offset: &offset,
                },
                elements: Elements::Functions(Cow::Borrowed(&plan.table_indices)),
            });
            element_section = Some(section);
        }
        WasmCallableTableElements {
            element_section,
            element_payload,
        }
    }
}
