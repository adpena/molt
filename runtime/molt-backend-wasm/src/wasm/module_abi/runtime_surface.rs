use std::collections::{BTreeMap, BTreeSet};

use super::super::class_def_layout::ClassDefLayout;
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm_abi::RESERVED_RUNTIME_CALLABLE_SPECS;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_imports::{
    IMPORT_REGISTRY, OP_IMPORT_DEPS, runtime_surface_requires_direct_import,
};
use crate::wasm_options::{WasmCompileOptions, WasmProfile};
use crate::wasm_plan::{gpu_runtime_call_symbol, wasm_specialized_container_import};
use crate::{FunctionIR, OpIR, SimpleIR, TrampolineKind};

pub(super) struct WasmRuntimeSurfacePlan {
    pub(super) auto_required_imports: Option<BTreeSet<String>>,
    pub(super) max_func_arity: usize,
    pub(super) max_call_arity: usize,
    pub(super) max_class_def_words: usize,
    pub(super) builtin_trampoline_specs: BTreeMap<String, usize>,
    pub(super) direct_import_call_specs: BTreeMap<String, usize>,
    pub(super) manifest_intrinsic_names: BTreeSet<String>,
}

impl WasmRuntimeSurfacePlan {
    pub(super) fn build(
        ir: &SimpleIR,
        lir_fast_outputs: &BTreeMap<String, crate::wasm_lir_fast_output::WasmFunctionOutput>,
        task_kinds: &BTreeMap<String, TrampolineKind>,
        options: &WasmCompileOptions,
    ) -> Self {
        let defined_function_names: BTreeSet<&str> =
            ir.functions.iter().map(|func| func.name.as_str()).collect();
        let known_imports: BTreeSet<&str> = IMPORT_REGISTRY.iter().map(|&(name, _)| name).collect();
        let deps_map: BTreeMap<&str, &[&str]> = OP_IMPORT_DEPS
            .iter()
            .map(|&(kind, deps)| (kind, deps))
            .collect();
        let mut plan = Self {
            auto_required_imports: initial_auto_required_imports(options, &deps_map),
            max_func_arity: 0,
            max_call_arity: 0,
            max_class_def_words: 0,
            builtin_trampoline_specs: BTreeMap::new(),
            direct_import_call_specs: BTreeMap::new(),
            manifest_intrinsic_names: BTreeSet::new(),
        };

        for func_ir in &ir.functions {
            plan.observe_function(func_ir, &defined_function_names, &known_imports, &deps_map);
        }
        plan.finish_auto_required_imports(lir_fast_outputs, task_kinds);
        plan
    }

    pub(super) fn auto_import_names(&self, import_ids: &TrackedImportIds) -> Vec<(String, usize)> {
        let mut auto_import_names: Vec<(String, usize)> = self
            .builtin_trampoline_specs
            .iter()
            .map(|(runtime_name, arity)| (runtime_import_name(runtime_name), *arity))
            .filter(|(import_name, _)| !import_ids.contains_key(import_name))
            .collect();
        auto_import_names.extend(
            self.direct_import_call_specs
                .iter()
                .map(|(runtime_name, arity)| (runtime_import_name(runtime_name), *arity))
                .filter(|(import_name, _)| !import_ids.contains_key(import_name)),
        );
        for spec in RESERVED_RUNTIME_CALLABLE_SPECS {
            if !import_ids.contains_key(spec.import_name) {
                auto_import_names.push((spec.import_name.to_string(), spec.arity));
            }
        }
        auto_import_names.sort_by(|a, b| a.0.cmp(&b.0));
        auto_import_names.dedup_by(|a, b| a.0 == b.0);
        auto_import_names
    }

    fn observe_function(
        &mut self,
        func_ir: &FunctionIR,
        defined_function_names: &BTreeSet<&str>,
        known_imports: &BTreeSet<&str>,
        deps_map: &BTreeMap<&str, &[&str]>,
    ) {
        let is_poll = func_ir.name.ends_with("_poll");
        let scalar_plan = ScalarRepresentationPlan::for_function_ir(func_ir);
        let const_strings: BTreeMap<&str, &str> = func_ir
            .ops
            .iter()
            .filter_map(|op| {
                if op.kind == "const_str" {
                    Some((op.out.as_deref()?, op.s_value.as_deref()?))
                } else {
                    None
                }
            })
            .collect();
        let runtime_lookup_vars: BTreeSet<&str> = func_ir
            .ops
            .iter()
            .filter_map(|op| {
                if op.kind == "builtin_func"
                    && matches!(
                        op.s_value.as_deref(),
                        Some("molt_require_intrinsic_runtime")
                            | Some("molt_load_intrinsic_runtime")
                    )
                {
                    op.out.as_deref()
                } else {
                    None
                }
            })
            .collect();

        if !is_poll {
            self.max_func_arity = self.max_func_arity.max(func_ir.params.len());
        }
        for (op_index, op) in func_ir.ops.iter().enumerate() {
            self.observe_op(
                func_ir.name.as_str(),
                op,
                op_index,
                is_poll,
                &scalar_plan,
                &const_strings,
                &runtime_lookup_vars,
                defined_function_names,
                known_imports,
                deps_map,
            );
        }
    }

