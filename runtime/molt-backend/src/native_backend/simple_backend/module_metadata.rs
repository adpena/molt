use super::*;

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) struct NativeBackendIrAnalysis {
    pub(in crate::native_backend::simple_backend) defined_functions: BTreeSet<String>,
    pub(in crate::native_backend::simple_backend) closure_functions: BTreeSet<String>,
    pub(in crate::native_backend::simple_backend) task_kinds: BTreeMap<String, TrampolineKind>,
    pub(in crate::native_backend::simple_backend) task_closure_sizes: BTreeMap<String, i64>,
    /// Functions that contain no user-level calls (call, call_guarded,
    /// call_func, call_internal, call_indirect, call_bind, invoke_ffi).
    /// These can skip the recursion guard on direct calls.
    pub(in crate::native_backend::simple_backend) leaf_functions: BTreeSet<String>,
}

#[cfg(feature = "native-backend")]
#[derive(Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct NativeBackendModuleContext {
    pub(in crate::native_backend::simple_backend) function_arities: BTreeMap<String, usize>,
    pub(in crate::native_backend::simple_backend) function_has_ret: BTreeMap<String, bool>,
    pub(in crate::native_backend::simple_backend) closure_functions: BTreeSet<String>,
    pub(in crate::native_backend::simple_backend) task_kinds: BTreeMap<String, TrampolineKind>,
    pub(in crate::native_backend::simple_backend) task_closure_sizes: BTreeMap<String, i64>,
    pub(in crate::native_backend::simple_backend) leaf_functions: BTreeSet<String>,
    pub(in crate::native_backend::simple_backend) return_alias_summaries:
        BTreeMap<String, crate::passes::ReturnAliasSummary>,
}

#[cfg(feature = "native-backend")]
impl NativeBackendModuleContext {
    pub(in crate::native_backend::simple_backend) fn from_functions(
        functions: &[FunctionIR],
    ) -> Self {
        // The module context carries the whole-program leaf set consumed by the
        // batched codegen path, so compute it here (Tier-0 S4).
        let analysis = analyze_native_backend_functions(functions, /* compute_leaves */ true);
        Self {
            function_arities: functions
                .iter()
                .map(|func| (func.name.clone(), func.params.len()))
                .collect(),
            function_has_ret: compute_function_has_ret(functions),
            closure_functions: analysis.closure_functions,
            task_kinds: analysis.task_kinds,
            task_closure_sizes: analysis.task_closure_sizes,
            leaf_functions: analysis.leaf_functions,
            return_alias_summaries: crate::passes::compute_return_alias_summaries(functions),
        }
    }
}

