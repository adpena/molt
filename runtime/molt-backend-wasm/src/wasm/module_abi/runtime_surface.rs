use std::collections::{BTreeMap, BTreeSet};

use super::super::class_def_layout::ClassDefLayout;
use crate::wasm_abi::{
    IMPORT_REGISTRY, RESERVED_RUNTIME_CALLABLE_SPECS, WasmRuntimeImport, runtime_callable_arity,
    runtime_callable_import, wasm_runtime_import,
};
use crate::wasm_abi_generated::op_loop_runtime_call;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_options::WasmProfile;
use crate::{FunctionIR, OpIR, SimpleIR};
use molt_tir::passes::collect_app_callable_requirements;

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
    pub(super) fn build(ir: &SimpleIR, profile: WasmProfile) -> Self {
        let defined_function_names: BTreeSet<&str> =
            ir.functions.iter().map(|func| func.name.as_str()).collect();
        let known_imports: BTreeSet<WasmRuntimeImport> =
            IMPORT_REGISTRY.iter().map(|spec| spec.import).collect();
        let requirements = collect_app_callable_requirements(&ir.functions);
        let mut builtin_trampoline_specs = requirements.builtin_trampolines;
        // Namespace construction publishes what this profile supports; it is
        // not a demand to execute every public builtin. Actual global lookup
        // and materialization roots remain mandatory and are never filtered.
        for (name, arity) in requirements.builtin_namespace_trampolines {
            if runtime_callable_import(&name)
                .is_some_and(|import| profile.allows_runtime_import(import))
            {
                builtin_trampoline_specs.entry(name).or_insert(arity);
            }
        }
        let mut plan = Self {
            max_func_arity: 0,
            max_call_arity: 0,
            max_class_def_words: 0,
            builtin_trampoline_specs,
            direct_import_call_specs: BTreeMap::new(),
            manifest_intrinsic_names: requirements
                .intrinsic_names
                .into_iter()
                // Resolver candidates do not demand a capability. Like native
                // staticlib admission, retain only providers this profile has;
                // an unavailable dynamic lookup must stay unavailable. Actual
                // calls/materializations below remain mandatory requirements.
                .filter(|name| {
                    runtime_callable_import(name)
                        .is_some_and(|import| profile.allows_runtime_import(import))
                })
                .collect(),
            required_imports: BTreeSet::new(),
        };

        for func_ir in &ir.functions {
            plan.observe_function(func_ir, &defined_function_names, &known_imports);
        }
        plan.validate_profile(profile);
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
                    profile.allows_runtime_import(import),
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
        self.max_func_arity = self.max_func_arity.max(func_ir.params.len());
        for op in &func_ir.ops {
            self.observe_op(op, defined_function_names, known_imports);
        }
    }

    fn observe_op(
        &mut self,
        op: &OpIR,
        defined_function_names: &BTreeSet<&str>,
        known_imports: &BTreeSet<WasmRuntimeImport>,
    ) {
        let kind = op.kind.as_str();
        if (kind == "call_func" || kind == "invoke_ffi")
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
    use molt_ir::python_builtin_callables_generated::PYTHON_BUILTIN_CALLABLES;

    fn builtin_publication_ir(extra_ops: Vec<OpIR>) -> SimpleIR {
        let mut ops = vec![
            OpIR {
                kind: "const_str".into(),
                out: Some("module_name".into()),
                s_value: Some("builtins".into()),
                ..Default::default()
            },
            OpIR {
                kind: "module_cache_set".into(),
                args: Some(vec!["module_name".into(), "module".into()]),
                ..Default::default()
            },
        ];
        ops.extend(extra_ops);
        SimpleIR {
            functions: vec![FunctionIR {
                name: "builtin_bootstrap".into(),
                params: vec!["module".into()],
                ops,
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                execution_context: Default::default(),
            }],
            profile: None,
        }
    }

    #[test]
    fn builtin_namespace_publication_admits_only_profile_supported_candidates() {
        let ir = builtin_publication_ir(vec![]);
        for profile in [WasmProfile::Pure, WasmProfile::Auto, WasmProfile::Full] {
            let plan = WasmRuntimeSurfacePlan::build(&ir, profile);
            for spec in PYTHON_BUILTIN_CALLABLES {
                let import = runtime_callable_import(spec.runtime_name).unwrap();
                assert_eq!(
                    plan.builtin_trampoline_specs.get(spec.runtime_name),
                    profile.allows_runtime_import(import).then_some(&spec.arity),
                    "{:?}: {}",
                    profile,
                    spec.python_name
                );
            }
        }
        let pure = WasmRuntimeSurfacePlan::build(&ir, WasmProfile::Pure);
        assert!(
            !pure
                .builtin_trampoline_specs
                .contains_key("molt_open_builtin")
        );
        assert!(pure.builtin_trampoline_specs.contains_key("molt_len"));
    }

    #[test]
    #[should_panic(
        expected = "WASM pure profile cannot admit reachable runtime import 'open_builtin'"
    )]
    fn builtin_namespace_publication_does_not_weaken_mandatory_global_lookup() {
        let ir = builtin_publication_ir(vec![
            OpIR {
                kind: "const_str".into(),
                out: Some("lookup_name".into()),
                s_value: Some("open".into()),
                ..Default::default()
            },
            OpIR {
                kind: "module_get_global".into(),
                args: Some(vec!["module".into(), "lookup_name".into()]),
                out: Some("value".into()),
                ..Default::default()
            },
        ]);
        WasmRuntimeSurfacePlan::build(&ir, WasmProfile::Pure);
    }

    #[test]
    #[should_panic(
        expected = "WASM pure profile cannot admit reachable runtime import 'open_builtin'"
    )]
    fn builtin_namespace_publication_does_not_weaken_mandatory_materialization() {
        let ir = builtin_publication_ir(vec![OpIR {
            kind: "builtin_func".into(),
            s_value: Some("molt_open_builtin".into()),
            value: Some(runtime_callable_arity("molt_open_builtin").unwrap() as i64),
            out: Some("value".into()),
            ..Default::default()
        }]);
        WasmRuntimeSurfacePlan::build(&ir, WasmProfile::Pure);
    }

    #[test]
    #[should_panic(
        expected = "WASM pure profile cannot admit reachable runtime import 'open_builtin'"
    )]
    fn builtin_namespace_publication_does_not_weaken_mandatory_direct_calls() {
        let ir = builtin_publication_ir(vec![OpIR {
            kind: "call".into(),
            s_value: Some("molt_open_builtin".into()),
            args: Some(vec![
                "module".into();
                runtime_callable_arity("molt_open_builtin").unwrap()
            ]),
            out: Some("value".into()),
            ..Default::default()
        }]);
        WasmRuntimeSurfacePlan::build(&ir, WasmProfile::Pure);
    }

    #[test]
    fn stored_intrinsic_names_share_resolver_and_import_reachability() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "sys_bootstrap".into(),
                params: vec![],
                ops: vec![
                    OpIR {
                        kind: "const_str".into(),
                        out: Some("name".into()),
                        s_value: Some("molt_sys_version".into()),
                        ..Default::default()
                    },
                    OpIR {
                        kind: "store_var".into(),
                        var: Some("later".into()),
                        args: Some(vec!["name".into()]),
                        ..Default::default()
                    },
                    OpIR {
                        kind: "const_str".into(),
                        out: Some("message".into()),
                        s_value: Some("molt_not_a_callable: diagnostic".into()),
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
        let plan = WasmRuntimeSurfacePlan::build(&ir, WasmProfile::Auto);
        let symbols = BTreeSet::from(["molt_sys_version".into()]);
        assert_eq!(
            plan.manifest_intrinsic_names,
            molt_tir::passes::compute_app_callable_manifest(&ir.functions, &symbols)
        );
        assert!(
            plan.reachable_imports()
                .any(|import| Some(import) == runtime_callable_import("molt_sys_version"))
        );
    }

    #[test]
    fn named_lookup_plan_roots_only_the_matching_generated_callable() {
        for spec in PYTHON_BUILTIN_CALLABLES {
            let plan = WasmRuntimeSurfacePlan::build(
                &SimpleIR {
                    functions: vec![FunctionIR {
                        name: "lookup".into(),
                        params: vec!["module".into()],
                        ops: vec![
                            OpIR {
                                kind: "const_str".into(),
                                out: Some("name".into()),
                                s_value: Some(spec.python_name.into()),
                                ..Default::default()
                            },
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
                },
                WasmProfile::Auto,
            );
            assert_eq!(
                plan.builtin_trampoline_specs,
                BTreeMap::from([(spec.runtime_name.to_string(), spec.arity)])
            );
        }
    }

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
            let plan = WasmRuntimeSurfacePlan::build(&ir, WasmProfile::Auto);
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
