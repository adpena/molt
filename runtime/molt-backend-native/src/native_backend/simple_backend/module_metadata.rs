use super::*;
use crate::stdlib_module_symbols::{is_user_owned_symbol, original_partition_source};

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) struct NativeBackendIrAnalysis {
    pub(in crate::native_backend::simple_backend) defined_functions: BTreeSet<String>,
    pub(in crate::native_backend::simple_backend) closure_functions: BTreeSet<String>,
    pub(in crate::native_backend::simple_backend) task_kinds: BTreeMap<String, TrampolineKind>,
    pub(in crate::native_backend::simple_backend) task_closure_sizes: BTreeMap<String, i64>,
    /// Final bodies with no synchronous Python callback site, including
    /// protocol dispatch and lifetime releases. These can skip direct-call guards.
    pub(in crate::native_backend::simple_backend) leaf_functions: BTreeSet<String>,
}

#[cfg(feature = "native-backend")]
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct NativeFunctionLinkageAbi {
    /// Target-neutral source ABI frozen from the owning FunctionIR before
    /// partitioning. Consumer declarations must match it exactly.
    pub source_signature: crate::ir::ExternFunctionSignature,
    /// Entry parameter custody frozen with the source signature (design 20
    /// §1.6). A consumer declaration must carry it exactly, so a caller in one
    /// object adopts precisely what the entry in another takes over.
    pub parameter_custody: Vec<crate::ir::ParameterCustody>,
    /// Exact machine-carrier types frozen before the function set is split
    /// into independently compiled objects.
    pub param_types: Vec<crate::tir::types::TirType>,
    /// `None` is a true native-void ABI. `Some` is the exact return carrier.
    pub return_type: Option<crate::tir::types::TirType>,
}

#[cfg(feature = "native-backend")]
#[derive(Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct NativeBackendModuleContext {
    #[serde(default)]
    pub(in crate::native_backend::simple_backend) partition_sources: BTreeMap<String, String>,
    pub(in crate::native_backend::simple_backend) function_arities: BTreeMap<String, usize>,
    pub(in crate::native_backend::simple_backend) function_has_ret: BTreeMap<String, bool>,
    pub(in crate::native_backend::simple_backend) closure_functions: BTreeSet<String>,
    pub(in crate::native_backend::simple_backend) task_kinds: BTreeMap<String, TrampolineKind>,
    pub(in crate::native_backend::simple_backend) task_closure_sizes: BTreeMap<String, i64>,
    /// Captured whole-program linkage ABI, dependency-projected into each job.
    /// Provider definitions and consumer declarations must read the same row;
    /// neither may reconstruct a machine signature from its local body subset.
    pub(in crate::native_backend::simple_backend) function_linkage_abis:
        BTreeMap<String, NativeFunctionLinkageAbi>,
}

#[cfg(feature = "native-backend")]
impl NativeBackendModuleContext {
    /// Exact names whose context rows this object can consume. The generated
    /// defined-function edges include indirect/name-taking and task references;
    /// callable metadata contributes runtime callable constructors as well.
    /// No sibling symbol is inferred from a spelling convention.
    pub fn object_dependencies(functions: &[FunctionIR]) -> BTreeSet<String> {
        let mut names: BTreeSet<String> = functions.iter().map(|f| f.name.clone()).collect();
        for function in functions {
            for op in &function.ops {
                if crate::tir::op_kinds_generated::simpleir_kind_references_defined_function(
                    &op.kind,
                ) && let Some(name) = op.s_value.as_ref()
                {
                    names.insert(name.clone());
                }
            }
        }
        names.extend(
            molt_tir::trampolines::CallableMetadata::from_functions(functions)
                .escaped_callable_targets,
        );
        names
    }