/// Analyze the native backend's SimpleIR function set.
///
/// `compute_leaves` gates the whole-program TIR call-graph leaf-set computation
/// (Tier-0 S4): it is a relatively heavy whole-program lift, so the callers that
/// only need `task_kinds` / `task_closure_sizes` (the pre-megafunction-split
/// task-annotation capture) pass `false` and leave `leaf_functions` empty, while
/// the callers that actually consume the leaf set (the post-split analysis and
/// the module-context builder) pass `true`.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn analyze_native_backend_functions(
    functions: &[FunctionIR],
    compute_leaves: bool,
) -> NativeBackendIrAnalysis {
    let defined_functions: BTreeSet<String> = functions
        .iter()
        .filter(|func| !func.is_extern)
        .map(|func| func.name.clone())
        .collect();
    let mut closure_functions: BTreeSet<String> = BTreeSet::new();
    let mut task_kinds: BTreeMap<String, TrampolineKind> = BTreeMap::new();
    let mut task_closure_sizes: BTreeMap<String, i64> = BTreeMap::new();
    let mut has_task_attrs = false;

    for func_ir in functions {
        for op in &func_ir.ops {
            match op.kind.as_str() {
                "func_new_closure" => {
                    if let Some(name) = op.s_value.as_ref() {
                        closure_functions.insert(name.clone());
                    }
                }
                "set_attr_generic_obj" => {
                    if matches!(
                        op.s_value.as_deref(),
                        Some(
                            "__molt_is_generator__"
                                | "__molt_is_coroutine__"
                                | "__molt_is_async_generator__"
                                | "__molt_closure_size__"
                        )
                    ) {
                        has_task_attrs = true;
                    }
                }
                _ => {}
            }
        }
    }

    if has_task_attrs {
        for func_ir in functions {
            let mut func_obj_names: BTreeMap<String, String> = BTreeMap::new();
            let mut const_values: BTreeMap<String, i64> = BTreeMap::new();
            let mut const_bools: BTreeMap<String, bool> = BTreeMap::new();
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
                        if let Some(out) = op.out.as_ref() {
                            func_obj_names.insert(out.clone(), name.clone());
                        }
                    }
                    _ => {}
                }
            }
            for op in &func_ir.ops {
                if op.kind != "set_attr_generic_obj" {
                    continue;
                }
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
                let Some(func_name) = func_obj_names.get(&args[0]) else {
                    continue;
                };
                match attr {
                    "__molt_is_generator__"
                    | "__molt_is_coroutine__"
                    | "__molt_is_async_generator__" => {
                        let val_name = &args[1];
                        let is_true = const_bools
                            .get(val_name)
                            .copied()
                            .or_else(|| const_values.get(val_name).map(|val| *val != 0))
                            .unwrap_or(false);
                        if is_true {
                            if !func_name.ends_with("_poll") {
                                continue;
                            }
                            let kind = match attr {
                                "__molt_is_generator__" => TrampolineKind::Generator,
                                "__molt_is_coroutine__" => TrampolineKind::Coroutine,
                                "__molt_is_async_generator__" => TrampolineKind::AsyncGen,
                                _ => TrampolineKind::Plain,
                            };
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
                        let val_name = &args[1];
                        if let Some(size) = const_values.get(val_name) {
                            task_closure_sizes.insert(func_name.clone(), *size);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    // Detect leaf functions via the whole-program TIR call graph (Tier-0 S4).
    // A leaf makes no call of any kind and therefore cannot recurse, so call
    // sites targeting it may skip the recursion guard. The TIR call graph is
    // strictly more precise than the former raw-SimpleIR "has no call op" scan
    // (TIR DCE / devirtualization may have removed calls the raw IR still
    // carried), and it conservatively treats dynamic dispatch (`CallMethod`) and
    // indirect/opaque calls as recursion-capable  never marking a function that
    // retains a call as a leaf. See `tir::call_graph` and `tir::module_phase`.
    let leaf_functions = if compute_leaves {
        let leaves = compute_leaf_functions_via_call_graph(functions);
        if !leaves.is_empty() {
            eprintln!(
                "MOLT_BACKEND: leaf functions (skip recursion guard): {} detected",
                leaves.len()
            );
        }
        leaves
    } else {
        BTreeSet::new()
    };

    NativeBackendIrAnalysis {
        defined_functions,
        closure_functions,
        task_kinds,
        task_closure_sizes,
        leaf_functions,
    }
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn analyze_native_backend_ir(
    ir: &SimpleIR,
    compute_leaves: bool,
) -> NativeBackendIrAnalysis {
    analyze_native_backend_functions(&ir.functions, compute_leaves)
}

/// Compute the leaf-function set over the whole-program TIR call graph
/// (Tier-0 S4). Lifts the SimpleIR `functions` to a [`crate::tir::TirModule`]
/// and returns the leaf set of its [`crate::tir::CallGraph`]. The transient
/// `TirModule` is dropped as soon as the leaf set is extracted, so peak
/// additional memory is one whole-program TIR lift (the same function set
/// already held as `FunctionIR`), not a retained copy.
///
/// ## Why this builds the call graph directly, NOT via `run_module_pipeline`
///
/// `run_module_pipeline` runs the **E1 inliner** (a body transform) and already
/// ran earlier in `compile`  the `FunctionIR`s analyzed HERE are the
/// post-inline, post-`split_megafunctions` program. The leaf set gates the
/// recursion-guard skip at call sites in the *emitted* code, so it must
/// describe exactly this final function set (megafunction chunk functions
/// included), which the pre-split `ModuleAnalysis` cannot. Re-running the full
/// module pipeline here would re-inline; a plain `CallGraph::build` over the
/// final bodies is the sound leaf authority.
///
/// Extern functions (bodies in `stdlib_shared.o`) carry no ops here; they lift
/// to call-free TIR and would appear "leaf", but the leaf set only gates the
/// recursion-guard skip at *direct* call sites to *defined* functions, so an
/// extern entry is harmless. We exclude them to keep the set identity-equal to
/// the set of real local function bodies.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn compute_leaf_functions_via_call_graph(
    functions: &[FunctionIR],
) -> BTreeSet<String> {
    let tir_functions: Vec<crate::tir::TirFunction> = functions
        .iter()
        .filter(|f| !f.is_extern)
        .map(crate::tir::lower_from_simple::lower_to_tir)
        .collect();
    let module = crate::tir::TirModule {
        name: "native_leaf_analysis".to_string(),
        functions: tir_functions,
    };
    crate::tir::CallGraph::build(&module).leaf_functions()
}

pub(crate) fn parse_truthy_env(raw: &str) -> bool {
    let norm = raw.trim().to_ascii_lowercase();
    matches!(norm.as_str(), "1" | "true" | "yes" | "on")
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn compute_function_has_ret(
    functions: &[FunctionIR],
) -> BTreeMap<String, bool> {
    functions
        .iter()
        .map(|func| (func.name.clone(), function_requires_value_return(func)))
        .collect()
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn merge_function_arities(
    module_context: Option<&NativeBackendModuleContext>,
    local_function_arities: BTreeMap<String, usize>,
) -> BTreeMap<String, usize> {
    let mut merged = module_context
        .map(|context| context.function_arities.clone())
        .unwrap_or_default();
    merged.extend(local_function_arities);
    merged
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn merge_function_has_ret(
    module_context: Option<&NativeBackendModuleContext>,
    local_function_has_ret: BTreeMap<String, bool>,
) -> BTreeMap<String, bool> {
    let mut merged = module_context
        .map(|context| context.function_has_ret.clone())
        .unwrap_or_default();
    merged.extend(local_function_has_ret);
    merged
}

/// Union the whole-program `closure_functions` set (carried in the module
/// context across batches) with the CURRENT batch's local scan.
///
/// SOUNDNESS  why this must be a union, not a replace (the bug class fixed in
/// design-20 finding #3C activation): `closure_functions` decides whether a
/// `call_guarded`/`call`/`call_internal` site extracts the closure env from the
/// callee function object and prepends it as arg 0 (`function_compiler.rs`
/// "extract env from function object"). It is keyed by the names that appear in
/// `func_new_closure(name)` ops. The module context is built ONCE per
/// compilation unit  for the stdlib cache it is built from the stdlib
/// functions ONLY (`main.rs` `stdlib_module_context`), so it does NOT contain a
/// user program's closures. When a batch that DEFINES a user closure is
/// compiled with that (stdlib) module context set, REPLACING the local scan
/// dropped the user closure from the set  the call site skipped env extraction
///  the callee received a garbage/zero closure  `'object' object is not
/// subscriptable` when it indexed its cell tuple. The local scan ALWAYS knows
/// the closures defined in this batch; the module context adds cross-batch
/// knowledge. Both are required, exactly like `merge_function_arities` /
/// `merge_function_has_ret` already do for their maps. (This asymmetry was
/// latent until RC drop insertion shifted function sizes enough to change which
/// batch the user code landed in; the bug is the replace semantics, not the
/// drops.)
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn merge_closure_functions(
    module_context: Option<&NativeBackendModuleContext>,
    local_closure_functions: BTreeSet<String>,
) -> BTreeSet<String> {
    let mut merged = module_context
        .map(|context| context.closure_functions.clone())
        .unwrap_or_default();
    merged.extend(local_closure_functions);
    merged
}

/// Union the whole-program task-kind map (trampoline kind per generator/
/// coroutine/async-gen function) with the current batch's local scan. Same
/// union rationale as [`merge_closure_functions`]: the module context's map is
/// not guaranteed to contain a name defined only in this batch, and the
/// trampoline-kind decision at a `func_new`/call site must see this batch's own
/// task functions. Local entries take precedence on the (rare) key overlap.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn merge_task_kinds(
    module_context: Option<&NativeBackendModuleContext>,
    local_task_kinds: BTreeMap<String, TrampolineKind>,
) -> BTreeMap<String, TrampolineKind> {
    let mut merged = module_context
        .map(|context| context.task_kinds.clone())
        .unwrap_or_default();
    merged.extend(local_task_kinds);
    merged
}

/// Union the whole-program task-closure-size map with the current batch's local
/// scan (same union rationale as [`merge_closure_functions`]).
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn merge_task_closure_sizes(
    module_context: Option<&NativeBackendModuleContext>,
    local_task_closure_sizes: BTreeMap<String, i64>,
) -> BTreeMap<String, i64> {
    let mut merged = module_context
        .map(|context| context.task_closure_sizes.clone())
        .unwrap_or_default();
    merged.extend(local_task_closure_sizes);
    merged
}

/// Union the whole-program leaf-function set (functions with no user-level
/// calls, eligible to skip the recursion guard on direct calls) with the
/// current batch's local scan.
///
/// Leaf-ness is an intrinsic per-function property (does the body contain a
/// call?), so the two sets agree on any shared name; the union simply ensures a
/// function defined only in THIS batch is not lost when a module context built
/// from a different function set (e.g. the stdlib cache) is active. Missing a
/// genuine leaf from the set is only a perf regression (an unnecessary recursion
/// guard), never a miscompile  but the union keeps the fast path firing for the
/// current batch's own leaves, matching the other merged metadata.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn merge_leaf_functions(
    module_context: Option<&NativeBackendModuleContext>,
    local_leaf_functions: BTreeSet<String>,
) -> BTreeSet<String> {
    let mut merged = module_context
        .map(|context| context.leaf_functions.clone())
        .unwrap_or_default();
    merged.extend(local_leaf_functions);
    merged
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn backend_setting_requests_llvm(
    setting: Option<&str>,
) -> bool {
    setting == Some("llvm")
}

#[cfg(all(feature = "native-backend", not(feature = "llvm")))]
pub(in crate::native_backend::simple_backend) fn assert_requested_llvm_backend_available(
    use_llvm: bool,
) {
    if use_llvm {
        panic!(
            "MOLT_BACKEND=llvm requested but molt-backend was built without the llvm feature; rebuild with `--features llvm` or choose the Cranelift backend explicitly"
        );
    }
}

#[cfg(all(feature = "native-backend", feature = "llvm"))]
pub(in crate::native_backend::simple_backend) fn assert_requested_llvm_backend_available(
    _use_llvm: bool,
) {
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn emitted_module_symbol(name: &str) -> Option<&str> {
    name.strip_prefix("molt_init_")
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn emitted_name_matches_module_symbol(
    name: &str,
    module_symbol: &str,
) -> bool {
    if let Some(rest) = name.strip_prefix("molt_init_") {
        return rest == module_symbol;
    }
    name.starts_with(&format!("{module_symbol}__"))
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn is_user_owned_symbol(
    name: &str,
    entry_module: &str,
    stdlib_module_symbols: Option<&BTreeSet<String>>,
) -> bool {
    let entry_init = format!("molt_init_{entry_module}");
    if name == "molt_main"
        || name.starts_with(&format!("{entry_module}__"))
        || name == entry_init
        || name == "molt_init___main__"
        || name == "molt_isolate_import"
        || name == "molt_isolate_bootstrap"
    {
        return true;
    }
    if let Some(stdlib_module_symbols) = stdlib_module_symbols {
        if let Some(module_symbol) = emitted_module_symbol(name) {
            return !stdlib_module_symbols.contains(module_symbol);
        }
        return !stdlib_module_symbols
            .iter()
            .any(|module_symbol| emitted_name_matches_module_symbol(name, module_symbol));
    }
    false
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn prune_and_partition_native_stdlib(
    ir: &mut SimpleIR,
    entry_module: &str,
    stdlib_module_symbols: Option<&BTreeSet<String>>,
) -> (Vec<FunctionIR>, Vec<FunctionIR>) {
    eliminate_dead_functions(ir);
    let user_func_set: BTreeSet<String> = ir
        .functions
        .iter()
        .filter(|f| is_user_owned_symbol(&f.name, entry_module, stdlib_module_symbols))
        .map(|f| f.name.clone())
        .collect();
    let all_funcs: Vec<_> = ir.functions.drain(..).collect();
    let (user_remaining, mut stdlib_funcs): (Vec<_>, Vec<_>) = all_funcs
        .into_iter()
        .partition(|f| user_func_set.contains(&f.name));
    let mut seen: BTreeSet<String> = BTreeSet::new();
    stdlib_funcs.retain(|f| seen.insert(f.name.clone()));
    (user_remaining, stdlib_funcs)
}

/// The names of the functions `externalize_shared_stdlib_partition` *will*
/// externalize into `stdlib_shared.o` (their definitions live in the shared
/// object; this app object must reference them as undefined externals).
///
/// Computed up front  BEFORE the module-phase inliner runs  so the inliner can
/// treat these as **external-linkage** functions and refuse to inline them: a
/// caller that does not own a callee's canonical definition (it lives in another
/// object) must not splice in a private copy of its body. Doing so would (a)
/// drop the external reference the linker resolves against `stdlib_shared.o`,
/// breaking the partition contract, and (b)  once `externalize_*` later clears
/// the in-app body  leave the app running a stale private fork of a function
/// whose real definition is the shared one.
///
/// Returns an empty set when the partition is inactive (no `MOLT_STDLIB_OBJ`, no
/// `MOLT_ENTRY_MODULE`, or the shared object file is absent), so the inliner is
/// unconstrained in the common (non-partitioned) build. This mirrors the exact
/// activation guards and `is_user_owned_symbol` predicate
/// `externalize_shared_stdlib_partition` uses, so the do-not-inline set and the
/// later externalized set are computed from one source of truth.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn shared_stdlib_external_symbols(
    ir: &SimpleIR,
) -> BTreeSet<String> {
    let Some(stdlib_obj_path) = std::env::var("MOLT_STDLIB_OBJ").ok() else {
        return BTreeSet::new();
    };
    let Ok(entry_module) = std::env::var("MOLT_ENTRY_MODULE") else {
        return BTreeSet::new();
    };
    if !std::path::Path::new(&stdlib_obj_path).exists() {
        return BTreeSet::new();
    }
    let explicit_stdlib_module_symbols =
        crate::stdlib_module_symbols::stdlib_module_symbols_from_env_or_panic();
    ir.functions
        .iter()
        .filter(|f| {
            !is_user_owned_symbol(
                &f.name,
                &entry_module,
                explicit_stdlib_module_symbols.as_ref(),
            )
        })
        .map(|f| f.name.clone())
        .collect()
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn externalize_shared_stdlib_partition(
    ir: &mut SimpleIR,
) {
    let Some(stdlib_obj_path) = std::env::var("MOLT_STDLIB_OBJ").ok() else {
        return;
    };
    let Ok(entry_module) = std::env::var("MOLT_ENTRY_MODULE") else {
        return;
    };
    let stdlib_path = std::path::Path::new(&stdlib_obj_path);
    if !stdlib_path.exists() {
        return;
    }
    let explicit_stdlib_module_symbols =
        crate::stdlib_module_symbols::stdlib_module_symbols_from_env_or_panic();
    let (mut user_remaining, mut stdlib_funcs) = prune_and_partition_native_stdlib(
        ir,
        &entry_module,
        explicit_stdlib_module_symbols.as_ref(),
    );
    let mut retained = std::mem::take(&mut user_remaining);
    for mut func in std::mem::take(&mut stdlib_funcs) {
        crate::externalize_function_with_signature(&mut func);
        retained.push(func);
    }
    ir.functions = retained;
}
