use std::collections::{BTreeMap, BTreeSet};

use wasm_encoder::{EntityType, ExportKind, RefType, TableType};

use super::super::poll_table::WasmPollTableLayout;
use super::runtime_callables::WasmRuntimeCallableTablePlan;
use super::{WasmCallableTableEntry, WasmCallableTablePlan, WasmCallableTrampolineEntry};
use crate::wasm::WasmBackend;
use crate::wasm::module_abi::user_functions::WasmUserFunctionImports;
use crate::wasm_abi::{
    RESERVED_RUNTIME_CALLABLE_COUNT, RESERVED_RUNTIME_CALLABLE_SPECS,
    ReservedRuntimeCallableDispatch, runtime_callable_import, wasm_runtime_import,
};
use crate::wasm_table::{
    WasmCallableTableAddress, WasmCallableTableRole, WasmCallableTableTarget, WasmFunctionSymbol,
};
use crate::{SimpleIR, TrampolineBehavior, TrampolineKind, TrampolineSpec};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CallableTableRegionLayout {
    fixed_shared_runtime_abi_slot_end: u32,
    reserved_runtime_callable_table_start: u32,
    reserved_runtime_trampoline_table_start: u32,
    compact_builtin_table_start: u32,
    compact_builtin_trampoline_table_start: u32,
    app_callable_resolver_table_start: u32,
    app_callable_resolver_entry_start: Option<u32>,
    user_func_table_start: u32,
    user_trampoline_table_start: u32,
    table_len: u32,
    table_min: u32,
    user_func_start: u32,
    app_callable_resolver_func_index: Option<u32>,
    reserved_runtime_trampoline_func_start: u32,
    compact_builtin_trampoline_func_start: u32,
    user_trampoline_start: u32,
}