    /// Produce the context actually transported to, hashed for, and consumed by
    /// one native object job. Unrelated whole-program rows never reach codegen.
    /// Every field is explicitly destructured so adding context state requires
    /// deciding its projection here rather than silently omitting a new input.
    pub fn project_object_dependencies(&self, names: &BTreeSet<String>) -> Self {
        let Self {
            partition_sources,
            function_arities,
            function_has_ret,
            closure_functions,
            task_kinds,
            task_closure_sizes,
            function_linkage_abis,
        } = self;
        let mut origins = BTreeMap::new();
        for name in names {
            let mut current = name.as_str();
            let mut chain = BTreeSet::new();
            while let Some(origin) = partition_sources.get(current) {
                assert!(
                    chain.insert(current),
                    "cyclic native partition source for {name}"
                );
                origins.insert(current.to_string(), origin.clone());
                current = origin;
            }
        }
        Self {
            partition_sources: origins,
            function_arities: function_arities
                .iter()
                .filter(|(name, _)| names.contains(*name))
                .map(|(name, row)| (name.clone(), *row))
                .collect(),
            function_has_ret: function_has_ret
                .iter()
                .filter(|(name, _)| names.contains(*name))
                .map(|(name, row)| (name.clone(), *row))
                .collect(),
            closure_functions: closure_functions.intersection(names).cloned().collect(),
            task_kinds: task_kinds
                .iter()
                .filter(|(name, _)| names.contains(*name))
                .map(|(name, row)| (name.clone(), *row))
                .collect(),
            task_closure_sizes: task_closure_sizes
                .iter()
                .filter(|(name, _)| names.contains(*name))
                .map(|(name, row)| (name.clone(), *row))
                .collect(),
            function_linkage_abis: function_linkage_abis
                .iter()
                .filter(|(name, _)| names.contains(*name))
                .map(|(name, row)| (name.clone(), row.clone()))
                .collect(),
        }
    }

    pub fn original_function_name<'a>(&'a self, name: &'a str) -> &'a str {
        original_partition_source(name, &self.partition_sources)
    }

    pub fn function_linkage_abi(&self, name: &str) -> Option<&NativeFunctionLinkageAbi> {
        self.function_linkage_abis.get(name)
    }

    /// Prove that every declaration consumed from this context has exactly the
    /// target-neutral signature and machine shape frozen by its body owner.
    /// Local bodies with a row are checked too; local synthetic functions may
    /// legitimately have no whole-program row and derive one in the compiler.
    pub fn validate_function_linkage_abis(&self, functions: &[FunctionIR]) -> Result<(), String> {
        for function in functions {
            let signature = function.function_signature()?;
            let Some(linkage_abi) = self.function_linkage_abis.get(&function.name) else {
                if function.is_extern {
                    return Err(format!(
                        "extern function `{}` has no frozen native linkage ABI",
                        function.name
                    ));
                }
                continue;
            };
            if linkage_abi.param_types.len() != linkage_abi.source_signature.arity {
                return Err(format!(
                    "native linkage ABI `{}` has {} carriers but source arity {}",
                    function.name,
                    linkage_abi.param_types.len(),
                    linkage_abi.source_signature.arity
                ));
            }
            if linkage_abi.return_type.is_some() != linkage_abi.source_signature.returns_value {
                return Err(format!(
                    "native linkage ABI `{}` return carrier disagrees with its frozen source signature",
                    function.name
                ));
            }
            if signature != linkage_abi.source_signature {
                return Err(format!(
                    "function `{}` signature {:?} disagrees with frozen native linkage signature {:?}",
                    function.name, signature, linkage_abi.source_signature
                ));
            }
            if function.parameter_custody != linkage_abi.parameter_custody {
                return Err(format!(
                    "function `{}` parameter custody {:?} disagrees with frozen native linkage custody {:?}",
                    function.name, function.parameter_custody, linkage_abi.parameter_custody
                ));
            }
        }
        Ok(())
    }

