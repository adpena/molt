use std::collections::{BTreeMap, BTreeSet};

use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm::container_runtime_select::selected_container_runtime_import;
use crate::wasm::lir_fast::WasmFunctionLoweringPlans;
use crate::wasm::method_ic_select::selected_method_ic_runtime;
use crate::wasm::object_new_bound_select::selected_object_new_bound_runtime;
use crate::wasm::task_runtime::WasmTaskRuntimeLayout;
use crate::wasm_abi::{
    WasmRuntimeImport, runtime_callable_arity, runtime_callable_import, wasm_runtime_import,
};
use crate::wasm_abi_generated::{op_loop_runtime_call, wasm_bulk_memory_op};
use crate::wasm_imports::{OP_IMPORT_DEPS, runtime_surface_requires_direct_import};
use crate::wasm_options::{WasmCompileOptions, WasmProfile};
use crate::{OpIR, TrampolineKind};

type RuntimeImportDepsMap = BTreeMap<&'static str, &'static [WasmRuntimeImport]>;

pub(super) struct WasmRuntimeImportDemand {
    auto_required_imports: Option<BTreeSet<WasmRuntimeImport>>,
    deps_map: RuntimeImportDepsMap,
}

impl WasmRuntimeImportDemand {
    pub(super) fn new(options: &WasmCompileOptions) -> Self {
        let deps_map: RuntimeImportDepsMap = OP_IMPORT_DEPS
            .iter()
            .map(|&(kind, deps)| (kind, deps))
            .collect();
        let auto_required_imports = initial_auto_required_imports(options, &deps_map);
        Self {
            auto_required_imports,
            deps_map,
        }
    }

    pub(super) fn auto_required_imports(&self) -> Option<&BTreeSet<WasmRuntimeImport>> {
        self.auto_required_imports.as_ref()
    }

    pub(super) fn observe_op(
        &mut self,
        func_name: &str,
        op: &OpIR,
        op_index: usize,
        is_poll: bool,
        scalar_plan: &ScalarRepresentationPlan,
        defined_function_names: &BTreeSet<&str>,
        known_imports: &BTreeSet<WasmRuntimeImport>,
    ) {
        let kind = op.kind.as_str();
        if debug_imports_enabled() && op.s_value.as_deref() == Some("__main_____f_poll") {
            eprintln!(
                "WASM_IMPORTS saw_op kind={} s_value={:?} task_kind={:?} args={:?} func={}",
                kind, op.s_value, op.task_kind, op.args, func_name
            );
        }

        if kind == "object_new_bound" {
            self.require_import(selected_object_new_bound_runtime(op).import);
            return;
        }
        if let Some(selected) = selected_method_ic_runtime(op) {
            self.require_import(selected.import);
            return;
        }

        if kind == "alloc_task" {
            let layout = WasmTaskRuntimeLayout::for_alloc_task_kind(op.task_kind.as_deref());
            self.require_task_layout(
                layout,
                op.args.as_ref().is_some_and(|args| !args.is_empty()),
            );
        } else if kind == "call_async" {
            self.require_task_layout(
                WasmTaskRuntimeLayout::for_call_async(),
                op.args.as_ref().is_some_and(|args| !args.is_empty()),
            );
        } else if let Some(call) = op_loop_runtime_call(kind) {
            self.require_imports(call.required_imports);
        } else if wasm_bulk_memory_op(kind).is_some() {
            // WASM-native bulk-memory ops emit no runtime import. Keep the
            // no-demand fact generated beside their emission spec.
        } else if let Some(deps) = self.deps_map.get(kind).copied() {
            self.require_imports(deps);
        } else if let Some(import) = wasm_runtime_import(kind)
            && known_imports.contains(&import)
        {
            self.require_import(import);
        }
        if crate::tir::op_kinds_generated::kind_result_mints_owned_selected_operand_table(kind)
            && op.out.is_some()
        {
            self.require_import(WasmRuntimeImport::IncRefObj);
        }

        if kind == "builtin_func"
            && let Some(name) = op.s_value.as_ref()
        {
            if let Some(import) = runtime_callable_import(name) {
                self.require_import(import);
            } else if runtime_callable_arity(name).is_none() {
                panic!("builtin runtime callable missing from WASM ABI manifest: {name}");
            }
        }
        if kind == "call"
            && let Some(name) = op.s_value.as_ref()
            && !defined_function_names.contains(name.as_str())
        {
            let import = wasm_runtime_import(name);
            if name.starts_with("molt_") && import.is_none() {
                panic!("direct runtime call missing WASM ABI manifest import: {name}");
            }
            if let Some(import) = import
                && known_imports.contains(&import)
            {
                self.require_import(import);
            }
        }
        if kind == "call_async"
            && let Some(name) = op.s_value.as_ref()
        {
            if let Some(import) = wasm_runtime_import(name)
                && known_imports.contains(&import)
            {
                self.require_import(import);
            }
        }

        if let Some(import) = selected_container_runtime_import(scalar_plan, op_index, kind, op) {
            self.require_import(import);
        }

        if runtime_surface_requires_direct_import(kind) {
            self.require_import_name(kind);
        }
        if is_poll
            && (kind == "call_func" || kind == "invoke_ffi")
            && let Some(name) = op.s_value.as_ref()
            && name.ends_with("_poll")
        {
            self.require_import_name(name);
        }
    }

