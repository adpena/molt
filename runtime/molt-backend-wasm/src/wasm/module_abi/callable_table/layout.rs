use std::collections::{BTreeMap, BTreeSet};

use wasm_encoder::{EntityType, ExportKind, RefType, TableType};

use super::super::poll_table::WasmPollTableLayout;
use super::runtime_callables::WasmRuntimeCallableTablePlan;
use super::{WasmCallableTablePlan, WasmCallableTrampolineEntry};
use crate::wasm::WasmBackend;
use crate::wasm_abi::{
    RESERVED_RUNTIME_CALLABLE_COUNT, RESERVED_RUNTIME_CALLABLE_SPECS,
    ReservedRuntimeCallableDispatch, runtime_callable_import, wasm_runtime_import,
};
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
        manifest_intrinsic_names: &BTreeSet<String>,
    ) -> WasmCallableTablePlan {
        let runtime_callable_plan = WasmRuntimeCallableTablePlan::build(builtin_trampoline_specs);
        let compact_builtin_table_len = runtime_callable_plan.compact_builtin_table_len();
        let app_callable_resolver_names =
            app_callable_resolver_names(manifest_intrinsic_names, builtin_trampoline_specs);
        let app_callable_resolver_table_len = if app_callable_resolver_names.is_empty() {
            0
        } else {
            1 + app_callable_resolver_names.len()
        };
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
            + app_callable_resolver_table_len
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
        // Slot-addressed table construction: every region writes its entries at
        // the arithmetic slot the rest of the pipeline (resolver metadata,
        // trampoline maps, manifest, host) uses. Positional pushes are banned —
        // a push-order/arithmetic mismatch silently rebinds callable slots
        // (observed as the app-callable-resolver executing the wrong runtime
        // function). Double writes and unwritten slots fail the build loudly.
        let mut slots = WasmCallableTableSlots::new(table_len);
        for (slot, func_index) in poll_table
            .initial_table_indices(&table_import_wrappers, &self.import_ids, sentinel_func_idx)
            .into_iter()
            .enumerate()
        {
            slots.set(slot as u32, func_index, "poll table prefix");
        }
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
        let app_callable_resolver_table_start = compact_builtin_trampoline_table_start
            + runtime_callable_plan.compact_builtin_trampoline_count();
        let user_func_table_start =
            app_callable_resolver_table_start + app_callable_resolver_table_len as u32;
        let user_trampoline_table_start = user_func_table_start + ir.functions.len() as u32;

        let reserved_runtime_trampoline_count = RESERVED_RUNTIME_CALLABLE_SPECS
            .iter()
            .filter(|spec| {
                spec.import.is_some() && builtin_trampoline_specs.contains_key(spec.runtime_name)
            })
            .count() as u32;

        for spec in RESERVED_RUNTIME_CALLABLE_SPECS {
            let runtime_name = spec.runtime_name.to_string();
            let import_idx = spec
                .import
                .map(|import| self.import_ids[import])
                .unwrap_or(sentinel_func_idx);
            let reachable_as_builtin = builtin_trampoline_specs.contains_key(spec.runtime_name);
            let reachable_as_direct = direct_import_call_specs.contains_key(spec.runtime_name);
            let direct_target_idx = if spec.dispatch == ReservedRuntimeCallableDispatch::Direct
                && (reachable_as_builtin || reachable_as_direct)
                && spec.import.is_some()
            {
                if import_idx == u32::MAX {
                    panic!("reserved runtime callable unexpectedly stripped for {runtime_name}");
                }
                import_idx
            } else {
                sentinel_func_idx
            };
            func_to_table_idx.insert(
                runtime_name.clone(),
                reserved_runtime_callable_table_start + spec.index,
            );
            func_to_index.insert(runtime_name, direct_target_idx);
            slots.set(
                reserved_runtime_callable_table_start + spec.index,
                direct_target_idx,
                spec.runtime_name,
            );
        }

        let mut compact_slot = 0u32;
        for callable in runtime_callable_plan.compact_builtin_runtime_callables() {
            let runtime_key = callable.runtime_name.clone();
            let idx = compact_slot + compact_builtin_table_start;
            func_to_table_idx.insert(runtime_key.clone(), idx);
            let target_index = if let Some(wrapper_idx) = builtin_wrapper_indices.get(&runtime_key)
            {
                func_to_index.insert(runtime_key.clone(), *wrapper_idx);
                *wrapper_idx
            } else {
                let import_idx = self
                    .import_ids
                    .get(callable.import)
                    .copied()
                    .unwrap_or(sentinel_func_idx);
                let safe = if import_idx == u32::MAX {
                    sentinel_func_idx
                } else {
                    import_idx
                };
                func_to_index.insert(runtime_key.clone(), safe);
                safe
            };
            slots.set(idx, target_index, &runtime_key);
            compact_slot += 1;
        }
        debug_assert_eq!(
            compact_slot as usize, compact_builtin_table_len,
            "compact slot count must match pre-computed builtin_table_len"
        );

        let user_func_start = self.func_count;
        let user_func_count = ir.functions.len() as u32;
        let app_callable_resolver_func_index = if app_callable_resolver_names.is_empty() {
            None
        } else {
            Some(user_func_start + user_func_count)
        };
        let app_callable_resolver_func_count =
            u32::from(app_callable_resolver_func_index.is_some());
        let compact_builtin_trampoline_count =
            runtime_callable_plan.compact_builtin_trampoline_count();
        let builtin_trampoline_start =
            user_func_start + user_func_count + app_callable_resolver_func_count;
        let reserved_runtime_trampoline_func_start = builtin_trampoline_start;
        let compact_builtin_trampoline_func_start =
            reserved_runtime_trampoline_func_start + reserved_runtime_trampoline_count;
        let user_trampoline_start =
            compact_builtin_trampoline_func_start + compact_builtin_trampoline_count;

        let mut func_to_trampoline_idx = BTreeMap::new();
        let mut trampoline_entries = Vec::new();
        let mut reserved_runtime_trampoline_func_offset = 0u32;
        for spec in RESERVED_RUNTIME_CALLABLE_SPECS {
            let runtime_name = spec.runtime_name.to_string();
            let trampoline_table_idx = reserved_runtime_trampoline_table_start + spec.index;
            func_to_trampoline_idx.insert(runtime_name.clone(), trampoline_table_idx);
            if let Some(import) = spec.import
                && let Some(arity) = builtin_trampoline_specs.get(spec.runtime_name)
            {
                let import_idx = self.import_ids[import];
                if import_idx == u32::MAX {
                    panic!("reserved runtime callable unexpectedly stripped for {runtime_name}");
                }
                let expected_func_index = reserved_runtime_trampoline_func_start
                    + reserved_runtime_trampoline_func_offset;
                reserved_runtime_trampoline_func_offset += 1;
                push_trampoline_entry(
                    &mut slots,
                    trampoline_table_idx,
                    &mut trampoline_entries,
                    WasmCallableTrampolineEntry {
                        name: runtime_name,
                        expected_func_index,
                        target_func_index: import_idx,
                        table_index: table_base + trampoline_table_idx,
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
            } else {
                slots.set(trampoline_table_idx, sentinel_func_idx, spec.runtime_name);
            }
        }
        let mut app_callable_resolver = None;
        if let Some(resolver_func_index) = app_callable_resolver_func_index {
            let resolver_slot = app_callable_resolver_table_start;
            let resolver_table_index = table_base + resolver_slot;
            slots.set(resolver_slot, resolver_func_index, "app callable resolver");
            let mut entries = Vec::new();
            for (idx, runtime_name) in app_callable_resolver_names.iter().enumerate() {
                let import = runtime_callable_import(runtime_name).unwrap_or_else(|| {
                    panic!("app callable resolver missing generated WASM import: {runtime_name}")
                });
                let import_idx = self.import_ids[import];
                if import_idx == u32::MAX {
                    panic!("app callable resolver import unexpectedly stripped for {runtime_name}");
                }
                let table_slot = app_callable_resolver_table_start + 1 + idx as u32;
                slots.set(table_slot, import_idx, runtime_name);
                entries.push(super::WasmAppCallableResolverEntry {
                    name: runtime_name.clone(),
                    table_index: table_base + table_slot,
                });
            }
            app_callable_resolver = Some(super::WasmAppCallableResolverPlan {
                resolver_func_index,
                resolver_table_index,
                entries,
            });
        }
        for runtime_name in direct_import_call_specs.keys() {
            let import = wasm_runtime_import(runtime_name).unwrap_or_else(|| {
                panic!("missing direct runtime import token for {runtime_name}")
            });
            let import_idx = self.import_ids[import];
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
                &mut slots,
                idx,
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
            slots.set(idx, user_func_start + i as u32, &func_ir.name);
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
                &mut slots,
                idx,
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

        let table_indices = slots.finish();

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
            sentinel_func_idx,
            split_runtime_owned_slot_start,
            split_runtime_shared_abi_slot_end,
            func_to_table_idx,
            func_to_index,
            func_to_trampoline_idx,
            app_callable_resolver,
            closure_functions,
            trampoline_entries,
        }
    }
}

fn push_trampoline_entry(
    slots: &mut WasmCallableTableSlots,
    trampoline_slot: u32,
    trampoline_entries: &mut Vec<WasmCallableTrampolineEntry>,
    entry: WasmCallableTrampolineEntry,
) {
    slots.set(trampoline_slot, entry.expected_func_index, &entry.name);
    trampoline_entries.push(entry);
}

/// Slot-addressed callable-table builder.
///
/// The callable-table layout has exactly one authority: the arithmetic slot
/// numbers consumed by the resolver metadata, trampoline maps, host manifest,
/// and `molt_table_init`/`__molt_table_ref_*` publication. Every region writes
/// its entries at those slots; the element/table content can therefore never
/// drift from the published numbering. Writing a slot twice, writing outside
/// the table, or leaving a slot unwritten is a layout bug and fails the build
/// loudly.
struct WasmCallableTableSlots {
    slots: Vec<Option<u32>>,
}

impl WasmCallableTableSlots {
    fn new(table_len: u32) -> Self {
        Self {
            slots: vec![None; table_len as usize],
        }
    }

    fn set(&mut self, slot: u32, func_index: u32, owner: &str) {
        let len = self.slots.len();
        let cell = self.slots.get_mut(slot as usize).unwrap_or_else(|| {
            panic!(
                "wasm callable table slot {slot} for {owner} is outside the \
                 planned table length {len}"
            )
        });
        if let Some(existing) = cell {
            panic!(
                "wasm callable table slot {slot} written twice: {owner} \
                 (function index {func_index}) collides with function index \
                 {existing}; slot regions must tile the table exactly once"
            );
        }
        *cell = Some(func_index);
    }

    fn finish(self) -> Vec<u32> {
        self.slots
            .into_iter()
            .enumerate()
            .map(|(slot, cell)| {
                cell.unwrap_or_else(|| {
                    panic!(
                        "wasm callable table slot {slot} was never written; \
                         slot regions must tile the table exactly once"
                    )
                })
            })
            .collect()
    }
}

fn app_callable_resolver_names(
    manifest_intrinsic_names: &BTreeSet<String>,
    builtin_trampoline_specs: &BTreeMap<String, usize>,
) -> BTreeSet<String> {
    let mut names = manifest_intrinsic_names.clone();
    names.extend(builtin_trampoline_specs.keys().cloned());
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callable_table_slots_are_write_order_independent() {
        // Regression shape: the app-callable-resolver region used to be
        // pushed positionally BEFORE the compact-builtin trampolines while
        // the arithmetic layout places it after them, shifting every
        // resolver slot by the trampoline count. Slot-addressed writes make
        // source order irrelevant.
        let mut slots = WasmCallableTableSlots::new(4);
        slots.set(2, 200, "resolver");
        slots.set(3, 300, "resolver entry");
        slots.set(0, 100, "compact trampoline 0");
        slots.set(1, 101, "compact trampoline 1");
        assert_eq!(slots.finish(), vec![100, 101, 200, 300]);
    }

    #[test]
    #[should_panic(expected = "written twice")]
    fn callable_table_slots_reject_double_writes() {
        let mut slots = WasmCallableTableSlots::new(2);
        slots.set(1, 7, "first owner");
        slots.set(1, 8, "second owner");
    }

    #[test]
    #[should_panic(expected = "outside the planned table length")]
    fn callable_table_slots_reject_out_of_range_writes() {
        let mut slots = WasmCallableTableSlots::new(2);
        slots.set(2, 7, "overflow owner");
    }

    #[test]
    #[should_panic(expected = "never written")]
    fn callable_table_slots_reject_unwritten_slots() {
        let mut slots = WasmCallableTableSlots::new(2);
        slots.set(0, 7, "only slot");
        let _ = slots.finish();
    }

    #[test]
    fn app_callable_resolver_names_include_intrinsics_and_builtins() {
        let names = app_callable_resolver_names(
            &BTreeSet::from(["molt_json_parse_scalar".to_string()]),
            &BTreeMap::from([("molt_len".to_string(), 1usize)]),
        );

        assert_eq!(
            names,
            BTreeSet::from(["molt_json_parse_scalar".to_string(), "molt_len".to_string()])
        );
    }
}