    pub(in crate::native_backend::simple_backend) fn from_functions(
        functions: &mut Vec<FunctionIR>,
    ) -> Self {
        // Capture source task/closure facts without constructing unbounded TIR.
        let source_callables = molt_tir::trampolines::CallableMetadata::from_functions(functions);
        let mut ir = SimpleIR {
            functions: std::mem::take(functions),
            profile: None,
        };
        let partition_sources = split_megafunctions(&mut ir);
        *functions = ir.functions;
        let timing = crate::env_setting("MOLT_BACKEND_TIMING")
            .as_deref()
            .map(parse_truthy_env)
            .unwrap_or(false);
        let started = timing.then(std::time::Instant::now);
        let mut unique_names = BTreeSet::new();
        for function in functions.iter() {
            assert!(
                unique_names.insert(function.name.as_str()),
                "duplicate FunctionIR name `{}` cannot own a deterministic native linkage ABI",
                function.name
            );
        }
        // Freeze the source machine ABI before partitioning. These bodies have
        // not completed worker lifetime finalization and cannot certify leaves.
        // Final callback effects are derived only by the codegen worker.
        let tir_functions: Vec<crate::tir::TirFunction> = functions
            .iter()
            .filter(|function| !function.is_extern)
            .map(crate::tir::lower_from_simple::lower_to_tir)
            .collect();
        let function_linkage_abis = functions
            .iter()
            .filter(|function| !function.is_extern)
            .zip(&tir_functions)
            .map(|(function, tir)| {
                let source_signature = function
                    .function_signature()
                    .unwrap_or_else(|error| panic!("invalid linkage ABI source: {error}"));
                let param_types = crate::representation_plan::native_linkage_param_types(tir);
                let return_type = source_signature
                    .returns_value
                    .then(|| tir.return_type.clone());
                (
                    function.name.clone(),
                    NativeFunctionLinkageAbi {
                        source_signature,
                        parameter_custody: function.parameter_custody.clone(),
                        param_types,
                        return_type,
                    },
                )
            })
            .collect();
        let context = Self {
            partition_sources,
            function_arities: functions
                .iter()
                .map(|func| (func.name.clone(), func.params.len()))
                .collect(),
            function_has_ret: compute_function_has_ret(functions),
            closure_functions: source_callables
                .trampoline_specs
                .iter()
                .filter(|(_, (_, has_closure))| *has_closure)
                .map(|(name, _)| name.clone())
                .collect(),
            task_kinds: source_callables.task_kinds,
            task_closure_sizes: source_callables.task_closure_sizes,
            function_linkage_abis,
        };
        if let Some(started) = started {
            let carrier_count = context
                .function_linkage_abis
                .values()
                .map(|abi| abi.param_types.len() + usize::from(abi.return_type.is_some()))
                .sum::<usize>();
            eprintln!(
                "MOLT_BACKEND_TIMING: froze {} native linkage ABI rows / {} carriers from {} functions in {:.2?}",
                context.function_linkage_abis.len(),
                carrier_count,
                functions.len(),
                started.elapsed(),
            );
        }
        context
    }
}

/// Analyze the native backend's SimpleIR function set.
///
/// Source callable facts arrive from pipeline custody. Final bodies contribute
/// constructor/escape facts only; lowered marker operands cannot re-author
/// source task facts. Callback/leaf facts always come from these final bodies;
/// pre-pipeline shared context never supplies or overrides them.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn analyze_native_backend_ir(
    ir: &SimpleIR,
    mut callable_metadata: molt_tir::trampolines::CallableMetadata,
) -> NativeBackendIrAnalysis {
    let functions = &ir.functions;
    let defined_functions: BTreeSet<String> = functions
        .iter()
        .filter(|func| !func.is_extern)
        .map(|func| func.name.clone())
        .collect();
    callable_metadata.merge(molt_tir::trampolines::CallableMetadata::from_functions(
        functions,
    ));
    let closure_functions = callable_metadata
        .trampoline_specs
        .iter()
        .filter(|(_, (_, has_closure))| *has_closure)
        .map(|(name, _)| name.clone())
        .collect();
    let task_kinds = callable_metadata.task_kinds;
    let task_closure_sizes = callable_metadata.task_closure_sizes;

    // Detect leaf functions via the whole-program TIR call graph (Tier-0 S4).
    // A leaf makes no call of any kind and therefore cannot recurse, so call
    // sites targeting it may skip the recursion guard. The TIR call graph is
    // strictly more precise than the former raw-SimpleIR "has no call op" scan
    // (TIR DCE / devirtualization may have removed calls the raw IR still
    // carried), and it conservatively treats dynamic dispatch (`CallMethod`) and
    // indirect/opaque calls as recursion-capable  never marking a function that
    // retains a call as a leaf. See `tir::call_graph` and `tir::module_phase`.
    let leaf_functions = compute_leaf_functions_via_call_graph(functions);
    if !leaf_functions.is_empty() {
        eprintln!(
            "MOLT_BACKEND: final-body leaf functions (skip recursion guard): {} detected",
            leaf_functions.len()
        );
    }

    NativeBackendIrAnalysis {
        defined_functions,
        closure_functions,
        task_kinds,
        task_closure_sizes,
        leaf_functions,
    }
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
/// post-inline, post-lifetime-finalization, post-`split_megafunctions` program. The leaf set gates the
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
    compute_leaf_functions_from_tir(tir_functions)
}

