use std::collections::{BTreeMap, BTreeSet};

use super::super::class_def_layout::ClassDefLayout;
use crate::wasm_abi::{
    IMPORT_REGISTRY, RESERVED_RUNTIME_CALLABLE_SPECS, WasmRuntimeImport, runtime_callable_arity,
    runtime_callable_import, wasm_runtime_import,
};
use crate::wasm_abi_generated::{
    PYTHON_BUILTIN_CALLABLES, op_loop_runtime_call, python_builtin_callable,
};
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_options::WasmProfile;
use crate::{FunctionIR, OpIR, SimpleIR};
use molt_ir::tir::simple_def_use::visit_simple_ir_defined_names;

pub(super) struct WasmRuntimeSurfacePlan {
    pub(super) max_func_arity: usize,
    pub(super) max_call_arity: usize,
    pub(super) max_class_def_words: usize,
    pub(super) builtin_trampoline_specs: BTreeMap<String, usize>,
    pub(super) direct_import_call_specs: BTreeMap<String, usize>,
    pub(super) manifest_intrinsic_names: BTreeSet<String>,
    pub(super) required_imports: BTreeSet<WasmRuntimeImport>,
}

impl WasmRuntimeSurfacePlan {
    pub(super) fn build(ir: &SimpleIR) -> Self {
        let defined_function_names: BTreeSet<&str> =
            ir.functions.iter().map(|func| func.name.as_str()).collect();
        let known_imports: BTreeSet<WasmRuntimeImport> =
            IMPORT_REGISTRY.iter().map(|spec| spec.import).collect();
        let mut plan = Self {
            max_func_arity: 0,
            max_call_arity: 0,
            max_class_def_words: 0,
            builtin_trampoline_specs: BTreeMap::new(),
            direct_import_call_specs: BTreeMap::new(),
            manifest_intrinsic_names: BTreeSet::new(),
            required_imports: BTreeSet::new(),
        };

        for func_ir in &ir.functions {
            plan.observe_function(func_ir, &defined_function_names, &known_imports);
        }
        plan
    }

    pub(super) fn auto_imports(&self, import_ids: &TrackedImportIds) -> Vec<WasmRuntimeImport> {
        let mut auto_imports: Vec<WasmRuntimeImport> = self.reachable_imports().collect();
        auto_imports.extend(
            RESERVED_RUNTIME_CALLABLE_SPECS
                .iter()
                .map(|spec| spec.import),
        );
        auto_imports.retain(|&import| !import_ids.contains_key(import));
        auto_imports.sort_by_key(|import| import.name());
        auto_imports.dedup();
        auto_imports
    }

    pub(super) fn validate_profile(&self, profile: WasmProfile) {
        if profile == WasmProfile::Pure {
            for import in self.reachable_imports() {
                assert!(
                    !crate::wasm_abi_generated::pure_profile_skips_import(import.name()),
                    "WASM pure profile cannot admit reachable runtime import '{}'; select a profile supporting this callable closure",
                    import.name()
                );
            }
        }
    }