    pub(super) fn finish(
        &mut self,
        lir_lowering_plans: &WasmFunctionLoweringPlans,
        task_kinds: &BTreeMap<String, TrampolineKind>,
    ) {
        let Some(required) = self.auto_required_imports.as_mut() else {
            return;
        };
        for kind in task_kinds.values() {
            if let Some(layout) = WasmTaskRuntimeLayout::for_trampoline_kind(*kind) {
                layout.extend_required_imports(required, true);
            }
        }
        for plan in lir_lowering_plans.values() {
            if let Some(output) = plan.lir_fast_body() {
                required.extend(output.runtime_imports());
            }
        }
    }

    fn require_import(&mut self, import: WasmRuntimeImport) {
        if let Some(required) = self.auto_required_imports.as_mut() {
            required.insert(import);
        }
    }

    fn require_task_layout(&mut self, layout: WasmTaskRuntimeLayout, has_payload_slots: bool) {
        if let Some(required) = self.auto_required_imports.as_mut() {
            layout.extend_required_imports(required, has_payload_slots);
        }
    }

    fn require_import_name(&mut self, name: &str) {
        let import = wasm_runtime_import(name)
            .unwrap_or_else(|| panic!("runtime import {name} missing generated import token"));
        self.require_import(import);
    }

    fn require_imports(&mut self, imports: &[WasmRuntimeImport]) {
        for &import in imports {
            self.require_import(import);
        }
    }
}

fn initial_auto_required_imports(
    options: &WasmCompileOptions,
    deps_map: &RuntimeImportDepsMap,
) -> Option<BTreeSet<WasmRuntimeImport>> {
    if options.wasm_profile != WasmProfile::Auto || !options.reloc_enabled {
        return None;
    }

    let mut required = BTreeSet::new();
    if let Some(structural) = deps_map.get("__structural__") {
        required.extend(structural.iter().copied());
    }
    if let Ok(extra_required) = std::env::var("MOLT_WASM_EXTRA_REQUIRED_IMPORTS") {
        required.extend(
            extra_required
                .split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(|name| {
                    wasm_runtime_import(name).unwrap_or_else(|| {
                        panic!(
                            "MOLT_WASM_EXTRA_REQUIRED_IMPORTS references unknown runtime import {name}"
                        )
                    })
                }),
        );
    }
    Some(required)
}

fn debug_imports_enabled() -> bool {
    matches!(
        std::env::var("MOLT_DEBUG_WASM_IMPORTS").ok().as_deref(),
        Some("1")
    )
}