#[cfg(feature = "native-backend")]
fn compute_leaf_functions_from_tir(
    tir_functions: Vec<crate::tir::TirFunction>,
) -> BTreeSet<String> {
    // A TIR callback graph cannot see releases later minted by legacy native
    // value tracking. Only the full lifetime-finalization fact disables that
    // competing authority (including line/branch/return cleanup). The narrower
    // exception-region marker does not qualify. Unfinalized bodies keep guards.
    let module = crate::tir::TirModule {
        name: "native_leaf_analysis".to_string(),
        functions: tir_functions
            .into_iter()
            .filter(|function| {
                matches!(
                    function
                        .attrs
                        .get(crate::tir::passes::drop_insertion::DROP_INSERTED_ATTR),
                    Some(crate::tir::ops::AttrValue::Bool(true)),
                )
            })
            .collect(),
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

/// The entry custody of every function a `func_new` can name, each from its
/// own parameter declaration: the whole-program linkage rows, then
/// `functions`, bodies and extern declarations alike. Nothing is inferred for
/// a function without a declaration; lowering a `func_new` that names one
/// fails.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn merge_function_entry_custody(
    module_context: Option<&NativeBackendModuleContext>,
    functions: &[FunctionIR],
) -> BTreeMap<String, molt_codegen_abi::EntryCustodyDeclaration> {
    fn declare(
        signature: &crate::ir::ExternFunctionSignature,
        custody: &[crate::ir::ParameterCustody],
    ) -> molt_codegen_abi::EntryCustodyDeclaration {
        let transferred: Vec<bool> = custody
            .iter()
            .map(|custody| matches!(custody, crate::ir::ParameterCustody::Transferred))
            .collect();
        molt_codegen_abi::EntryCustodyDeclaration::declare(
            signature.has_closure,
            signature.arity,
            &transferred,
        )
    }
    let mut merged: BTreeMap<String, molt_codegen_abi::EntryCustodyDeclaration> = module_context
        .map(|context| {
            context
                .function_linkage_abis
                .iter()
                .map(|(name, row)| {
                    (
                        name.clone(),
                        declare(&row.source_signature, &row.parameter_custody),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    for function in functions {
        let signature = function
            .function_signature()
            .unwrap_or_else(|error| panic!("invalid native function declaration: {error}"));
        merged.insert(
            function.name.clone(),
            declare(&signature, &function.parameter_custody),
        );
    }
    merged
}

/// Union the job's projected source closure facts with its final local scan.
/// NativeBackendModuleContext is captured before user/stdlib separation, then
/// each object retains its exact dependency rows. Local transformations may
/// add definitions or remove constructors, so neither source custody nor final
/// local facts can replace the other. Calls and function-object constructors
/// consume the same retained target identity across batch boundaries.
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
/// task functions. Overlapping immutable facts must agree.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn merge_task_kinds(
    module_context: Option<&NativeBackendModuleContext>,
    local_task_kinds: BTreeMap<String, TrampolineKind>,
) -> BTreeMap<String, TrampolineKind> {
    let mut merged = module_context
        .map(|context| context.task_kinds.clone())
        .unwrap_or_default();
    molt_tir::trampolines::merge_callable_facts(
        &mut merged,
        local_task_kinds,
        "callable task kind",
    );
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
    molt_tir::trampolines::merge_callable_facts(
        &mut merged,
        local_task_closure_sizes,
        "callable closure size",
    );
    merged
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn prune_and_partition_native_stdlib(
    ir: &mut SimpleIR,
    entry_module: &str,
    stdlib_module_symbols: Option<&BTreeSet<String>>,
    module_registry_roots: &BTreeSet<String>,
    partition_sources: &BTreeMap<String, String>,
) -> (Vec<FunctionIR>, Vec<FunctionIR>) {
    eliminate_dead_functions_with_roots(ir, module_registry_roots);
    let user_func_set: BTreeSet<String> = ir
        .functions
        .iter()
        .filter(|f| {
            is_user_owned_symbol(
                original_partition_source(&f.name, partition_sources),
                entry_module,
                stdlib_module_symbols,
            )
        })
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
    partition_sources: &BTreeMap<String, String>,
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
                original_partition_source(&f.name, partition_sources),
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
    module_registry_roots: &BTreeSet<String>,
    partition_sources: &BTreeMap<String, String>,
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
        module_registry_roots,
        partition_sources,
    );
    let mut retained = std::mem::take(&mut user_remaining);
    for mut func in std::mem::take(&mut stdlib_funcs) {
        crate::externalize_function_with_signature(&mut func);
        retained.push(func);
    }
    ir.functions = retained;
}
