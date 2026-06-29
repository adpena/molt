use std::collections::BTreeMap;

use wasm_encoder::{EntityType, ExportKind, RefType, TableType};

use super::super::poll_table::WasmPollTableLayout;
use super::runtime_callables::WasmRuntimeCallableTablePlan;
use super::{WasmCallableTablePlan, WasmCallableTrampolineEntry};
use crate::wasm::WasmBackend;
use crate::wasm_abi::{RESERVED_RUNTIME_CALLABLE_COUNT, RESERVED_RUNTIME_CALLABLE_SPECS};
use crate::{SimpleIR, TrampolineKind, TrampolineSpec};

impl WasmBackend {
    pub(in crate::wasm::module_abi) fn build_table_abi(
        &mut self,
        ir: &SimpleIR,
        builtin_trampoline_specs: &BTreeMap<String, usize>,
        direct_import_call_specs: &BTreeMap<String, usize>,
        default_trampoline_spec: &BTreeMap<String, (usize, bool)>,
        task_kinds: &BTreeMap<String, TrampolineKind>,
        task_closure_sizes: &BTreeMap<String, i64>,
        function_has_ret: &BTreeMap<String, bool>,
        multi_return_candidates: &BTreeMap<String, usize>,
        user_type_map: &BTreeMap<usize, u32>,
        reloc_enabled: bool,
        sentinel_func_idx: u32,
    ) -> WasmCallableTablePlan {
        let runtime_callable_plan = WasmRuntimeCallableTablePlan::build(builtin_trampoline_specs);
        let compact_builtin_table_len = runtime_callable_plan.compact_builtin_table_len();
        let split_runtime_runtime_table_min = self.options.split_runtime_runtime_table_min;
        let table_base: u32 = split_runtime_runtime_table_min
            .map(|min| min.max(self.options.table_base))
            .unwrap_or(self.options.table_base);
        let split_runtime_owned_slot_start = split_runtime_runtime_table_min
            .map(|min| min.saturating_sub(table_base) as usize)
            .unwrap_or(0);
        let poll_table = WasmPollTableLayout::build();
        let poll_table_prefix = poll_table.prefix_len();
        let reserved_runtime_callable_table_len = RESERVED_RUNTIME_CALLABLE_COUNT as usize;
        let table_len = (poll_table_prefix as usize
            + reserved_runtime_callable_table_len * 2
            + compact_builtin_table_len
            + runtime_callable_plan.compact_builtin_trampoline_count() as usize
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

        let builtin_wrapper_indices = self.emit_runtime_callable_wrappers(
            &runtime_callable_plan,
            user_type_map,
            reloc_enabled,
        );

        let table_import_wrappers =
            poll_table.emit_import_wrappers(self, reloc_enabled, user_type_map);
        let mut table_indices = poll_table.initial_table_indices(
            &table_import_wrappers,
            &self.import_ids,
            sentinel_func_idx,
        );
        let mut func_to_table_idx = BTreeMap::new();
        let mut func_to_index = BTreeMap::new();
        func_to_index.insert(
            "molt_runtime_init".to_string(),
            self.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::RuntimeInit],
        );
        func_to_index.insert(
            "molt_runtime_shutdown".to_string(),
            self.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::RuntimeShutdown],
        );
        func_to_index.insert(
            "molt_sys_set_version_info".to_string(),
            self.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::SysSetVersionInfo],
        );
        poll_table.seed_function_table_slots(&mut func_to_table_idx);

        let reserved_runtime_callable_table_start = poll_table_prefix;
        let reserved_runtime_trampoline_table_start =
            reserved_runtime_callable_table_start + RESERVED_RUNTIME_CALLABLE_COUNT;
        let compact_builtin_table_start =
            reserved_runtime_trampoline_table_start + RESERVED_RUNTIME_CALLABLE_COUNT;
        let split_runtime_shared_abi_slot_end = compact_builtin_table_start as usize;
        let compact_builtin_trampoline_table_start =
            compact_builtin_table_start + compact_builtin_table_len as u32;
        let user_func_table_start = compact_builtin_trampoline_table_start
            + runtime_callable_plan.compact_builtin_trampoline_count();
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
        for callable in runtime_callable_plan.compact_builtin_runtime_callables() {
            let runtime_key = callable.runtime_name.clone();
            let idx = compact_slot + compact_builtin_table_start;
            func_to_table_idx.insert(runtime_key.clone(), idx);
            let target_index = if let Some(wrapper_idx) = builtin_wrapper_indices.get(&runtime_key)
            {
                func_to_index.insert(runtime_key, *wrapper_idx);
                *wrapper_idx
            } else {
                let import_idx = self
                    .import_ids
                    .get_name(callable.import_name.as_str())
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
            compact_builtin_entries.push((callable.import_name.clone(), target_index));
            compact_slot += 1;
        }
        debug_assert_eq!(
            compact_slot as usize, compact_builtin_table_len,
            "compact slot count must match pre-computed builtin_table_len"
        );

        let user_func_start = self.func_count;
        let user_func_count = ir.functions.len() as u32;
        let builtin_trampoline_count = RESERVED_RUNTIME_CALLABLE_COUNT
            + runtime_callable_plan.compact_builtin_trampoline_count();
        let builtin_trampoline_start = user_func_start + user_func_count;
        let user_trampoline_start = builtin_trampoline_start + builtin_trampoline_count;
        let reserved_runtime_trampoline_func_start = builtin_trampoline_start;
        let compact_builtin_trampoline_func_start =
            reserved_runtime_trampoline_func_start + RESERVED_RUNTIME_CALLABLE_COUNT;

        let mut func_to_trampoline_idx = BTreeMap::new();
        let mut trampoline_entries = Vec::new();
        for spec in RESERVED_RUNTIME_CALLABLE_SPECS {
            let runtime_name = spec.runtime_name.to_string();
            func_to_trampoline_idx.insert(
                runtime_name,
                reserved_runtime_trampoline_table_start + spec.index,
            );
            let expected_func_index = reserved_runtime_trampoline_func_start + spec.index;
            let name = spec.runtime_name;
            let target_func_index = *func_to_index
                .get(name)
                .unwrap_or_else(|| panic!("reserved runtime trampoline target missing for {name}"));
            let table_slot = *func_to_table_idx.get(name).unwrap_or_else(|| {
                panic!("reserved runtime trampoline table slot missing for {name}")
            });
            push_trampoline_entry(
                &mut table_indices,
                &mut trampoline_entries,
                WasmCallableTrampolineEntry {
                    name: name.to_string(),
                    expected_func_index,
                    target_func_index,
                    table_index: table_base + table_slot,
                    spec: TrampolineSpec {
                        arity: spec.arity,
                        has_closure: false,
                        kind: TrampolineKind::Plain,
                        closure_size: 0,
                        target_has_ret: true,
                    },
                    multi_return_count: None,
                },
            );
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
                .get_name(import_name)
                .unwrap_or_else(|| panic!("missing direct runtime import for {runtime_name}"));
            if import_idx == u32::MAX {
                panic!("direct runtime import unexpectedly stripped for {runtime_name}");
            }
            func_to_index.insert(runtime_name.clone(), import_idx);
        }
        let compact_builtin_trampoline_funcs: Vec<(String, usize)> = runtime_callable_plan
            .compact_builtin_runtime_callables()
            .iter()
            .map(|callable| (callable.runtime_name.clone(), callable.arity))
            .collect();
        for (i, (name, arity)) in compact_builtin_trampoline_funcs.iter().enumerate() {
            let idx = compact_builtin_trampoline_table_start + i as u32;
            func_to_trampoline_idx.insert(name.clone(), idx);
            let expected_func_index = compact_builtin_trampoline_func_start + i as u32;
            let target_func_index = *func_to_index
                .get(name)
                .unwrap_or_else(|| panic!("builtin trampoline target missing for {name}"));
            let table_slot = *func_to_table_idx
                .get(name)
                .unwrap_or_else(|| panic!("builtin trampoline table slot missing for {name}"));
            push_trampoline_entry(
                &mut table_indices,
                &mut trampoline_entries,
                WasmCallableTrampolineEntry {
                    name: name.clone(),
                    expected_func_index,
                    target_func_index,
                    table_index: table_base + table_slot,
                    spec: TrampolineSpec {
                        arity: *arity,
                        has_closure: false,
                        kind: TrampolineKind::Plain,
                        closure_size: 0,
                        target_has_ret: true,
                    },
                    multi_return_count: None,
                },
            );
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
            let expected_func_index = user_trampoline_start + i as u32;
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
            let target_func_index = *func_to_index
                .get(target_name)
                .unwrap_or_else(|| panic!("trampoline target missing for {target_name}"));
            let table_slot = *func_to_table_idx
                .get(target_name)
                .unwrap_or_else(|| panic!("trampoline table slot missing for {target_name}"));
            let closure_size = if kind == TrampolineKind::Plain {
                0
            } else {
                *task_closure_sizes
                    .get(&func_ir.name)
                    .unwrap_or_else(|| panic!("task closure size missing for {}", func_ir.name))
            };
            let multi_return_count = if kind == TrampolineKind::Plain {
                multi_return_candidates
                    .get(&func_ir.name)
                    .copied()
                    .filter(|&count| count > 1)
            } else {
                None
            };
            push_trampoline_entry(
                &mut table_indices,
                &mut trampoline_entries,
                WasmCallableTrampolineEntry {
                    name: func_ir.name.clone(),
                    expected_func_index,
                    target_func_index,
                    table_index: table_base + table_slot,
                    spec: TrampolineSpec {
                        arity,
                        has_closure,
                        kind,
                        closure_size,
                        target_has_ret: *function_has_ret.get(target_name).unwrap_or(&true),
                    },
                    multi_return_count,
                },
            );
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
            trampoline_entries,
        }
    }
}

fn push_trampoline_entry(
    table_indices: &mut Vec<u32>,
    trampoline_entries: &mut Vec<WasmCallableTrampolineEntry>,
    entry: WasmCallableTrampolineEntry,
) {
    table_indices.push(entry.expected_func_index);
    trampoline_entries.push(entry);
}
