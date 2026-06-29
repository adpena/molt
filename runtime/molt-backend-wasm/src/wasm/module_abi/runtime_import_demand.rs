use std::collections::{BTreeMap, BTreeSet};

use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm::container_runtime_select::selected_container_runtime_import;
use crate::wasm::lir_fast::WasmFunctionLoweringPlans;
use crate::wasm::method_ic_select::selected_method_ic_runtime;
use crate::wasm::object_new_bound_select::selected_object_new_bound_runtime;
use crate::wasm_abi::RESERVED_RUNTIME_CALLABLE_SPECS;
use crate::wasm_abi_generated::{op_loop_runtime_call, wasm_bulk_memory_op};
use crate::wasm_imports::{OP_IMPORT_DEPS, runtime_surface_requires_direct_import};
use crate::wasm_options::{WasmCompileOptions, WasmProfile};
use crate::{OpIR, TrampolineKind};

type RuntimeImportDepsMap = BTreeMap<&'static str, &'static [&'static str]>;

pub(super) struct WasmRuntimeImportDemand {
    auto_required_imports: Option<BTreeSet<String>>,
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

    pub(super) fn auto_required_imports(&self) -> Option<&BTreeSet<String>> {
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
        known_imports: &BTreeSet<&str>,
    ) {
        let kind = op.kind.as_str();
        if debug_imports_enabled() && op.s_value.as_deref() == Some("__main_____f_poll") {
            eprintln!(
                "WASM_IMPORTS saw_op kind={} s_value={:?} task_kind={:?} args={:?} func={}",
                kind, op.s_value, op.task_kind, op.args, func_name
            );
        }

        if kind == "object_new_bound" {
            self.require_import(selected_object_new_bound_runtime(op).import_name);
            return;
        }
        if let Some(selected) = selected_method_ic_runtime(op) {
            self.require_import(selected.import_name);
            return;
        }

        if let Some(call) = op_loop_runtime_call(kind) {
            self.require_imports(call.required_imports);
        } else if wasm_bulk_memory_op(kind).is_some() {
            // WASM-native bulk-memory ops emit no runtime import. Keep the
            // no-demand fact generated beside their emission spec.
        } else if let Some(deps) = self.deps_map.get(kind).copied() {
            if debug_imports_enabled() && kind == "alloc_task" {
                eprintln!("WASM_IMPORTS alloc_task deps={deps:?} func={}", func_name);
            }
            self.require_imports(deps);
        } else if known_imports.contains(kind) {
            self.require_import(kind);
        }
        if crate::tir::op_kinds_generated::kind_result_mints_owned_selected_operand_table(kind)
            && op.out.is_some()
        {
            self.require_import("inc_ref_obj");
        }

        if kind == "builtin_func"
            && let Some(name) = op.s_value.as_ref()
        {
            let import_name =
                crate::wasm_abi::runtime_callable_import_name(name).unwrap_or_else(|| {
                    panic!("builtin runtime callable missing from WASM ABI manifest: {name}")
                });
            self.require_import(import_name);
        }
        if kind == "call"
            && let Some(name) = op.s_value.as_ref()
            && !defined_function_names.contains(name.as_str())
        {
            let import_name = runtime_import_name_str(name);
            if name.starts_with("molt_") && !known_imports.contains(import_name) {
                panic!("direct runtime call missing WASM ABI manifest import: {name}");
            }
            if known_imports.contains(import_name) {
                self.require_import(import_name);
            }
        }
        if kind == "call_async"
            && let Some(name) = op.s_value.as_ref()
        {
            let import_name = runtime_import_name_str(name);
            if known_imports.contains(import_name) {
                self.require_import(import_name);
            }
        }

        if let Some(task_kind) = op.task_kind.as_deref() {
            if debug_imports_enabled() {
                eprintln!(
                    "WASM_IMPORTS task_meta kind={} task_kind={} args={} func={}",
                    kind,
                    task_kind,
                    op.args.as_ref().map(|args| args.len()).unwrap_or(0),
                    func_name
                );
            }
            self.require_import("task_new");
            if op.args.as_ref().is_some_and(|args| !args.is_empty()) {
                self.require_import("handle_resolve");
                self.require_import("inc_ref_obj");
            }
            if matches!(task_kind, "future" | "coroutine") {
                self.require_import("cancel_token_get_current");
                self.require_import("task_register_token_owned");
            }
        }

        if let Some(import_name) =
            selected_container_runtime_import(scalar_plan, op_index, kind, op)
        {
            self.require_import(import_name);
        }

        if runtime_surface_requires_direct_import(kind) {
            self.require_import(kind);
        }
        if is_poll
            && (kind == "call_func" || kind == "invoke_ffi")
            && let Some(name) = op.s_value.as_ref()
            && name.ends_with("_poll")
        {
            self.require_import(runtime_import_name_str(name));
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
        if !task_kinds.is_empty() {
            required.insert("task_new".to_string());
        }
        if task_kinds.values().any(|kind| {
            matches!(
                kind,
                TrampolineKind::Generator | TrampolineKind::Coroutine | TrampolineKind::AsyncGen
            )
        }) {
            required.insert("handle_resolve".to_string());
            required.insert("inc_ref_obj".to_string());
        }
        if task_kinds
            .values()
            .any(|kind| matches!(kind, TrampolineKind::Coroutine))
        {
            required.insert("cancel_token_get_current".to_string());
            required.insert("task_register_token_owned".to_string());
        }
        if task_kinds
            .values()
            .any(|kind| matches!(kind, TrampolineKind::AsyncGen))
        {
            required.insert("asyncgen_new".to_string());
        }
        required.extend(
            RESERVED_RUNTIME_CALLABLE_SPECS
                .iter()
                .map(|spec| spec.import_name.to_string()),
        );
        for plan in lir_lowering_plans.values() {
            if let Some(output) = plan.lir_fast_body() {
                required.extend(output.runtime_imports().map(str::to_string));
            }
        }
    }

    fn require_import(&mut self, name: &str) {
        if let Some(required) = self.auto_required_imports.as_mut() {
            required.insert(name.to_string());
        }
    }

    fn require_imports(&mut self, names: &[&str]) {
        for &name in names {
            self.require_import(name);
        }
    }
}

fn initial_auto_required_imports(
    options: &WasmCompileOptions,
    deps_map: &RuntimeImportDepsMap,
) -> Option<BTreeSet<String>> {
    if options.wasm_profile != WasmProfile::Auto || !options.reloc_enabled {
        return None;
    }

    let mut required = BTreeSet::new();
    if let Some(structural) = deps_map.get("__structural__") {
        required.extend(structural.iter().map(|name| (*name).to_string()));
    }
    if let Ok(extra_required) = std::env::var("MOLT_WASM_EXTRA_REQUIRED_IMPORTS") {
        required.extend(
            extra_required
                .split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string),
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

pub(super) fn runtime_import_name_str(runtime_name: &str) -> &str {
    runtime_name.strip_prefix("molt_").unwrap_or(runtime_name)
}