impl CallableTableRegionLayout {
    #[allow(clippy::too_many_arguments)]
    fn build(
        table_base: u32,
        fixed_shared_runtime_abi_base: Option<u32>,
        user_func_start: u32,
        poll_table_prefix: u32,
        reserved_runtime_callable_count: u32,
        compact_builtin_count: u64,
        compact_builtin_trampoline_count: u32,
        app_callable_resolver_name_count: u64,
        user_func_count: u64,
        defined_user_func_count: u64,
        reserved_runtime_trampoline_count: u64,
    ) -> Result<Self, String> {
        let compact_builtin_count = checked_u32_count(
            "compact builtin callable-table count",
            compact_builtin_count,
        )?;
        let app_callable_resolver_name_count = checked_u32_count(
            "app callable resolver entry count",
            app_callable_resolver_name_count,
        )?;
        let user_func_count = checked_u32_count("user function count", user_func_count)?;
        let defined_user_func_count =
            checked_u32_count("defined user function count", defined_user_func_count)?;
        let reserved_runtime_trampoline_count = checked_u32_count(
            "reserved runtime trampoline count",
            reserved_runtime_trampoline_count,
        )?;
        let app_callable_resolver_table_len = if app_callable_resolver_name_count == 0 {
            0
        } else {
            checked_add(
                "app callable resolver table length",
                1,
                app_callable_resolver_name_count,
            )?
        };

        let reserved_runtime_callable_table_start = poll_table_prefix;
        let reserved_runtime_trampoline_table_start = checked_add(
            "reserved runtime callable-table end",
            reserved_runtime_callable_table_start,
            reserved_runtime_callable_count,
        )?;
        let compact_builtin_table_start = checked_add(
            "reserved runtime trampoline-table end",
            reserved_runtime_trampoline_table_start,
            reserved_runtime_callable_count,
        )?;
        let compact_builtin_trampoline_table_start = checked_add(
            "compact builtin callable-table end",
            compact_builtin_table_start,
            compact_builtin_count,
        )?;
        let app_callable_resolver_table_start = checked_add(
            "compact builtin trampoline-table end",
            compact_builtin_trampoline_table_start,
            compact_builtin_trampoline_count,
        )?;
        let user_func_table_start = checked_add(
            "app callable resolver-table end",
            app_callable_resolver_table_start,
            app_callable_resolver_table_len,
        )?;
        let app_callable_resolver_entry_start =
            (app_callable_resolver_name_count != 0).then(|| {
                checked_add(
                    "app callable resolver entry start",
                    app_callable_resolver_table_start,
                    1,
                )
            });
        let app_callable_resolver_entry_start = match app_callable_resolver_entry_start {
            Some(result) => Some(result?),
            None => None,
        };
        let user_trampoline_table_start = checked_add(
            "user function-table end",
            user_func_table_start,
            user_func_count,
        )?;
        let table_len = checked_add(
            "user trampoline-table end",
            user_trampoline_table_start,
            user_func_count,
        )?;
        let fixed_shared_runtime_abi_slot_end = if fixed_shared_runtime_abi_base.is_some() {
            compact_builtin_table_start
        } else {
            0
        };
        if let Some(runtime_base) = fixed_shared_runtime_abi_base {
            let runtime_prefix_end = checked_add(
                "fixed shared runtime callable-table boundary",
                runtime_base,
                fixed_shared_runtime_abi_slot_end,
            )?;
            if runtime_prefix_end > table_base {
                return Err(format!(
                    "shared runtime callable-table prefix {runtime_base}..{runtime_prefix_end} overlaps finalized app base {table_base}"
                ));
            }
        }
        let app_table_len = table_len
            .checked_sub(fixed_shared_runtime_abi_slot_end)
            .ok_or_else(|| {
                "shared runtime callable-table prefix exceeds table length".to_string()
            })?;
        let table_min = checked_add(
            "finalized app callable-table boundary",
            table_base,
            app_table_len,
        )?;

        let user_func_end = checked_add(
            "user function-index region end",
            user_func_start,
            defined_user_func_count,
        )?;
        let app_callable_resolver_func_index =
            (app_callable_resolver_name_count != 0).then_some(user_func_end);
        let builtin_trampoline_start = checked_add(
            "app callable resolver function end",
            user_func_end,
            u32::from(app_callable_resolver_func_index.is_some()),
        )?;
        let reserved_runtime_trampoline_func_start = builtin_trampoline_start;
        let compact_builtin_trampoline_func_start = checked_add(
            "reserved runtime trampoline function end",
            reserved_runtime_trampoline_func_start,
            reserved_runtime_trampoline_count,
        )?;
        let user_trampoline_start = checked_add(
            "compact builtin trampoline function end",
            compact_builtin_trampoline_func_start,
            compact_builtin_trampoline_count,
        )?;
        checked_add(
            "user trampoline function end",
            user_trampoline_start,
            user_func_count,
        )?;

        Ok(Self {
            fixed_shared_runtime_abi_slot_end,
            reserved_runtime_callable_table_start,
            reserved_runtime_trampoline_table_start,
            compact_builtin_table_start,
            compact_builtin_trampoline_table_start,
            app_callable_resolver_table_start,
            app_callable_resolver_entry_start,
            user_func_table_start,
            user_trampoline_table_start,
            table_len,
            table_min,
            user_func_start,
            app_callable_resolver_func_index,
            reserved_runtime_trampoline_func_start,
            compact_builtin_trampoline_func_start,
            user_trampoline_start,
        })
    }
}

fn checked_u32_count(label: &str, count: u64) -> Result<u32, String> {
    u32::try_from(count).map_err(|_| format!("{label} {count} exceeds wasm32 u32 capacity"))
}

fn checked_add(label: &str, start: u32, count: u32) -> Result<u32, String> {
    start
        .checked_add(count)
        .ok_or_else(|| format!("{label} overflow: {start} + {count}"))
}

fn usize_count(label: &str, count: usize) -> u64 {
    u64::try_from(count).unwrap_or_else(|_| panic!("{label} exceeds u64 capacity: {count}"))
}

fn usize_index(label: &str, index: usize) -> u32 {
    u32::try_from(index).unwrap_or_else(|_| panic!("{label} exceeds wasm32 u32 capacity: {index}"))
}

fn region_index(label: &str, start: u32, offset: u32) -> u32 {
    checked_add(label, start, offset)
        .unwrap_or_else(|error| panic!("invalid wasm callable-table layout: {error}"))
}

