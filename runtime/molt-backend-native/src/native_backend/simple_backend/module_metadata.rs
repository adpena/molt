use super::*;
use crate::stdlib_module_symbols::{
    is_user_owned_symbol, original_partition_source, partition_user_owned_functions,
};

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

/// A required, one-consumer projection of final typed function custody.
/// Entries cover every local function, including bodies that cannot certify a
/// leaf. This is never serialized into early/cross-batch module context.
#[cfg(feature = "native-backend")]
#[derive(Default)]
pub(in crate::native_backend::simple_backend) struct NativeFinalCallbacks {
    eligibility: BTreeMap<String, NativeLeafEligibility>,
}

#[cfg(feature = "native-backend")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NativeLeafEligibility {
    Unfinalized,
    Callback,
    Leaf,
}

#[cfg(feature = "native-backend")]
impl NativeFinalCallbacks {
    pub(in crate::native_backend::simple_backend) fn record(
        &mut self,
        function: &crate::tir::TirFunction,
    ) {
        let finalized = matches!(
            function
                .attrs
                .get(crate::tir::passes::drop_insertion::DROP_INSERTED_ATTR),
            Some(crate::tir::ops::AttrValue::Bool(true)),
        );
        let eligibility = if !finalized {
            // Legacy cleanup can still mint finalizer calls. Exception-only
            // drop authority does not certify a function's complete lifetime.
            NativeLeafEligibility::Unfinalized
        } else if crate::tir::call_sites::FunctionCallSites::for_function(function)
            .may_call_python()
        {
            NativeLeafEligibility::Callback
        } else {
            NativeLeafEligibility::Leaf
        };
        assert!(
            self.eligibility
                .insert(function.name.clone(), eligibility)
                .is_none(),
            "native final callback custody produced duplicate function '{}'",
            function.name,
        );
    }

    fn into_leaf_functions(mut self, functions: &[FunctionIR]) -> BTreeSet<String> {
        let mut leaves = BTreeSet::new();
        for function in functions.iter().filter(|function| !function.is_extern) {
            let eligibility = self.eligibility.remove(&function.name).unwrap_or_else(|| {
                panic!(
                    "native final callback custody is missing local function '{}'",
                    function.name,
                )
            });
            let finalized = function
                .ops
                .iter()
                .any(|op| op.kind == crate::tir::passes::drop_insertion::DROP_INSERTED_ATTR);
            assert_eq!(
                eligibility != NativeLeafEligibility::Unfinalized,
                finalized,
                "native final callback custody disagrees with final lifetime authority for '{}'",
                function.name,
            );
            if eligibility == NativeLeafEligibility::Leaf {
                leaves.insert(function.name.clone());
            }
        }
        // DFE and shared-stdlib externalization may retire local bodies after
        // their projection. Their facts die here and never certify an extern.
        leaves
    }
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
    final_callbacks: NativeFinalCallbacks,
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

    // Typed callback facts were streamed from final lifetime custody before
    // back-conversion. The final SimpleIR rewrites preserve callback eligibility;
    // consume only the surviving local function identities here.
    let leaf_functions = final_callbacks.into_leaf_functions(functions);
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
    partition_user_owned_functions(ir, |name| {
        is_user_owned_symbol(
            original_partition_source(name, partition_sources),
            entry_module,
            stdlib_module_symbols,
        )
    })
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
