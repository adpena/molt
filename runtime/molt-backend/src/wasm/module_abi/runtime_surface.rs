use super::*;

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
        lir_fast_outputs: &BTreeMap<String, crate::tir::lower_to_wasm::WasmFunctionOutput>,
        task_kinds: &BTreeMap<String, TrampolineKind>,
        import_ids: &TrackedImportIds,
        options: &WasmCompileOptions,
    ) -> Self {
        let auto_required_imports =
            plan_auto_required_imports(ir, lir_fast_outputs, task_kinds, options);
        let defined_function_names: BTreeSet<&str> =
            ir.functions.iter().map(|func| func.name.as_str()).collect();
        let mut plan = Self {
            auto_required_imports,
            max_func_arity: 0,
            max_call_arity: 0,
            max_class_def_words: 0,
            builtin_trampoline_specs: BTreeMap::new(),
            direct_import_call_specs: BTreeMap::new(),
            manifest_intrinsic_names: BTreeSet::new(),
        };

        for func_ir in &ir.functions {
            plan.observe_function(func_ir, &defined_function_names, import_ids);
        }
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
        import_ids: &TrackedImportIds,
    ) {
        let is_poll = func_ir.name.ends_with("_poll");
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
        for op in &func_ir.ops {
            self.observe_op(
                op,
                is_poll,
                &const_strings,
                &runtime_lookup_vars,
                defined_function_names,
                import_ids,
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
        import_ids: &TrackedImportIds,
    ) {
        if !is_poll
            && (op.kind == "call_func" || op.kind == "invoke_ffi")
            && let Some(args) = &op.args
            && !args.is_empty()
        {
            self.max_call_arity = self.max_call_arity.max(args.len() - 1);
        }
        if op.kind == "class_def"
            && let Some(meta) = op.s_value.as_deref()
        {
            self.max_class_def_words = self.max_class_def_words.max(class_def_spill_words(meta));
        }
        if op.kind == "builtin_func"
            && let Some(name) = op.s_value.as_ref()
        {
            self.record_arity(
                name,
                op.value.unwrap_or(0) as usize,
                RuntimeArityPlan::BuiltinTrampoline,
            );
        }
        if op.kind == "call"
            && let Some(target_name) = op.s_value.as_ref()
            && !defined_function_names.contains(target_name.as_str())
        {
            let import_name = target_name
                .strip_prefix("molt_")
                .unwrap_or(target_name.as_str());
            let is_runtime_import_target =
                target_name.starts_with("molt_") || import_ids.contains_key(import_name);
            if is_runtime_import_target {
                self.record_arity(
                    target_name,
                    op.args.as_ref().map_or(0, Vec::len),
                    RuntimeArityPlan::DirectImportCall,
                );
            }
        }
        if let Some(runtime_name) = gpu_runtime_call_symbol(op.kind.as_str()) {
            self.direct_import_call_specs
                .entry(runtime_name.to_string())
                .or_insert(0);
        }
        if op.kind == "call_func"
            && let Some(args) = op.args.as_ref()
            && args.len() >= 3
            && runtime_lookup_vars.contains(args[0].as_str())
            && let Some(name) = const_strings.get(args[1].as_str())
        {
            self.manifest_intrinsic_names.insert((*name).to_string());
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

fn plan_auto_required_imports(
    ir: &SimpleIR,
    lir_fast_outputs: &BTreeMap<String, crate::tir::lower_to_wasm::WasmFunctionOutput>,
    task_kinds: &BTreeMap<String, TrampolineKind>,
    options: &WasmCompileOptions,
) -> Option<BTreeSet<String>> {
    if options.wasm_profile != WasmProfile::Auto || !options.reloc_enabled {
        return None;
    }
    let mut required = collect_reloc_required_imports(ir);
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
    Some(required)
}

fn class_def_spill_words(meta: &str) -> usize {
    let mut parts = meta.split(',');
    let nbases = parts
        .next()
        .and_then(|s| s.parse::<usize>().ok())
        .expect("class_def metadata missing base count");
    let nattrs = parts
        .next()
        .and_then(|s| s.parse::<usize>().ok())
        .expect("class_def metadata missing attr count");
    nbases.max(1) + (nattrs * 2).max(1)
}

fn runtime_import_name(runtime_name: &str) -> String {
    runtime_name
        .strip_prefix("molt_")
        .unwrap_or(runtime_name)
        .to_string()
}