impl WasmBackend {
    pub(in crate::wasm::module_abi) fn build_table_abi(
        &mut self,
        ir: &SimpleIR,
        builtin_trampoline_specs: &BTreeMap<String, usize>,
        direct_import_call_specs: &BTreeMap<String, usize>,
        default_trampoline_spec: &BTreeMap<String, (usize, bool)>,
        task_kinds: &BTreeMap<String, TrampolineKind>,
        task_closure_sizes: &BTreeMap<String, i64>,
        function_abi_returns_value: &BTreeMap<String, bool>,
        user_function_imports: &WasmUserFunctionImports,
        user_type_map: &BTreeMap<usize, u32>,
        reloc_enabled: bool,
        sentinel_func_idx: u32,
        manifest_intrinsic_names: &BTreeSet<String>,
    ) -> WasmCallableTablePlan {
        let runtime_callable_plan = WasmRuntimeCallableTablePlan::build(builtin_trampoline_specs);
        let compact_builtin_table_len = runtime_callable_plan.compact_builtin_table_len();
        let app_callable_resolver_names =
            app_callable_resolver_names(manifest_intrinsic_names, builtin_trampoline_specs)
                .into_iter()
                .filter(|runtime_name| {
                    let import = runtime_callable_import(runtime_name).unwrap_or_else(|| {
                        panic!(
                            "app callable resolver missing generated WASM import: {runtime_name}"
                        )
                    });
                    // Import pruning is the reachability authority.  A resolver row for
                    // a pruned import is not merely dead space: it would publish u32::MAX
                    // as an executable function index.  Size the entire resolver region
                    // from the live import set so table arithmetic and emitted rows stay
                    // identical by construction.
                    self.import_ids[import] != u32::MAX
                })
                .collect::<BTreeSet<_>>();
        let split_runtime_app_table_base = self.options.split_runtime_app_table_base;
        let table_base = split_runtime_app_table_base.unwrap_or(self.options.table_base);
        let fixed_shared_runtime_abi_base =
            split_runtime_app_table_base.map(|_| self.options.table_base);
        let poll_table = WasmPollTableLayout::build();
        let poll_table_prefix = poll_table.prefix_len();
        // Wrapper functions are part of the defined-function prefix. Emit them
        // before freezing the region layout so every downstream user/trampoline
        // index is derived from the actual next function index, not the
        // pre-wrapper snapshot.
        let builtin_wrapper_indices = self.emit_runtime_callable_wrappers(
            &runtime_callable_plan,
            user_type_map,
            reloc_enabled,
        );
        let table_import_wrappers =
            poll_table.emit_import_wrappers(self, reloc_enabled, user_type_map);
        let reserved_runtime_trampoline_count = RESERVED_RUNTIME_CALLABLE_SPECS.len();
        let layout = CallableTableRegionLayout::build(
            table_base,
            fixed_shared_runtime_abi_base,
            self.func_count,
            poll_table_prefix,
            RESERVED_RUNTIME_CALLABLE_COUNT,
            usize_count(
                "compact builtin callable-table count",
                compact_builtin_table_len,
            ),
            runtime_callable_plan.compact_builtin_trampoline_count(),
            usize_count(
                "app callable resolver entry count",
                app_callable_resolver_names.len(),
            ),
            usize_count("user function count", ir.functions.len()),
            usize_count(
                "defined user function count",
                ir.functions
                    .iter()
                    .filter(|function| !function.is_extern)
                    .count(),
            ),
            usize_count(
                "reserved runtime trampoline count",
                reserved_runtime_trampoline_count,
            ),
        )
        .unwrap_or_else(|error| panic!("invalid wasm callable-table layout: {error}"));
        let table_ty = TableType {
            element_type: RefType::FUNCREF,
            table64: false,
            minimum: u64::from(layout.table_min),
            maximum: None,
            shared: false,
        };
        self.imports.import(
            "env",
            "__indirect_function_table",
            EntityType::Table(table_ty),
        );
        self.exports.export("molt_table", ExportKind::Table, 0);

        // Slot-addressed table construction: every region writes its entries at
        // the arithmetic slot the rest of the pipeline (resolver metadata,
        // trampoline maps, manifest, host) uses. Positional pushes are banned —
        // a push-order/arithmetic mismatch silently rebinds callable slots
        // (observed as the app-callable-resolver executing the wrong runtime
        // function). Double writes and unwritten slots fail the build loudly.
        let mut slots = WasmCallableTableSlots::new(layout.table_len);
        for (slot, func_index) in poll_table
            .initial_table_indices(&table_import_wrappers, &self.import_ids, sentinel_func_idx)
            .into_iter()
            .enumerate()
        {
            slots.set(
                usize_index("poll callable-table slot", slot),
                func_index,
                "poll table prefix",
            );
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

        let reserved_runtime_callable_table_start = layout.reserved_runtime_callable_table_start;
        let reserved_runtime_trampoline_table_start =
            layout.reserved_runtime_trampoline_table_start;
        let compact_builtin_table_start = layout.compact_builtin_table_start;
        let compact_builtin_trampoline_table_start = layout.compact_builtin_trampoline_table_start;
        let app_callable_resolver_table_start = layout.app_callable_resolver_table_start;
        let user_func_table_start = layout.user_func_table_start;
        let user_trampoline_table_start = layout.user_trampoline_table_start;
        let split_runtime_shared_abi_slot_end =
            usize::try_from(layout.fixed_shared_runtime_abi_slot_end)
                .expect("u32 callable-table prefix must fit usize");

        for spec in RESERVED_RUNTIME_CALLABLE_SPECS {
            let runtime_name = spec.runtime_name.to_string();
            let import_idx = self.import_ids[spec.import];
            let direct_target_idx = if spec.dispatch == ReservedRuntimeCallableDispatch::Direct {
                if import_idx == u32::MAX {
                    panic!("reserved runtime callable unexpectedly stripped for {runtime_name}");
                }
                import_idx
            } else {
                sentinel_func_idx
            };
            let table_slot = region_index(
                "reserved runtime callable-table slot",
                reserved_runtime_callable_table_start,
                spec.index,
            );
            func_to_table_idx.insert(runtime_name.clone(), table_slot);
            func_to_index.insert(runtime_name, direct_target_idx);
            slots.set(table_slot, direct_target_idx, spec.runtime_name);
        }

        let mut compact_slot = 0u32;
        for callable in runtime_callable_plan.compact_builtin_runtime_callables() {
            let runtime_key = callable.runtime_name.clone();
            let idx = region_index(
                "compact builtin callable-table slot",
                compact_builtin_table_start,
                compact_slot,
            );
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
            compact_slot = compact_slot
                .checked_add(1)
                .expect("compact builtin callable-table offset overflow");
        }
        debug_assert_eq!(
            usize::try_from(compact_slot).expect("u32 compact slot must fit usize"),
            compact_builtin_table_len,
            "compact slot count must match pre-computed builtin_table_len"
        );

        let user_func_start = layout.user_func_start;
        let app_callable_resolver_func_index = layout.app_callable_resolver_func_index;
        let reserved_runtime_trampoline_func_start = layout.reserved_runtime_trampoline_func_start;
        let compact_builtin_trampoline_func_start = layout.compact_builtin_trampoline_func_start;
        let user_trampoline_start = layout.user_trampoline_start;

        let mut func_to_trampoline_idx = BTreeMap::new();
        let mut trampoline_entries = Vec::new();
        let mut reserved_runtime_trampoline_func_offset = 0u32;
        for spec in RESERVED_RUNTIME_CALLABLE_SPECS {
            let runtime_name = spec.runtime_name.to_string();
            let trampoline_table_idx = region_index(
                "reserved runtime trampoline-table slot",
                reserved_runtime_trampoline_table_start,
                spec.index,
            );
            func_to_trampoline_idx.insert(runtime_name.clone(), trampoline_table_idx);
            let import_idx = self.import_ids[spec.import];
            if import_idx == u32::MAX {
                panic!("reserved runtime callable unexpectedly stripped for {runtime_name}");
            }
            let expected_func_index = region_index(
                "reserved runtime trampoline function index",
                reserved_runtime_trampoline_func_start,
                reserved_runtime_trampoline_func_offset,
            );
            reserved_runtime_trampoline_func_offset = reserved_runtime_trampoline_func_offset
                .checked_add(1)
                .expect("reserved runtime trampoline function offset overflow");
            push_trampoline_entry(
                &mut slots,
                trampoline_table_idx,
                &mut trampoline_entries,
                WasmCallableTrampolineEntry {
                    name: runtime_name,
                    expected_func_index,
                    target_func_index: import_idx,
                    target: callable_target(
                        self,
                        user_function_imports,
                        table_base,
                        fixed_shared_runtime_abi_base,
                        split_runtime_shared_abi_slot_end,
                        trampoline_table_idx,
                        expected_func_index,
                        WasmCallableTableRole::Trampoline,
                    ),
                    spec: TrampolineSpec {
                        arity: spec.arity,
                        has_closure: false,
                        kind: spec.trampoline_abi.trampoline_kind(),
                        closure_size: 0,
                        target_has_ret: true,
                    },
                },
            );
        }
        let mut app_callable_resolver = None;
        if let Some(resolver_func_index) = app_callable_resolver_func_index {
            let resolver_slot = app_callable_resolver_table_start;
            slots.set(resolver_slot, resolver_func_index, "app callable resolver");
            let mut entries = Vec::new();
            for (idx, runtime_name) in app_callable_resolver_names.iter().enumerate() {
                let import = runtime_callable_import(runtime_name).unwrap_or_else(|| {
                    panic!("app callable resolver missing generated WASM import: {runtime_name}")
                });
                let import_idx = self.import_ids[import];
                debug_assert_ne!(import_idx, u32::MAX);
                let table_slot = region_index(
                    "app callable resolver entry slot",
                    layout
                        .app_callable_resolver_entry_start
                        .expect("resolver entries require a resolver region"),
                    usize_index("app callable resolver entry offset", idx),
                );
                slots.set(table_slot, import_idx, runtime_name);
                entries.push(super::WasmAppCallableResolverEntry {
                    name: runtime_name.clone(),
                    target: callable_target(
                        self,
                        user_function_imports,
                        table_base,
                        fixed_shared_runtime_abi_base,
                        split_runtime_shared_abi_slot_end,
                        table_slot,
                        import_idx,
                        WasmCallableTableRole::ResolverEntry,
                    ),
                });
            }
            app_callable_resolver = Some(super::WasmAppCallableResolverPlan {
                resolver_func_index,
                resolver_target: callable_target(
                    self,
                    user_function_imports,
                    table_base,
                    fixed_shared_runtime_abi_base,
                    split_runtime_shared_abi_slot_end,
                    resolver_slot,
                    resolver_func_index,
                    WasmCallableTableRole::AppCallableResolver,
                ),
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
            let offset = usize_index("compact builtin trampoline offset", i);
            let idx = region_index(
                "compact builtin trampoline-table slot",
                compact_builtin_trampoline_table_start,
                offset,
            );
            func_to_trampoline_idx.insert(name.clone(), idx);
            let expected_func_index = region_index(
                "compact builtin trampoline function index",
                compact_builtin_trampoline_func_start,
                offset,
            );
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
                    target: callable_target(
                        self,
                        user_function_imports,
                        table_base,
                        fixed_shared_runtime_abi_base,
                        split_runtime_shared_abi_slot_end,
                        table_slot,
                        target_func_index,
                        WasmCallableTableRole::DirectCallable,
                    ),
                    spec: TrampolineSpec {
                        arity: *arity,
                        has_closure: false,
                        kind: TrampolineKind::Plain,
                        closure_size: 0,
                        target_has_ret: true,
                    },
                },
            );
        }
        let mut defined_user_function_offset = 0u32;
        for (i, func_ir) in ir.functions.iter().enumerate() {
            let offset = usize_index("user callable offset", i);
            let idx = region_index("user function-table slot", user_func_table_start, offset);
            let func_index = if func_ir.is_extern {
                user_function_imports
                    .function_index(&func_ir.name)
                    .unwrap_or_else(|| {
                        panic!("missing WASM extern import index for {}", func_ir.name)
                    })
            } else {
                let func_index = region_index(
                    "defined user function index",
                    user_func_start,
                    defined_user_function_offset,
                );
                defined_user_function_offset = defined_user_function_offset
                    .checked_add(1)
                    .expect("defined WASM user function offset overflow");
                func_index
            };
            func_to_table_idx.insert(func_ir.name.clone(), idx);
            func_to_index.insert(func_ir.name.clone(), func_index);
            slots.set(idx, func_index, &func_ir.name);
        }
        for (i, func_ir) in ir.functions.iter().enumerate() {
            let offset = usize_index("user trampoline offset", i);
            let idx = region_index(
                "user trampoline-table slot",
                user_trampoline_table_start,
                offset,
            );
            func_to_trampoline_idx.insert(func_ir.name.clone(), idx);
            let expected_func_index = region_index(
                "user trampoline function index",
                user_trampoline_start,
                offset,
            );
            let (arity, has_closure) = *default_trampoline_spec
                .get(&func_ir.name)
                .unwrap_or_else(|| panic!("missing trampoline spec for {}", func_ir.name));
            let kind = task_kinds
                .get(&func_ir.name)
                .copied()
                .unwrap_or(TrampolineKind::Plain);
            let is_task = matches!(kind.behavior(), TrampolineBehavior::Task(_));
            let poll_name = if is_task && !func_ir.name.ends_with("_poll") {
                format!("{}_poll", func_ir.name)
            } else {
                func_ir.name.clone()
            };
            let target_name = if is_task { &poll_name } else { &func_ir.name };
            let target_func_index = *func_to_index
                .get(target_name)
                .unwrap_or_else(|| panic!("trampoline target missing for {target_name}"));
            let table_slot = *func_to_table_idx
                .get(target_name)
                .unwrap_or_else(|| panic!("trampoline table slot missing for {target_name}"));
            let closure_size = if is_task {
                *task_closure_sizes
                    .get(&func_ir.name)
                    .unwrap_or_else(|| panic!("task closure size missing for {}", func_ir.name))
            } else {
                0
            };
            push_trampoline_entry(
                &mut slots,
                idx,
                &mut trampoline_entries,
                WasmCallableTrampolineEntry {
                    name: func_ir.name.clone(),
                    expected_func_index,
                    target_func_index,
                    target: callable_target(
                        self,
                        user_function_imports,
                        table_base,
                        fixed_shared_runtime_abi_base,
                        split_runtime_shared_abi_slot_end,
                        table_slot,
                        target_func_index,
                        WasmCallableTableRole::DirectCallable,
                    ),
                    spec: TrampolineSpec {
                        arity,
                        has_closure,
                        kind,
                        closure_size,
                        target_has_ret: *function_abi_returns_value
                            .get(target_name)
                            .unwrap_or(&true),
                    },
                },
            );
        }

        let table_entries = slots
            .finish()
            .into_iter()
            .map(|func_index| WasmCallableTableEntry {
                func_index,
                symbol: function_symbol(self, user_function_imports, func_index),
            })
            .collect();

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
        let mut call_target_abi_returns_value = func_to_index
            .keys()
            .map(|name| (name.clone(), true))
            .collect::<BTreeMap<_, _>>();
        call_target_abi_returns_value.extend(
            function_abi_returns_value
                .iter()
                .map(|(name, returns_value)| (name.clone(), *returns_value)),
        );

        WasmCallableTablePlan {
            table_base,
            fixed_shared_runtime_abi_base,
            table_entries,
            split_runtime_shared_abi_slot_end,
            func_to_table_idx,
            func_to_index,
            func_to_trampoline_idx,
            app_callable_resolver,
            closure_functions,
            function_abi_returns_value: call_target_abi_returns_value,
            trampoline_entries,
        }
    }
}

fn function_symbol(
    backend: &WasmBackend,
    user_function_imports: &WasmUserFunctionImports,
    func_index: u32,
) -> WasmFunctionSymbol {
    if func_index < backend.func_import_count {
        if let Some(import) = backend.import_ids.import_for_index(func_index) {
            return WasmFunctionSymbol::RuntimeImport(import);
        }
        if let Some(user_import_ordinal) = user_function_imports.import_ordinal(func_index) {
            return WasmFunctionSymbol::UserImport {
                user_import_ordinal,
            };
        }
        panic!(
            "callable table target import index {func_index} has neither runtime nor user-function import identity"
        );
    }
    WasmFunctionSymbol::Defined {
        defined_func_index: func_index - backend.func_import_count,
    }
}

fn callable_target(
    backend: &WasmBackend,
    user_function_imports: &WasmUserFunctionImports,
    table_base: u32,
    fixed_shared_runtime_abi_base: Option<u32>,
    shared_runtime_abi_slot_end: usize,
    slot: u32,
    func_index: u32,
    role: WasmCallableTableRole,
) -> WasmCallableTableTarget {
    let fixed_prefix_len = u32::try_from(shared_runtime_abi_slot_end)
        .expect("shared runtime callable-table prefix exceeds wasm32 u32 capacity");
    let (current_table_index, address) = if slot < fixed_prefix_len {
        let base = fixed_shared_runtime_abi_base
            .expect("fixed callable-table slot requires shared runtime base");
        (
            base.checked_add(slot)
                .expect("fixed callable-table address overflow"),
            WasmCallableTableAddress::FixedSharedRuntimeAbi {
                finalized_app_base: table_base,
            },
        )
    } else {
        (
            table_base
                .checked_add(slot - fixed_prefix_len)
                .expect("relocatable callable-table address overflow"),
            WasmCallableTableAddress::Relocatable(function_symbol(
                backend,
                user_function_imports,
                func_index,
            )),
        )
    };
    WasmCallableTableTarget {
        current_table_index,
        address,
        role,
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
/// and linker relocation/active-element publication. Every region writes
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
            slots: vec![
                None;
                usize::try_from(table_len)
                    .expect("wasm32 callable-table length must fit host usize")
            ],
        }
    }

