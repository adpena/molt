use std::collections::{BTreeMap, BTreeSet};

use super::super::class_def_layout::ClassDefLayout;
use super::runtime_import_demand::{WasmRuntimeImportDemand, runtime_import_name_str};
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm_abi::{runtime_callable_arity, runtime_callable_import};
use crate::wasm_abi_generated::{WasmRuntimeImport, wasm_runtime_import};
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_imports::IMPORT_REGISTRY;
use crate::wasm_options::WasmCompileOptions;
use crate::{FunctionIR, OpIR, SimpleIR, TrampolineKind};

pub(super) struct WasmRuntimeSurfacePlan {
    pub(super) import_demand: WasmRuntimeImportDemand,
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
        lir_lowering_plans: &crate::wasm::lir_fast::WasmFunctionLoweringPlans,
        task_kinds: &BTreeMap<String, TrampolineKind>,
        options: &WasmCompileOptions,
    ) -> Self {
        let defined_function_names: BTreeSet<&str> =
            ir.functions.iter().map(|func| func.name.as_str()).collect();
        let known_imports: BTreeSet<WasmRuntimeImport> =
            IMPORT_REGISTRY.iter().map(|spec| spec.import).collect();
        let mut plan = Self {
            import_demand: WasmRuntimeImportDemand::new(options),
            max_func_arity: 0,
            max_call_arity: 0,
            max_class_def_words: 0,
            builtin_trampoline_specs: BTreeMap::new(),
            direct_import_call_specs: BTreeMap::new(),
            manifest_intrinsic_names: BTreeSet::new(),
        };

        for func_ir in &ir.functions {
            plan.observe_function(func_ir, &defined_function_names, &known_imports);
        }
        plan.import_demand.finish(lir_lowering_plans, task_kinds);
        plan
    }

    pub(super) fn auto_imports(&self, import_ids: &TrackedImportIds) -> Vec<WasmRuntimeImport> {
        let mut auto_imports: Vec<WasmRuntimeImport> = self
            .builtin_trampoline_specs
            .iter()
            .map(|(runtime_name, _arity)| {
                runtime_callable_import(runtime_name).unwrap_or_else(|| {
                    panic!("runtime callable missing generated import spec: {runtime_name}")
                })
            })
            .filter(|&import| !import_ids.contains_key(import))
            .collect();
        auto_imports.sort_by_key(|import| import.name());
        auto_imports.dedup();
        auto_imports
    }

    fn observe_function(
        &mut self,
        func_ir: &FunctionIR,
        defined_function_names: &BTreeSet<&str>,
        known_imports: &BTreeSet<WasmRuntimeImport>,
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
        known_imports: &BTreeSet<WasmRuntimeImport>,
    ) {
        let kind = op.kind.as_str();
        self.import_demand.observe_op(
            func_name,
            op,
            op_index,
            is_poll,
            scalar_plan,
            defined_function_names,
            known_imports,
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
            let manifest_arity = runtime_callable_arity(name).unwrap_or_else(|| {
                panic!("builtin runtime callable missing from WASM ABI manifest: {name}")
            });
            if let Some(observed_arity) = op.value.map(|value| value as usize)
                && observed_arity != manifest_arity
            {
                panic!(
                    "builtin runtime callable arity mismatch for {name}: manifest {manifest_arity} vs observed {observed_arity}"
                );
            }
            self.record_arity(name, manifest_arity, RuntimeArityPlan::BuiltinTrampoline);
        }
        if kind == "call"
            && let Some(target_name) = op.s_value.as_ref()
            && !defined_function_names.contains(target_name.as_str())
        {
            let import_name = runtime_import_name_str(target_name);
            let import = wasm_runtime_import(import_name);
            if target_name.starts_with("molt_") && import.is_none() {
                panic!("direct runtime call missing WASM ABI manifest import: {target_name}");
            }
            if import.is_some_and(|import| known_imports.contains(&import)) {
                self.record_arity(
                    target_name,
                    op.args.as_ref().map_or(0, Vec::len),
                    RuntimeArityPlan::DirectImportCall,
                );
            }
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

    fn record_arity(&mut self, name: &str, arity: usize, plan: RuntimeArityPlan) {
        if let RuntimeArityPlan::BuiltinTrampoline = plan
            && let Some(manifest_arity) = runtime_callable_arity(name)
            && manifest_arity != arity
        {
            panic!(
                "{} arity mismatch for {name}: manifest {manifest_arity} vs observed {arity}",
                plan.diagnostic_name()
            );
        }
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