    fn observe_op(
        &mut self,
        func_name: &str,
        op: &OpIR,
        op_index: usize,
        is_poll: bool,
        scalar_plan: &ScalarRepresentationPlan,
        const_strings: &BTreeMap<&str, &str>,
        runtime_lookup_vars: &BTreeSet<&str>,
        defined_function_names: &BTreeSet<&str>,
        known_imports: &BTreeSet<&str>,
        deps_map: &BTreeMap<&str, &[&str]>,
    ) {
        let kind = op.kind.as_str();
        self.observe_required_imports(
            func_name,
            op,
            op_index,
            kind,
            is_poll,
            scalar_plan,
            defined_function_names,
            known_imports,
            deps_map,
        );
        if !is_poll
            && (kind == "call_func" || kind == "invoke_ffi")
            && let Some(args) = &op.args
            && !args.is_empty()
        {
            self.max_call_arity = self.max_call_arity.max(args.len() - 1);
        }
        if kind == "class_def"
            && let Some(meta) = op.s_value.as_deref()
        {
            self.max_class_def_words = self
                .max_class_def_words
                .max(ClassDefLayout::parse(meta).spill_words());
        }
        if kind == "builtin_func"
            && let Some(name) = op.s_value.as_ref()
        {
            self.record_arity(
                name,
                op.value.unwrap_or(0) as usize,
                RuntimeArityPlan::BuiltinTrampoline,
            );
        }
        if kind == "call"
            && let Some(target_name) = op.s_value.as_ref()
            && !defined_function_names.contains(target_name.as_str())
        {
            let import_name = runtime_import_name_str(target_name);
            let is_runtime_import_target =
                target_name.starts_with("molt_") || known_imports.contains(import_name);
            if is_runtime_import_target {
                self.record_arity(
                    target_name,
                    op.args.as_ref().map_or(0, Vec::len),
                    RuntimeArityPlan::DirectImportCall,
                );
            }
        }
        if let Some(runtime_name) = gpu_runtime_call_symbol(kind) {
            self.direct_import_call_specs
                .entry(runtime_name.to_string())
                .or_insert(0);
        }
        if kind == "call_func"
            && let Some(args) = op.args.as_ref()
            && args.len() >= 3
            && runtime_lookup_vars.contains(args[0].as_str())
            && let Some(name) = const_strings.get(args[1].as_str())
        {
            self.manifest_intrinsic_names.insert((*name).to_string());
        }
    }

    fn observe_required_imports(
        &mut self,
        func_name: &str,
        op: &OpIR,
        op_index: usize,
        kind: &str,
        is_poll: bool,
        scalar_plan: &ScalarRepresentationPlan,
        defined_function_names: &BTreeSet<&str>,
        known_imports: &BTreeSet<&str>,
        deps_map: &BTreeMap<&str, &[&str]>,
    ) {
        if matches!(
            std::env::var("MOLT_DEBUG_WASM_IMPORTS").ok().as_deref(),
            Some("1")
        ) && op.s_value.as_deref() == Some("__main_____f_poll")
        {
            eprintln!(
                "WASM_IMPORTS saw_op kind={} s_value={:?} task_kind={:?} args={:?} func={}",
                kind, op.s_value, op.task_kind, op.args, func_name
            );
        }

        if kind == "object_new_bound" {
            let import_name = if op.value.is_some_and(|size| size > 0) {
                "object_new_bound_sized"
            } else {
                "object_new_bound"
            };
            self.require_import(import_name);
            return;
        }

        if let Some(deps) = deps_map.get(kind) {
            if matches!(
                std::env::var("MOLT_DEBUG_WASM_IMPORTS").ok().as_deref(),
                Some("1")
            ) && kind == "alloc_task"
            {
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
            self.require_import(runtime_import_name_str(name));
        }
        if kind == "call"
            && let Some(name) = op.s_value.as_ref()
            && !defined_function_names.contains(name.as_str())
        {
            let import_name = runtime_import_name_str(name);
            if name.starts_with("molt_") || known_imports.contains(import_name) {
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
            if matches!(
                std::env::var("MOLT_DEBUG_WASM_IMPORTS").ok().as_deref(),
                Some("1")
            ) {
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
            wasm_specialized_container_import(scalar_plan, op_index, kind, op)
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
        if let Some(runtime_name) = gpu_runtime_call_symbol(kind) {
            self.require_import(runtime_import_name_str(runtime_name));
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

    fn finish_auto_required_imports(
        &mut self,
        lir_fast_outputs: &BTreeMap<String, crate::wasm_lir_fast_output::WasmFunctionOutput>,
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
        for output in lir_fast_outputs.values() {
            required.extend(output.runtime_calls.iter().map(|name| name.to_string()));
        }
    }

    fn record_arity(&mut self, name: &str, arity: usize, plan: RuntimeArityPlan) {
        let specs = match plan {
            RuntimeArityPlan::BuiltinTrampoline => &mut self.builtin_trampoline_specs,
            RuntimeArityPlan::DirectImportCall => &mut self.direct_import_call_specs,
        };
        if let Some(prev) = specs.get(name) {
            if *prev != arity {
                panic!(
                    "{} arity mismatch for {name}: {prev} vs {arity}",
                    plan.diagnostic_name()
                );
            }
        } else {
            specs.insert(name.to_string(), arity);
        }
    }
}

fn initial_auto_required_imports(
    options: &WasmCompileOptions,
    deps_map: &BTreeMap<&str, &[&str]>,
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

#[derive(Clone, Copy)]
enum RuntimeArityPlan {
    BuiltinTrampoline,
    DirectImportCall,
}

impl RuntimeArityPlan {
    fn diagnostic_name(self) -> &'static str {
        match self {
            Self::BuiltinTrampoline => "builtin trampoline",
            Self::DirectImportCall => "direct imported call",
        }
    }
}

fn runtime_import_name_str(runtime_name: &str) -> &str {
    runtime_name.strip_prefix("molt_").unwrap_or(runtime_name)
}

fn runtime_import_name(runtime_name: &str) -> String {
    runtime_import_name_str(runtime_name).to_string()
}
