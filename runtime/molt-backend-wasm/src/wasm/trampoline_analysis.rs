use crate::{SimpleIR, TrampolineKind, TrampolineTaskKind};
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct WasmTrampolineAnalysis {
    pub(super) escaped_callable_targets: BTreeSet<String>,
    pub(super) task_kinds: BTreeMap<String, TrampolineKind>,
    pub(super) task_closure_sizes: BTreeMap<String, i64>,
    pub(super) default_trampoline_spec: BTreeMap<String, (usize, bool)>,
    /// Whether a direct WASM call to each user function leaves one boxed-i64
    /// result on the operand stack. Defined functions always do (ret_void
    /// materializes None); extern declarations follow their canonical
    /// FunctionIR signature.
    pub(super) function_abi_returns_value: BTreeMap<String, bool>,
}

pub(super) fn analyze_wasm_trampolines(ir: &SimpleIR) -> WasmTrampolineAnalysis {
    // DETERMINISM: BTreeMap ensures iteration order is independent of hash seed
    let mut func_trampoline_spec: BTreeMap<String, (usize, bool)> = BTreeMap::new();
    let mut escaped_callable_targets: BTreeSet<String> = BTreeSet::new();
    let mut task_kinds: BTreeMap<String, TrampolineKind> = BTreeMap::new();
    let mut task_closure_sizes: BTreeMap<String, i64> = BTreeMap::new();
    for func_ir in &ir.functions {
        if func_ir.is_extern {
            continue;
        }
        let mut func_obj_names: BTreeMap<String, String> = BTreeMap::new();
        let mut const_values: BTreeMap<String, i64> = BTreeMap::new();
        let mut const_bools: BTreeMap<String, bool> = BTreeMap::new();
        let mut pending_attrs: Vec<(String, String, String)> = Vec::new();
        for op in &func_ir.ops {
            match op.kind.as_str() {
                "const" => {
                    let Some(out) = op.out.as_ref() else {
                        continue;
                    };
                    let val = op.value.unwrap_or(0);
                    const_values.insert(out.clone(), val);
                }
                "const_bool" => {
                    let Some(out) = op.out.as_ref() else {
                        continue;
                    };
                    let val = op.value.unwrap_or(0) != 0;
                    const_bools.insert(out.clone(), val);
                }
                "func_new" | "func_new_closure" => {
                    let Some(name) = op.s_value.as_ref() else {
                        continue;
                    };
                    let arity = op.value.unwrap_or(0) as usize;
                    let has_closure = op.kind == "func_new_closure";
                    escaped_callable_targets.insert(name.clone());
                    if let Some(out) = op.out.as_ref() {
                        func_obj_names.insert(out.clone(), name.clone());
                    }
                    if let Some((prev_arity, prev_closure)) = func_trampoline_spec.get(name) {
                        if *prev_arity != arity || *prev_closure != has_closure {
                            panic!("func_new arity mismatch for {name}");
                        }
                    } else {
                        func_trampoline_spec.insert(name.clone(), (arity, has_closure));
                    }
                }
                "builtin_func" => {
                    let Some(name) = op.s_value.as_ref() else {
                        continue;
                    };
                    escaped_callable_targets.insert(name.clone());
                }
                "set_attr_generic_obj" => {
                    let Some(attr) = op.s_value.as_deref() else {
                        continue;
                    };
                    if attr != "__molt_is_generator__"
                        && attr != "__molt_is_coroutine__"
                        && attr != "__molt_is_async_generator__"
                        && attr != "__molt_closure_size__"
                    {
                        continue;
                    }
                    let args = op.args.as_ref().expect("set_attr_generic_obj args missing");
                    pending_attrs.push((args[0].clone(), args[1].clone(), attr.to_string()));
                }
                _ => {}
            }
        }
        for (func_obj_name, val_name, attr) in pending_attrs {
            let Some(func_name) = func_obj_names.get(&func_obj_name) else {
                continue;
            };
            match attr.as_str() {
                "__molt_is_generator__"
                | "__molt_is_coroutine__"
                | "__molt_is_async_generator__" => {
                    let is_true = const_bools
                        .get(&val_name)
                        .copied()
                        .or_else(|| const_values.get(&val_name).map(|val| *val != 0))
                        .unwrap_or(false);
                    if is_true {
                        if !func_name.ends_with("_poll") {
                            continue;
                        }
                        let kind = TrampolineTaskKind::from_marker_attr(attr.as_str())
                            .expect("task marker was filtered above")
                            .trampoline_kind();
                        if let Some(prev) = task_kinds.insert(func_name.clone(), kind)
                            && prev != kind
                        {
                            panic!(
                                "conflicting task kinds for {func_name}: {:?} vs {:?}",
                                prev, kind
                            );
                        }
                    }
                }
                "__molt_closure_size__" => {
                    if let Some(size) = const_values.get(&val_name) {
                        task_closure_sizes.insert(func_name.clone(), *size);
                    }
                }
                _ => {}
            }
        }
    }
    // DETERMINISM: BTreeMap ensures iteration order is independent of hash seed
    let mut default_trampoline_spec: BTreeMap<String, (usize, bool)> = BTreeMap::new();
    let mut function_abi_returns_value: BTreeMap<String, bool> = BTreeMap::new();
    for func_ir in &ir.functions {
        let default_has_closure = func_ir
            .params
            .first()
            .is_some_and(|name| name == crate::MOLT_CLOSURE_PARAM_NAME);
        let mut default_arity = func_ir.params.len();
        if default_has_closure && default_arity > 0 {
            default_arity = default_arity.saturating_sub(1);
        }
        let spec = func_trampoline_spec
            .get(&func_ir.name)
            .copied()
            .unwrap_or((default_arity, default_has_closure));
        default_trampoline_spec.insert(func_ir.name.clone(), spec);
        let abi_returns_value = if func_ir.is_extern {
            func_ir
                .extern_signature()
                .unwrap_or_else(|error| panic!("invalid WASM extern function declaration: {error}"))
                .returns_value
        } else {
            true
        };
        function_abi_returns_value.insert(func_ir.name.clone(), abi_returns_value);
    }

    WasmTrampolineAnalysis {
        escaped_callable_targets,
        task_kinds,
        task_closure_sizes,
        default_trampoline_spec,
        function_abi_returns_value,
    }
}