    fn reachable_imports(&self) -> impl Iterator<Item = WasmRuntimeImport> + '_ {
        self.required_imports
            .iter()
            .copied()
            .chain(self.builtin_trampoline_specs.keys().map(|runtime_name| {
                runtime_callable_import(runtime_name).unwrap_or_else(|| {
                    panic!("runtime callable missing generated import spec: {runtime_name}")
                })
            }))
            .chain(self.direct_import_call_specs.keys().map(|runtime_name| {
                wasm_runtime_import(runtime_name).unwrap_or_else(|| {
                    panic!("direct runtime call missing generated import spec: {runtime_name}")
                })
            }))
            .chain(self.manifest_intrinsic_names.iter().map(|runtime_name| {
                runtime_callable_import(runtime_name).unwrap_or_else(|| {
                    panic!(
                        "intrinsic manifest symbol missing generated WASM import: {runtime_name}"
                    )
                })
            }))
    }

    fn observe_function(
        &mut self,
        func_ir: &FunctionIR,
        defined_function_names: &BTreeSet<&str>,
        known_imports: &BTreeSet<WasmRuntimeImport>,
    ) {
        if func_ir.is_extern {
            let signature = func_ir.extern_signature().unwrap_or_else(|error| {
                panic!("invalid WASM extern function declaration: {error}")
            });
            self.max_func_arity = self.max_func_arity.max(signature.arity);
            return;
        }
        let is_poll = func_ir.name.ends_with("_poll");
        // SimpleIR is mutable transport, not SSA. A whole-function constant
        // fact is valid only when no parameter or other definition can replace
        // it. Unknown builtin names conservatively retain the generated family.
        let mut definitions = BTreeMap::<&str, usize>::new();
        for name in &func_ir.params {
            *definitions.entry(name.as_str()).or_default() += 1;
        }
        for op in &func_ir.ops {
            visit_simple_ir_defined_names(op, |name| {
                *definitions.entry(name).or_default() += 1;
            });
        }
        let const_strings: BTreeMap<&str, &str> = func_ir
            .ops
            .iter()
            .filter_map(|op| {
                if op.kind == "const_str" && definitions.get(op.out.as_deref()?) == Some(&1) {
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
                        Some("molt_require_intrinsic_runtime" | "molt_load_intrinsic_runtime")
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
        for op in &func_ir.ops {
            self.observe_op(
                op,
                is_poll,
                &const_strings,
                &runtime_lookup_vars,
                defined_function_names,
                known_imports,
            );
        }
    }

    fn observe_op(
        &mut self,
        op: &OpIR,
        is_poll: bool,
        const_strings: &BTreeMap<&str, &str>,
        runtime_lookup_vars: &BTreeSet<&str>,
        defined_function_names: &BTreeSet<&str>,
        known_imports: &BTreeSet<WasmRuntimeImport>,
    ) {
        let kind = op.kind.as_str();
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
        if let Some(call) = op_loop_runtime_call(kind, op.is_async_work_poll()) {
            self.required_imports
                .extend(call.required_imports.iter().copied());
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
            let import = wasm_runtime_import(target_name);
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
        // Runtime lookup is a reachability edge, not a direct-call rewrite.
        // The generated callable root must survive even when a mutable global
        // has no explicit builtin_func producer in this application.
        let args = op.args.as_deref().unwrap_or_default();
        let runtime_call = match kind {
            "module_get_global" => Some((WasmRuntimeImport::ModuleGetGlobal, args)),
            "call"
                if !op
                    .s_value
                    .as_deref()
                    .is_some_and(|name| defined_function_names.contains(name)) =>
            {
                op.s_value
                    .as_deref()
                    .and_then(wasm_runtime_import)
                    .map(|import| (import, args))
            }
            "call_func" => args.split_first().and_then(|(callee, args)| {
                // Both helper identities have the same name operand. Retain
                // any observed helper definition, rather than last-write-wins.
                runtime_lookup_vars
                    .contains(callee.as_str())
                    .then_some((WasmRuntimeImport::RequireIntrinsicRuntime, args))
            }),
            _ => None,
        };
        match runtime_call {
            Some((WasmRuntimeImport::ModuleGetGlobal, [_, name])) => {
                self.record_builtin_lookup(const_strings.get(name.as_str()).copied());
            }
            Some((
                WasmRuntimeImport::RequireIntrinsicRuntime
                | WasmRuntimeImport::LoadIntrinsicRuntime,
                [name, ..],
            )) => {
                if let Some(name) = const_strings.get(name.as_str()) {
                    self.manifest_intrinsic_names.insert((*name).to_string());
                }
            }
            _ => {}
        }
    }

    fn record_builtin_lookup(&mut self, name: Option<&str>) {
        if let Some(name) = name {
            if let Some(spec) = python_builtin_callable(name) {
                self.record_arity(
                    spec.runtime_name,
                    spec.arity,
                    RuntimeArityPlan::BuiltinTrampoline,
                );
            }
        } else {
            // A computed name can select any supported Python builtin. Only
            // this genuinely dynamic case needs the complete generated family;
            // ordinary source-level global names retain exact tree shaking.
            for spec in PYTHON_BUILTIN_CALLABLES {
                self.record_arity(
                    spec.runtime_name,
                    spec.arity,
                    RuntimeArityPlan::BuiltinTrampoline,
                );
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wasm_import_tracking::TrackedImportIds;

    #[test]
    fn overwritten_lookup_name_does_not_keep_a_stale_constant_root() {
        for mutator in [
            OpIR {
                kind: "store_var".into(),
                var: Some("name".into()),
                args: Some(vec!["computed".into()]),
                ..Default::default()
            },
            OpIR {
                kind: "const_str".into(),
                out: Some("name".into()),
                s_value: Some("application_global".into()),
                ..Default::default()
            },
        ] {
            let ir = SimpleIR {
                functions: vec![FunctionIR {
                    name: "lookup".into(),
                    params: vec!["module".into(), "computed".into()],
                    ops: vec![
                        OpIR {
                            kind: "const_str".into(),
                            out: Some("name".into()),
                            s_value: Some("print".into()),
                            ..Default::default()
                        },
                        mutator,
                        OpIR {
                            kind: "module_get_global".into(),
                            args: Some(vec!["module".into(), "name".into()]),
                            out: Some("value".into()),
                            ..Default::default()
                        },
                    ],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                    codegen_partition: false,
                    execution_context: Default::default(),
                }],
                profile: None,
            };
            let plan = WasmRuntimeSurfacePlan::build(&ir);
            for spec in PYTHON_BUILTIN_CALLABLES {
                assert_eq!(
                    plan.builtin_trampoline_specs.get(spec.runtime_name),
                    Some(&spec.arity)
                );
            }
        }
    }

    #[test]
    fn auto_imports_root_the_complete_reserved_runtime_callable_family() {
        let plan = WasmRuntimeSurfacePlan {
            max_func_arity: 0,
            max_call_arity: 0,
            max_class_def_words: 0,
            builtin_trampoline_specs: BTreeMap::new(),
            direct_import_call_specs: BTreeMap::new(),
            manifest_intrinsic_names: BTreeSet::new(),
            required_imports: BTreeSet::new(),
        };
        let import_ids = TrackedImportIds::new(BTreeMap::new());
        let imports = plan.auto_imports(&import_ids);

        let expected = RESERVED_RUNTIME_CALLABLE_SPECS
            .iter()
            .map(|spec| spec.import)
            .collect::<BTreeSet<_>>();
        assert_eq!(imports.into_iter().collect::<BTreeSet<_>>(), expected);
    }
}