    fn set(&mut self, slot: u32, func_index: u32, owner: &str) {
        let len = self.slots.len();
        let host_slot =
            usize::try_from(slot).expect("wasm32 callable-table slot must fit host usize");
        let cell = self.slots.get_mut(host_slot).unwrap_or_else(|| {
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

    fn test_region_layout(
        table_base: u32,
        fixed_runtime_base: Option<u32>,
        function_base: u32,
        app_resolver_names: u64,
        user_functions: u64,
    ) -> Result<CallableTableRegionLayout, String> {
        CallableTableRegionLayout::build(
            table_base,
            fixed_runtime_base,
            function_base,
            3,
            4,
            5,
            5,
            app_resolver_names,
            user_functions,
            user_functions,
            2,
        )
    }

    #[test]
    fn extern_user_functions_consume_table_slots_but_not_defined_function_indices() {
        let layout = CallableTableRegionLayout::build(64, None, 100, 3, 4, 5, 5, 2, 7, 5, 2)
            .expect("five definitions plus two extern declarations");

        assert_eq!(layout.user_trampoline_table_start, 31);
        assert_eq!(layout.table_len, 38);
        assert_eq!(layout.app_callable_resolver_func_index, Some(105));
        assert_eq!(layout.reserved_runtime_trampoline_func_start, 106);
        assert_eq!(layout.user_trampoline_start, 113);
    }

    #[test]
    fn callable_table_region_layout_derives_every_boundary_once() {
        let layout = test_region_layout(64, None, 100, 2, 7).expect("valid layout");

        assert_eq!(layout.reserved_runtime_callable_table_start, 3);
        assert_eq!(layout.reserved_runtime_trampoline_table_start, 7);
        assert_eq!(layout.compact_builtin_table_start, 11);
        assert_eq!(layout.compact_builtin_trampoline_table_start, 16);
        assert_eq!(layout.app_callable_resolver_table_start, 21);
        assert_eq!(layout.app_callable_resolver_entry_start, Some(22));
        assert_eq!(layout.user_func_table_start, 24);
        assert_eq!(layout.user_trampoline_table_start, 31);
        assert_eq!(layout.table_len, 38);
        assert_eq!(layout.table_min, 102);
        assert_eq!(layout.app_callable_resolver_func_index, Some(107));
        assert_eq!(layout.reserved_runtime_trampoline_func_start, 108);
        assert_eq!(layout.compact_builtin_trampoline_func_start, 110);
        assert_eq!(layout.user_trampoline_start, 115);
    }

    #[test]
    fn callable_table_region_layout_rejects_count_above_wasm32_capacity() {
        let error = test_region_layout(0, None, 0, 0, u64::from(u32::MAX) + 1)
            .expect_err("oversized user function count must fail");

        assert!(error.contains("user function count"), "{error}");
        assert!(error.contains("exceeds wasm32 u32 capacity"), "{error}");
    }

    #[test]
    fn callable_table_region_layout_rejects_resolver_length_overflow() {
        let error = test_region_layout(0, None, 0, u64::from(u32::MAX), 0)
            .expect_err("resolver header plus entries must not wrap");

        assert!(
            error.contains("app callable resolver table length overflow"),
            "{error}"
        );
    }

    #[test]
    fn callable_table_region_layout_rejects_table_boundary_overflow() {
        let error = test_region_layout(u32::MAX, None, 0, 0, 1)
            .expect_err("table base plus live app region must not wrap");

        assert!(
            error.contains("finalized app callable-table boundary overflow"),
            "{error}"
        );
    }

    #[test]
    fn callable_table_region_layout_rejects_function_boundary_overflow() {
        let error = test_region_layout(0, None, u32::MAX, 0, 1)
            .expect_err("function base plus user functions must not wrap");

        assert!(
            error.contains("user function-index region end overflow"),
            "{error}"
        );
    }

    #[test]
    fn callable_table_region_layout_rejects_fixed_prefix_overlap() {
        let error = test_region_layout(10, Some(0), 0, 0, 0)
            .expect_err("fixed runtime prefix must end before the app base");

        assert!(error.contains("overlaps finalized app base"), "{error}");
    }

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
