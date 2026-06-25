//! Native (Cranelift) `SimpleBackend` code generation: NaN-box/box helpers,
//! variable/block helpers, RC emission, cleanup tracking, the `SimpleBackend`
//! struct and its codegen impls, plus the native-backend unit tests. Moved
//! verbatim from lib.rs as a pure structural split. This module is `cfg(native
//! -backend)` via its declaration in `native_backend/mod.rs`; the crate-root
//! glob (`use super::*`) makes every crate-root item (FunctionIR, OpIR, passes,
//! the `native_backend_consts` glob, etc.) visible exactly as in lib.rs.

use super::*;
// The shared Cranelift / std collection imports (and the `std::fmt::Write`
// trait used by the `writeln!` in `dump_ops_to_string`) live in
// `native_backend/mod.rs` and at the crate root, and reach this module
// unqualified via `use super::*`, matching how they reached this code when it
// lived at the crate root in `lib.rs`.

/// Pre-computed NaN-box tag mask constants materialized at each helper site.
///
/// These values are plain immediates, not Cranelift `Variable`s. Keeping
/// representation constants out of SSA repair prevents label/exception CFG
/// stitching from turning immutable tag facts into block parameters.
#[cfg(feature = "native-backend")]
#[derive(Clone, Copy)]
pub(crate) struct NanBoxConsts {
    /// `(QNAN | TAG_MASK) as i64`
    pub(crate) qnan_tag_mask: i64,
    /// `(QNAN | TAG_INT) as i64`
    pub(crate) qnan_tag_int: i64,
    /// `(QNAN | TAG_PTR) as i64`
    pub(crate) qnan_tag_ptr: i64,
    /// `INT_SHIFT` (17)
    int_shift: i64,
    /// `POINTER_MASK as i64`
    pub(crate) pointer_mask: i64,
    /// `(QNAN | TAG_BOOL) as i64`
    pub(crate) qnan_tag_bool: i64,
    /// `INT_WIDTH as i64` (47) — used in fused_both_int_check
    int_width: i64,
    /// `48i64` — shift to isolate tag field for nanboxed-special / int checks
    shift_48: i64,
    /// `0x7FF9i64` — base of special-tag range
    special_base: i64,
    /// `5i64` — width of special-tag range
    special_limit: i64,
    /// `((QNAN | TAG_INT) >> 48) as i64` — 16-bit tag for nanboxed int check
    int_tag_16: i64,
    /// `INT_MASK as i64` — mask for box_int_value
    pub(crate) int_mask: i64,
    /// `16i64` — sign-extension shift for unbox_ptr_value
    shift_16: i64,
    /// `CANONICAL_NAN_BITS as i64` — canonical NaN for box_float_value
    canonical_nan: i64,
}

#[cfg(feature = "native-backend")]
impl NanBoxConsts {
    pub(crate) fn new(_builder: &mut FunctionBuilder) -> Self {
        Self {
            qnan_tag_mask: (QNAN | TAG_MASK) as i64,
            qnan_tag_int: (QNAN | TAG_INT) as i64,
            qnan_tag_ptr: (QNAN | TAG_PTR) as i64,
            int_shift: INT_SHIFT,
            pointer_mask: POINTER_MASK as i64,
            qnan_tag_bool: (QNAN | TAG_BOOL) as i64,
            int_width: INT_WIDTH as i64,
            shift_48: 48,
            special_base: 0x7FF9,
            special_limit: 5,
            int_tag_16: ((QNAN | TAG_INT) >> 48) as i64,
            int_mask: INT_MASK as i64,
            shift_16: 16,
            canonical_nan: CANONICAL_NAN_BITS as i64,
        }
    }
}

#[cfg(feature = "native-backend")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ImportSignatureShape {
    params: Vec<String>,
    returns: Vec<String>,
}

#[cfg(feature = "native-backend")]
impl ImportSignatureShape {
    pub(crate) fn from_types(params: &[types::Type], returns: &[types::Type]) -> Self {
        Self {
            params: params.iter().map(ToString::to_string).collect(),
            returns: returns.iter().map(ToString::to_string).collect(),
        }
    }
}

#[cfg(feature = "native-backend")]
struct NativeBackendIrAnalysis {
    defined_functions: BTreeSet<String>,
    closure_functions: BTreeSet<String>,
    task_kinds: BTreeMap<String, TrampolineKind>,
    task_closure_sizes: BTreeMap<String, i64>,
    /// Functions that contain no user-level calls (call, call_guarded,
    /// call_func, call_internal, call_indirect, call_bind, invoke_ffi).
    /// These can skip the recursion guard on direct calls.
    leaf_functions: BTreeSet<String>,
}

#[cfg(feature = "native-backend")]
const TIR_OPTIMIZATION_BATCH_FUNCTION_LIMIT: usize = 128;
#[cfg(feature = "native-backend")]
const TIR_OPTIMIZATION_BATCH_OP_BUDGET: usize = 8_000;
#[cfg(feature = "native-backend")]
const TIR_OPTIMIZATION_BASELINE_MEMORY_BYTES: usize = 4 * 1024 * 1024 * 1024;
#[cfg(feature = "native-backend")]
const TIR_OPTIMIZATION_WORKER_MEMORY_BYTES: usize = 8 * 1024 * 1024 * 1024;
#[cfg(feature = "native-backend")]
const TIR_OPTIMIZATION_WAVE_FUNCTIONS_PER_THREAD: usize = 1;
#[cfg(feature = "native-backend")]
const TIR_OPTIMIZATION_WAVE_OPS_PER_THREAD: usize = 1_000;
#[cfg(feature = "native-backend")]
const DEFERRED_CODEGEN_FLUSH_FUNCTION_LIMIT: usize = 16;
#[cfg(feature = "native-backend")]
const DEFERRED_CODEGEN_FLUSH_OP_BUDGET: usize = 4_000;

#[cfg(feature = "native-backend")]
#[derive(Debug, Eq, PartialEq)]
struct TirOptimizationWorkItem {
    index: usize,
    content_hash: String,
    op_count: usize,
}

#[cfg(feature = "native-backend")]
struct TirOptimizationInput {
    index: usize,
    content_hash: String,
    name: String,
    params: Vec<String>,
    ops: Vec<OpIR>,
    param_types: Option<Vec<String>>,
}

#[cfg(feature = "native-backend")]
struct TirOptimizationOutput {
    index: usize,
    content_hash: String,
    simple_ops: Vec<OpIR>,
    tir_func: crate::tir::function::TirFunction,
}

#[cfg(feature = "native-backend")]
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
struct TirOptimizationResourcePlan {
    threads: usize,
    wave_function_limit: usize,
    wave_op_budget: usize,
}

#[cfg(feature = "native-backend")]
fn partition_tir_optimization_work_items_with_limits(
    work_items: Vec<TirOptimizationWorkItem>,
    max_functions_per_batch: usize,
    max_ops_per_batch: usize,
) -> Vec<Vec<TirOptimizationWorkItem>> {
    let max_functions = max_functions_per_batch.max(1);
    let max_ops = max_ops_per_batch.max(1);
    let mut batches: Vec<Vec<TirOptimizationWorkItem>> = Vec::new();
    let mut current: Vec<TirOptimizationWorkItem> = Vec::new();
    let mut current_ops = 0usize;

    for item in work_items {
        let item_ops = item.op_count.max(1);
        let count_full = current.len() >= max_functions;
        let ops_full = !current.is_empty() && current_ops.saturating_add(item_ops) > max_ops;
        if count_full || ops_full {
            batches.push(std::mem::take(&mut current));
            current_ops = 0;
        }
        current_ops = current_ops.saturating_add(item_ops);
        current.push(item);
    }

    if !current.is_empty() {
        batches.push(current);
    }
    batches
}

#[cfg(feature = "native-backend")]
#[cfg(test)]
fn partition_tir_optimization_work_items(
    work_items: Vec<TirOptimizationWorkItem>,
) -> Vec<Vec<TirOptimizationWorkItem>> {
    partition_tir_optimization_work_items_with_limits(
        work_items,
        TIR_OPTIMIZATION_BATCH_FUNCTION_LIMIT,
        TIR_OPTIMIZATION_BATCH_OP_BUDGET,
    )
}

#[cfg(feature = "native-backend")]
fn parse_positive_usize_env(name: &str) -> Option<usize> {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
}

#[cfg(feature = "native-backend")]
fn parse_nonnegative_gb_env(name: &str) -> Option<usize> {
    let gb = std::env::var(name)
        .ok()
        .and_then(|raw| raw.trim().parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value >= 0.0)?;
    Some((gb * 1024.0 * 1024.0 * 1024.0) as usize)
}

#[cfg(feature = "native-backend")]
fn env_memory_limit_bytes() -> Option<usize> {
    let available = [
        "MOLT_BACKEND_MEMORY_AVAILABLE_GB",
        "MOLT_CLI_MEMORY_AVAILABLE_GB",
        "MOLT_CLI_MEM_AVAILABLE_GB",
        "MOLT_MEMORY_AVAILABLE_GB",
        "MOLT_MEM_AVAILABLE_GB",
        "MOLT_BACKEND_MAX_RSS_GB",
    ]
    .iter()
    .find_map(|name| parse_nonnegative_gb_env(name))?;
    let reserve = [
        "MOLT_BACKEND_MEMORY_RESERVE_GB",
        "MOLT_CLI_MEMORY_RESERVE_GB",
        "MOLT_CLI_MEM_RESERVE_GB",
        "MOLT_MEMORY_RESERVE_GB",
        "MOLT_MEM_RESERVE_GB",
    ]
    .iter()
    .find_map(|name| parse_nonnegative_gb_env(name))
    .unwrap_or(0);
    Some(available.saturating_sub(reserve))
}

#[cfg(all(feature = "native-backend", unix))]
fn rlimit_address_space_bytes() -> Option<usize> {
    unsafe {
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if libc::getrlimit(libc::RLIMIT_AS, &mut limit) != 0 {
            return None;
        }
        let raw = limit.rlim_cur;
        if raw == libc::RLIM_INFINITY || raw == 0 {
            return None;
        }
        Some(raw.min(usize::MAX as libc::rlim_t) as usize)
    }
}

#[cfg(all(feature = "native-backend", not(unix)))]
fn rlimit_address_space_bytes() -> Option<usize> {
    None
}

#[cfg(feature = "native-backend")]
fn backend_memory_limit_bytes() -> Option<usize> {
    match (env_memory_limit_bytes(), rlimit_address_space_bytes()) {
        (Some(env_limit), Some(rlimit)) => Some(env_limit.min(rlimit)),
        (Some(env_limit), None) => Some(env_limit),
        (None, Some(rlimit)) => Some(rlimit),
        (None, None) => None,
    }
}

#[cfg(feature = "native-backend")]
fn tir_optimization_cpu_thread_limit() -> usize {
    parse_positive_usize_env("RAYON_NUM_THREADS")
        .or_else(|| std::thread::available_parallelism().ok().map(usize::from))
        .unwrap_or(1)
        .max(1)
}

#[cfg(feature = "native-backend")]
fn tir_optimization_resource_plan_from_limits(
    cpu_threads: usize,
    memory_limit_bytes: Option<usize>,
) -> TirOptimizationResourcePlan {
    let cpu_threads = cpu_threads.max(1);
    let memory_threads = memory_limit_bytes
        .map(|limit| {
            if limit <= TIR_OPTIMIZATION_BASELINE_MEMORY_BYTES {
                1
            } else {
                ((limit - TIR_OPTIMIZATION_BASELINE_MEMORY_BYTES)
                    / TIR_OPTIMIZATION_WORKER_MEMORY_BYTES)
                    .max(1)
            }
        })
        .unwrap_or(cpu_threads);
    let threads = cpu_threads.min(memory_threads).max(1);
    TirOptimizationResourcePlan {
        threads,
        wave_function_limit: TIR_OPTIMIZATION_BATCH_FUNCTION_LIMIT
            .min(threads.saturating_mul(TIR_OPTIMIZATION_WAVE_FUNCTIONS_PER_THREAD))
            .max(1),
        wave_op_budget: TIR_OPTIMIZATION_BATCH_OP_BUDGET
            .min(threads.saturating_mul(TIR_OPTIMIZATION_WAVE_OPS_PER_THREAD))
            .max(1),
    }
}

#[cfg(feature = "native-backend")]
fn tir_optimization_resource_plan() -> TirOptimizationResourcePlan {
    tir_optimization_resource_plan_from_limits(
        tir_optimization_cpu_thread_limit(),
        backend_memory_limit_bytes(),
    )
}

#[cfg(feature = "native-backend")]
fn should_flush_deferred_codegen(deferred_count: usize, deferred_ops: usize) -> bool {
    deferred_count > 0
        && (deferred_count >= DEFERRED_CODEGEN_FLUSH_FUNCTION_LIMIT
            || deferred_ops >= DEFERRED_CODEGEN_FLUSH_OP_BUDGET)
}

#[cfg(feature = "native-backend")]
fn trace_tir_function_enabled(name: &str) -> bool {
    std::env::var("MOLT_TIR_TRACE_FUNC")
        .ok()
        .is_some_and(|filter| filter == "1" || name.contains(&filter))
}

#[cfg(feature = "native-backend")]
fn trace_tir_function_stage(name: &str, stage: &str, simple_ops: usize) {
    if trace_tir_function_enabled(name) {
        eprintln!("[TIR-TRACE] {name} {stage}: simple_ops={simple_ops}");
    }
}

#[cfg(feature = "native-backend")]
fn native_tir_cache_hash_body(simple_body_bytes: &[u8]) -> Vec<u8> {
    let mut body = b"native-tir-function-cache-v1\0".to_vec();
    body.extend_from_slice(simple_body_bytes);
    body
}

#[cfg(feature = "native-backend")]
fn optimize_tir_input(input: TirOptimizationInput) -> TirOptimizationOutput {
    let idx = input.index;
    let content_hash = input.content_hash;
    let mut tmp_func = FunctionIR {
        name: input.name,
        params: input.params,
        ops: input.ops,
        param_types: input.param_types,
        source_file: None,
        is_extern: false,
    };
    trace_tir_function_stage(&tmp_func.name, "start", tmp_func.ops.len());
    if tmp_func.ops.iter().any(|op| op.kind == "phi") {
        rewrite_phi_to_store_load(&mut tmp_func.ops);
        trace_tir_function_stage(&tmp_func.name, "after_phi_rewrite", tmp_func.ops.len());
    }
    if tmp_func.ops.iter().any(|op| op.kind == "exception_push") {
        elide_useless_try_blocks_for_function(&mut tmp_func);
        trace_tir_function_stage(&tmp_func.name, "after_try_elision", tmp_func.ops.len());
    }
    let func_name = tmp_func.name.clone();
    let mut tir_func = crate::tir::lower_from_simple::lower_to_tir(&tmp_func);
    if trace_tir_function_enabled(&func_name) {
        eprintln!(
            "[TIR-TRACE] {func_name} after_lower_to_tir: blocks={} ops={}",
            tir_func.blocks.len(),
            tir_func
                .blocks
                .values()
                .map(|block| block.ops.len())
                .sum::<usize>()
        );
    }
    crate::tir::type_refine::refine_types(&mut tir_func);
    if trace_tir_function_enabled(&func_name) {
        eprintln!(
            "[TIR-TRACE] {func_name} after_refine_1: blocks={} ops={}",
            tir_func.blocks.len(),
            tir_func
                .blocks
                .values()
                .map(|block| block.ops.len())
                .sum::<usize>()
        );
    }
    let _stats = crate::tir::passes::run_pipeline(
        &mut tir_func,
        &crate::tir::target_info::TargetInfo::native_from_simd_caps(
            crate::tir::target_info::SimdCaps::detect_host(),
        ),
    );
    if trace_tir_function_enabled(&func_name) {
        eprintln!(
            "[TIR-TRACE] {func_name} after_pipeline: blocks={} ops={}",
            tir_func.blocks.len(),
            tir_func
                .blocks
                .values()
                .map(|block| block.ops.len())
                .sum::<usize>()
        );
    }
    crate::tir::type_refine::refine_types(&mut tir_func);
    if trace_tir_function_enabled(&func_name) {
        eprintln!(
            "[TIR-TRACE] {func_name} after_refine_2: blocks={} ops={}",
            tir_func.blocks.len(),
            tir_func
                .blocks
                .values()
                .map(|block| block.ops.len())
                .sum::<usize>()
        );
    }
    let lir_func = crate::tir::lower_to_lir::lower_function_to_lir(&tir_func, None);
    if trace_tir_function_enabled(&func_name) {
        eprintln!(
            "[TIR-TRACE] {func_name} after_lower_to_lir: blocks={} ops={}",
            lir_func.blocks.len(),
            lir_func
                .blocks
                .values()
                .map(|block| block.ops.len())
                .sum::<usize>()
        );
    }
    if let Err(errors) = crate::tir::verify_lir::verify_lir_function(&lir_func) {
        panic!(
            "[LIR] verification failed for '{}': {:?}",
            func_name, errors
        );
    }
    #[cfg(debug_assertions)]
    {
        let repr_violations = crate::tir::verify_lir_repr::verify_register_passable(&lir_func);
        if !repr_violations.is_empty() {
            eprintln!(
                "[LIR-repr] {} register-passable violation(s) in '{}': {:?}",
                repr_violations.len(),
                func_name,
                repr_violations,
            );
        }
    }
    let ops = crate::tir::lower_to_simple::lower_to_simple_ir(&tir_func);
    trace_tir_function_stage(&func_name, "after_lower_to_simple", ops.len());
    assert!(
        crate::tir::lower_to_simple::validate_labels(&ops),
        "TIR roundtrip emitted invalid labels for '{}'",
        func_name
    );
    TirOptimizationOutput {
        index: idx,
        content_hash,
        simple_ops: ops,
        tir_func,
    }
}

#[cfg(feature = "native-backend")]
#[derive(Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct NativeBackendModuleContext {
    function_arities: BTreeMap<String, usize>,
    function_has_ret: BTreeMap<String, bool>,
    closure_functions: BTreeSet<String>,
    task_kinds: BTreeMap<String, TrampolineKind>,
    task_closure_sizes: BTreeMap<String, i64>,
    leaf_functions: BTreeSet<String>,
    return_alias_summaries: BTreeMap<String, crate::passes::ReturnAliasSummary>,
}

#[cfg(feature = "native-backend")]
impl NativeBackendModuleContext {
    fn from_functions(functions: &[FunctionIR]) -> Self {
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
fn analyze_native_backend_functions(
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
    // indirect/opaque calls as recursion-capable — never marking a function that
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
fn analyze_native_backend_ir(ir: &SimpleIR, compute_leaves: bool) -> NativeBackendIrAnalysis {
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
/// ran earlier in `compile` — the `FunctionIR`s analyzed HERE are the
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
fn compute_leaf_functions_via_call_graph(functions: &[FunctionIR]) -> BTreeSet<String> {
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

#[cfg(feature = "native-backend")]
pub(crate) fn find_zero_pred_blocks(func: &Function) -> Vec<Block> {
    let mut preds: BTreeMap<Block, usize> = BTreeMap::new();
    for block in func.layout.blocks() {
        preds.entry(block).or_insert(0);
    }
    for block in func.layout.blocks() {
        for inst in func.layout.block_insts(block) {
            for dest in func.dfg.insts[inst]
                .branch_destination(&func.dfg.jump_tables, &func.dfg.exception_tables)
            {
                let dest_block = dest.block(&func.dfg.value_lists);
                *preds.entry(dest_block).or_insert(0) += 1;
            }
        }
    }
    let entry = func.layout.entry_block();
    preds
        .into_iter()
        .filter(|(block, count)| Some(*block) != entry && *count == 0)
        .map(|(block, _)| block)
        .collect()
}

#[cfg(feature = "native-backend")]
pub(crate) fn ensure_block_in_layout(builder: &mut FunctionBuilder, block: Block) {
    if builder.func.layout.is_block_inserted(block) {
        return;
    }
    if let Some(current) = builder.current_block()
        && builder.func.layout.is_block_inserted(current)
    {
        builder.insert_block_after(block, current);
        return;
    }
    builder.func.layout.append_block(block);
}

#[cfg(feature = "native-backend")]
pub(crate) fn block_has_terminator(builder: &FunctionBuilder, block: Block) -> bool {
    builder
        .func
        .layout
        .last_inst(block)
        .map(|inst| builder.func.dfg.insts[inst].opcode().is_terminator())
        .unwrap_or(false)
}

#[cfg(feature = "native-backend")]
#[allow(dead_code)]
pub(crate) fn sync_block_filled(builder: &FunctionBuilder, is_block_filled: &mut bool) {
    if let Some(block) = builder.current_block() {
        if block_has_terminator(builder, block) {
            *is_block_filled = true;
        } else {
            // The current block is open (no terminator) — clear the flag so
            // subsequent ops are not incorrectly skipped.  This fixes cases
            // where a control-flow op (e.g. check_exception) switched to a
            // fresh fallthrough block and cleared the flag via
            // switch_to_block_tracking, but a stale `true` value from a
            // previous iteration leaked through.
            *is_block_filled = false;
        }
    }
}

#[cfg(feature = "native-backend")]
pub(crate) fn switch_to_block_tracking(
    builder: &mut FunctionBuilder,
    block: Block,
    is_block_filled: &mut bool,
) {
    // Guard: if the block already has a terminator instruction, Cranelift's
    // `switch_to_block` will panic with "you cannot switch to a block which
    // is already filled".  This happens in complex control flow (e.g. stdlib
    // modules with nested try/except + if/else) where multiple paths converge
    // on the same block and a previous path already sealed it with a branch.
    // In that case we must NOT switch to it — just mark as filled so
    // subsequent ops create a fresh block or skip dead code.
    if block_has_terminator(builder, block) {
        *is_block_filled = true;
        return;
    }
    ensure_block_in_layout(builder, block);
    builder.switch_to_block(block);
    *is_block_filled = false;
}

#[cfg(feature = "native-backend")]
pub(crate) fn resolve_cleanup_value(
    builder: &mut FunctionBuilder,
    vars: &BTreeMap<String, Variable>,
    entry_vars: &BTreeMap<String, Value>,
    name: &str,
) -> Option<Value> {
    entry_vars
        .get(name)
        .copied()
        .or_else(|| var_get(builder, vars, name).map(|v| *v))
}

#[cfg(feature = "native-backend")]
pub(crate) fn box_int(val: i64) -> i64 {
    // Use INT_MASK (47 bits) not POINTER_MASK (48 bits) to match the
    // sign-extending unbox path (ishl/sshr by INT_SHIFT=17).
    let masked = (val as u64) & INT_MASK;
    (QNAN | TAG_INT | masked) as i64
}

#[cfg(feature = "native-backend")]
pub(crate) fn box_float(val: f64) -> i64 {
    if val.is_nan() {
        // Canonicalize NaN to avoid collision with the QNAN tag prefix.
        // Must match CANONICAL_NAN_BITS in molt-obj-model.
        0x7ff0_0000_0000_0001_u64 as i64
    } else {
        val.to_bits() as i64
    }
}

#[cfg(feature = "native-backend")]
pub(crate) fn box_none() -> i64 {
    (QNAN | TAG_NONE) as i64
}

#[cfg(feature = "native-backend")]
pub(crate) fn box_bool(val: i64) -> i64 {
    let bit = if val != 0 { 1u64 } else { 0u64 };
    (QNAN | TAG_BOOL | bit) as i64
}

#[cfg(feature = "native-backend")]
pub(crate) fn unbox_int(builder: &mut FunctionBuilder, val: Value, nbc: &NanBoxConsts) -> Value {
    // Debug-mode guard: verify the value actually carries the int tag before
    // unboxing.  In release builds this is a no-op; in debug builds an illegal
    // trap fires immediately if a non-int value reaches this path.
    #[cfg(debug_assertions)]
    {
        let mask = builder.ins().iconst(types::I64, nbc.qnan_tag_mask);
        let expected = builder.ins().iconst(types::I64, nbc.qnan_tag_int);
        let masked = builder.ins().band(val, mask);
        let is_int = builder.ins().icmp(IntCC::Equal, masked, expected);
        builder
            .ins()
            .trapz(is_int, cranelift_codegen::ir::TrapCode::user(1).unwrap());
    }

    // The ishl by INT_SHIFT (17) shifts out the upper 17 tag bits (QNAN+TAG),
    // then sshr sign-extends the 47-bit payload. No separate band with INT_MASK
    // is needed — the shift pair implicitly strips the tag.
    let shift = builder.ins().iconst(types::I64, nbc.int_shift);
    let shifted = builder.ins().ishl(val, shift);
    builder.ins().sshr(shifted, shift)
}

/// Unbox a NaN-boxed value that is either TAG_INT or TAG_BOOL to an i64.
///
/// Booleans are coerced to 0/1 (matching Python's `bool` subclass of `int`).
/// This is needed in `fast_int` arithmetic paths where the TIR optimizer may
/// mark an op as `fast_int` even when one or both operands are booleans.
#[cfg(feature = "native-backend")]
pub(crate) fn unbox_int_or_bool(
    builder: &mut FunctionBuilder,
    val: Value,
    nbc: &NanBoxConsts,
) -> Value {
    let mask = builder.ins().iconst(types::I64, nbc.qnan_tag_mask);
    let bool_tag = builder.ins().iconst(types::I64, nbc.qnan_tag_bool);
    let masked = builder.ins().band(val, mask);
    let is_bool = builder.ins().icmp(IntCC::Equal, masked, bool_tag);

    let bool_block = builder.create_block();
    let int_block = builder.create_block();
    let merge_block = builder.create_block();
    builder.append_block_param(merge_block, types::I64);

    builder.ins().brif(is_bool, bool_block, &[], int_block, &[]);

    // Bool path: extract bit 0 as the integer value (False=0, True=1).
    builder.switch_to_block(bool_block);
    builder.seal_block(bool_block);
    let one = builder.ins().iconst(types::I64, 1);
    let bool_val = builder.ins().band(val, one);
    jump_block(builder, merge_block, &[bool_val]);

    // Int path: normal unbox_int shift pair.
    builder.switch_to_block(int_block);
    builder.seal_block(int_block);
    let shift = builder.ins().iconst(types::I64, nbc.int_shift);
    let shifted = builder.ins().ishl(val, shift);
    let int_val = builder.ins().sshr(shifted, shift);
    jump_block(builder, merge_block, &[int_val]);

    builder.switch_to_block(merge_block);
    builder.seal_block(merge_block);
    builder.block_params(merge_block)[0]
}

#[allow(dead_code)]
#[cfg(feature = "native-backend")]
fn is_int_tag(builder: &mut FunctionBuilder, val: Value, nbc: &NanBoxConsts) -> Value {
    let mask = builder.ins().iconst(types::I64, nbc.qnan_tag_mask);
    let tag = builder.ins().iconst(types::I64, nbc.qnan_tag_int);
    let masked = builder.ins().band(val, mask);
    builder.ins().icmp(IntCC::Equal, masked, tag)
}

/// Fused tag-check-and-unbox for a single NaN-boxed value.
///
/// XORs the value against the expected int tag pattern `(QNAN | TAG_INT)`.
/// If the value is an int, the XOR zeros out the upper 17 tag bits, leaving
/// only the 47-bit payload.
///
/// Returns `(xored, unboxed)` where:
///   - `xored` can be used for the tag check: `(xored >> 47) == 0` iff the
///     value was a NaN-boxed int.
///   - `unboxed` is the sign-extended 47-bit integer payload (valid only when
///     the tag check passes).
#[cfg(feature = "native-backend")]
pub(crate) fn fused_tag_check_and_unbox_int(
    builder: &mut FunctionBuilder,
    val: Value,
    nbc: &NanBoxConsts,
) -> (Value, Value) {
    let expected_tag = builder.ins().iconst(types::I64, nbc.qnan_tag_int);
    let xored = builder.ins().bxor(val, expected_tag);
    let shift = builder.ins().iconst(types::I64, nbc.int_shift);
    let shifted = builder.ins().ishl(xored, shift);
    let unboxed = builder.ins().sshr(shifted, shift);
    (xored, unboxed)
}

/// Check that two XOR'd values both represent NaN-boxed ints.
///
/// Takes the `xored` outputs from two `fused_tag_check_and_unbox_int` calls
/// and checks that both had their tag bits zeroed (i.e., both were ints).
/// Uses BOR to combine the two values, then checks that the upper 17 bits
/// of the combined result are zero — true iff both inputs were ints.
#[cfg(feature = "native-backend")]
pub(crate) fn fused_both_int_check(
    builder: &mut FunctionBuilder,
    lhs_xored: Value,
    rhs_xored: Value,
    nbc: &NanBoxConsts,
) -> Value {
    let combined = builder.ins().bor(lhs_xored, rhs_xored);
    let tag_shift = builder.ins().iconst(types::I64, nbc.int_width);
    let upper = builder.ins().ushr(combined, tag_shift);
    builder.ins().icmp_imm(IntCC::Equal, upper, 0)
}

/// Returns true (i8 `1`) iff `val` is an inline NaN-boxed integer (`TAG_INT`) or
/// boolean (`TAG_BOOL`).
///
/// These are exactly the tags for which the trusted shift-unbox `(v << s) >> s`
/// (`unbox_int`) recovers the operand's integer value (`False`→0, `True`→1).
/// Crucially, this rejects heap pointers (`TAG_PTR`): a Python `int` whose
/// magnitude exceeds the 47-bit inline range is a BigInt carried as a `TAG_PTR`
/// NaN-box, and unboxing it would truncate the pointer to garbage. Callers use
/// this to keep the raw-int fast path correct while still accepting `bool`
/// operands (which are `int`-typed but tagged `TAG_BOOL`).
#[cfg(feature = "native-backend")]
pub(crate) fn fused_is_int_or_bool(
    builder: &mut FunctionBuilder,
    val: Value,
    nbc: &NanBoxConsts,
) -> Value {
    let mask = builder.ins().iconst(types::I64, nbc.qnan_tag_mask);
    let masked = builder.ins().band(val, mask);
    let int_tag = builder.ins().iconst(types::I64, nbc.qnan_tag_int);
    let is_int = builder.ins().icmp(IntCC::Equal, masked, int_tag);
    let bool_tag = builder.ins().iconst(types::I64, nbc.qnan_tag_bool);
    let is_bool = builder.ins().icmp(IntCC::Equal, masked, bool_tag);
    builder.ins().bor(is_int, is_bool)
}

/// Check whether a NaN-boxed value is a special tagged type (int/bool/none/ptr/pending)
/// rather than a plain f64.
///
/// All NaN-boxed specials have bits 62..48 in the range `0x7FF9..=0x7FFD`.
/// Returns true if the value IS a special (i.e., NOT a float).
#[cfg(feature = "native-backend")]
fn is_nanboxed_special(builder: &mut FunctionBuilder, val: Value, nbc: &NanBoxConsts) -> Value {
    // Shift right by 48 to isolate the tag field, then check range [0x7FF9, 0x7FFD].
    let shift48 = builder.ins().iconst(types::I64, nbc.shift_48);
    let tag16 = builder.ins().ushr(val, shift48);
    // tag16 - 0x7FF9; result < 5 means it's a tagged special
    let base = builder.ins().iconst(types::I64, nbc.special_base);
    let adjusted = builder.ins().isub(tag16, base);
    let limit = builder.ins().iconst(types::I64, nbc.special_limit);
    builder.ins().icmp(IntCC::UnsignedLessThan, adjusted, limit)
}

/// Check that both NaN-boxed values are plain f64 (not tagged specials).
#[cfg(feature = "native-backend")]
pub(crate) fn both_float_check(
    builder: &mut FunctionBuilder,
    lhs: Value,
    rhs: Value,
    nbc: &NanBoxConsts,
) -> Value {
    let lhs_special = is_nanboxed_special(builder, lhs, nbc);
    let rhs_special = is_nanboxed_special(builder, rhs, nbc);
    let either_special = builder.ins().bor(lhs_special, rhs_special);
    // both_float = !(lhs_special || rhs_special)
    // Since is_nanboxed_special returns an i8 (0 or 1), we check either_special == 0
    builder.ins().icmp_imm(IntCC::Equal, either_special, 0)
}

/// Check whether a NaN-boxed value carries the int tag.
#[cfg(feature = "native-backend")]
fn is_nanboxed_int(builder: &mut FunctionBuilder, val: Value, nbc: &NanBoxConsts) -> Value {
    let shift48 = builder.ins().iconst(types::I64, nbc.shift_48);
    let tag16 = builder.ins().ushr(val, shift48);
    let expected = builder.ins().iconst(types::I64, nbc.int_tag_16);
    builder.ins().icmp(IntCC::Equal, tag16, expected)
}

/// Emit inline mixed int+float arithmetic.  When exactly one operand is a
/// NaN-boxed int and the other is a plain f64, convert the int to f64 via
/// `fcvt_from_sint` and perform the requested float operation inline.
///
/// `f_op`: 0 = fadd, 1 = fsub, 2 = fmul.
#[cfg(feature = "native-backend")]
pub(crate) fn emit_mixed_int_float_op(
    builder: &mut FunctionBuilder,
    lhs: Value,
    rhs: Value,
    nbc: &NanBoxConsts,
    f_op: u8,
    merge_block: Block,
) {
    let lhs_is_int = is_nanboxed_int(builder, lhs, nbc);
    let rhs_is_int = is_nanboxed_int(builder, rhs, nbc);
    let lhs_special = is_nanboxed_special(builder, lhs, nbc);
    let rhs_special = is_nanboxed_special(builder, rhs, nbc);
    let rhs_not_special = builder.ins().icmp_imm(IntCC::Equal, rhs_special, 0);
    let lhs_not_special = builder.ins().icmp_imm(IntCC::Equal, lhs_special, 0);
    let case_a = builder.ins().band(lhs_is_int, rhs_not_special);
    let case_b = builder.ins().band(rhs_is_int, lhs_not_special);
    let lhs_int_block = builder.create_block();
    let check_rhs_block = builder.create_block();
    let rhs_int_block = builder.create_block();
    let not_mixed_block = builder.create_block();
    builder.set_cold_block(not_mixed_block);
    builder
        .ins()
        .brif(case_a, lhs_int_block, &[], check_rhs_block, &[]);
    // LHS is int, RHS is float
    builder.switch_to_block(lhs_int_block);
    builder.seal_block(lhs_int_block);
    let lhs_int_val = unbox_int(builder, lhs, nbc);
    let lhs_conv = builder.ins().fcvt_from_sint(types::F64, lhs_int_val);
    let rhs_flt = builder.ins().bitcast(types::F64, MemFlagsData::new(), rhs);
    let res_a = match f_op {
        0 => builder.ins().fadd(lhs_conv, rhs_flt),
        1 => builder.ins().fsub(lhs_conv, rhs_flt),
        2 => builder.ins().fmul(lhs_conv, rhs_flt),
        _ => unreachable!(),
    };
    let boxed_a = box_float_value(builder, res_a, nbc);
    jump_block(builder, merge_block, &[boxed_a]);
    // Check case_b
    builder.switch_to_block(check_rhs_block);
    builder.seal_block(check_rhs_block);
    builder
        .ins()
        .brif(case_b, rhs_int_block, &[], not_mixed_block, &[]);
    // RHS is int, LHS is float
    builder.switch_to_block(rhs_int_block);
    builder.seal_block(rhs_int_block);
    let rhs_int_val = unbox_int(builder, rhs, nbc);
    let rhs_conv = builder.ins().fcvt_from_sint(types::F64, rhs_int_val);
    let lhs_flt = builder.ins().bitcast(types::F64, MemFlagsData::new(), lhs);
    let res_b = match f_op {
        0 => builder.ins().fadd(lhs_flt, rhs_conv),
        1 => builder.ins().fsub(lhs_flt, rhs_conv),
        2 => builder.ins().fmul(lhs_flt, rhs_conv),
        _ => unreachable!(),
    };
    let boxed_b = box_float_value(builder, res_b, nbc);
    jump_block(builder, merge_block, &[boxed_b]);
    // Not mixed: caller emits slow path
    builder.switch_to_block(not_mixed_block);
    builder.seal_block(not_mixed_block);
}

#[cfg(feature = "native-backend")]
pub(crate) fn box_int_value(
    builder: &mut FunctionBuilder,
    val: Value,
    nbc: &NanBoxConsts,
) -> Value {
    let mask = builder.ins().iconst(types::I64, nbc.int_mask);
    let masked = builder.ins().band(val, mask);
    let tag = builder.ins().iconst(types::I64, nbc.qnan_tag_int);
    builder.ins().bor(tag, masked)
}

#[cfg(feature = "native-backend")]
pub(crate) fn box_float_value(
    builder: &mut FunctionBuilder,
    val: Value,
    nbc: &NanBoxConsts,
) -> Value {
    // Canonicalize NaN: if the f64 value is NaN, replace with CANONICAL_NAN_BITS
    // to avoid collision with the QNAN tag prefix used by NaN-boxing.
    let raw_bits = builder.ins().bitcast(types::I64, MemFlagsData::new(), val);
    let is_nan = builder.ins().fcmp(FloatCC::Unordered, val, val);
    let canonical = builder.ins().iconst(types::I64, nbc.canonical_nan);
    builder.ins().select(is_nan, canonical, raw_bits)
}

#[cfg(feature = "native-backend")]
pub(crate) fn int_value_fits_inline(builder: &mut FunctionBuilder, val: Value) -> Value {
    // Inline ints are 47-bit signed payloads: range [-(1<<46), (1<<46)-1].
    // Bias the value by +2^46 so the valid range maps to [0, 2^47-1],
    // then do a single unsigned comparison against 2^47.
    // This is a single-comparison range check that Cranelift cannot fold away.
    let bias = builder.ins().iconst(types::I64, 1_i64 << 46);
    let biased = builder.ins().iadd(val, bias);
    let limit = builder.ins().iconst(types::I64, 1_i64 << 47);
    builder.ins().icmp(IntCC::UnsignedLessThan, biased, limit)
}

/// Raw pieces of a signed 64-bit multiply with the `smulhi` overflow witness
/// (Cranelift 0.131 has NO `smul_overflow`, unlike `sadd_overflow`).
///
/// Returns `(product, hi, sign)`:
///   * `product` — the low 64 bits of `lhs * rhs` (the wrapping result),
///   * `hi`      — the upper 64 bits of the signed 128-bit product (`smulhi`),
///   * `sign`    — the arithmetic sign-extension of `product` (`product >> 63`).
///
/// The signed 64-bit multiply overflows iff `hi != sign`. Both
/// [`imul_overflow64`] and [`imul_checked_inline`] derive their boolean flag
/// from this single source of truth (no duplicated `smulhi` pattern), each
/// forming its own polarity with a direct `icmp` so the result stays a clean
/// `I8` 0/1 (Cranelift folded booleans into `I8`, so a `bnot` would yield
/// `0xFE` and silently corrupt a downstream `band`).
#[cfg(feature = "native-backend")]
fn imul_smulhi_pieces(
    builder: &mut FunctionBuilder,
    lhs: Value,
    rhs: Value,
) -> (Value, Value, Value) {
    let prod = builder.ins().imul(lhs, rhs);
    // smulhi gives the upper 64 bits of the signed 128-bit product.
    let hi = builder.ins().smulhi(lhs, rhs);
    // If there was no 64-bit overflow, hi must be the sign-extension of prod,
    // i.e. hi == prod >> 63 (arithmetic).
    let sixty_three = builder.ins().iconst(types::I64, 63);
    let sign = builder.ins().sshr(prod, sixty_three);
    (prod, hi, sign)
}

/// Perform `imul` with hardware-exact 64-bit signed-overflow detection via the
/// `smulhi` pattern.
///
/// Returns `(product, overflow64)` where `product` is the low 64 bits of the
/// signed multiplication and `overflow64` is an `I8` boolean Value that is
/// **true iff the signed product overflowed i64** — i.e. the full 128-bit
/// product does not fit in 64 bits. The flag polarity matches Cranelift's
/// `sadd_overflow` second result (true = overflowed), so the `checked_mul`
/// lowering mirrors the `checked_add` `(sum, of)` shape exactly.
///
/// This is a FULL 64-bit-exact overflow flag, NOT a 47-bit-inline-window test:
/// the overflow-peel accumulator is a genuine full-width i64 carrier, so it
/// must deopt to the boxed BigInt slow loop only at the true 2^63 boundary.
/// (Reusing the `fits_47`-ANDing `imul_checked_inline` here would deopt the
/// accumulator 2^16× too early — a perf bug, not a correctness bug.)
#[cfg(feature = "native-backend")]
pub(crate) fn imul_overflow64(
    builder: &mut FunctionBuilder,
    lhs: Value,
    rhs: Value,
) -> (Value, Value) {
    let (prod, hi, sign) = imul_smulhi_pieces(builder, lhs, rhs);
    // Overflow iff the high half differs from the low half's sign-extension.
    let overflow64 = builder.ins().icmp(IntCC::NotEqual, hi, sign);
    (prod, overflow64)
}

/// Perform `imul` with 64-bit overflow detection via `smulhi`.
///
/// Two 47-bit signed values can produce a product exceeding 64 bits (up to ~93
/// bits).  Plain `imul` silently wraps at 64 bits, and the truncated result may
/// happen to pass `int_value_fits_inline` even though it is wrong.
///
/// Returns `(product, fits)` where `product` is the low 64 bits of the
/// multiplication and `fits` is a boolean Value that is true only when:
///   1. The full 128-bit product equals the 64-bit `imul` result (no 64-bit
///      overflow), AND
///   2. The 64-bit result fits in a 47-bit signed inline integer.
///
/// Shares the `smulhi` pattern with [`imul_overflow64`] via
/// [`imul_smulhi_pieces`] (single source of truth); this variant additionally
/// ANDs in the 47-bit inline-window test, which the full-range `checked_mul`
/// carrier must NOT do.
#[cfg(feature = "native-backend")]
pub(crate) fn imul_checked_inline(
    builder: &mut FunctionBuilder,
    lhs: Value,
    rhs: Value,
) -> (Value, Value) {
    let (prod, hi, sign) = imul_smulhi_pieces(builder, lhs, rhs);
    // No 64-bit overflow iff hi == prod's sign-extension (direct icmp keeps the
    // result a clean I8 0/1 for the band below).
    let no_overflow_64 = builder.ins().icmp(IntCC::Equal, hi, sign);
    // Also check the result fits in 47-bit signed payload.
    let fits_47 = int_value_fits_inline(builder, prod);
    let both_ok = builder.ins().band(no_overflow_64, fits_47);
    (prod, both_ok)
}

#[cfg(feature = "native-backend")]
pub(crate) fn box_bool_value(
    builder: &mut FunctionBuilder,
    val: Value,
    nbc: &NanBoxConsts,
) -> Value {
    let one = builder.ins().iconst(types::I64, 1);
    let zero = builder.ins().iconst(types::I64, 0);
    let bool_val = builder.ins().select(val, one, zero);
    let tag = builder.ins().iconst(types::I64, nbc.qnan_tag_bool);
    builder.ins().bor(tag, bool_val)
}

#[cfg(feature = "native-backend")]
pub(crate) fn unbox_ptr_value(
    builder: &mut FunctionBuilder,
    val: Value,
    nbc: &NanBoxConsts,
) -> Value {
    let mask = builder.ins().iconst(types::I64, nbc.pointer_mask);
    let masked = builder.ins().band(val, mask);
    let shift = builder.ins().iconst(types::I64, nbc.shift_16);
    let shifted = builder.ins().ishl(masked, shift);
    builder.ins().sshr(shifted, shift)
}

#[cfg(feature = "native-backend")]
pub(crate) fn box_ptr_value(
    builder: &mut FunctionBuilder,
    val: Value,
    nbc: &NanBoxConsts,
) -> Value {
    let mask = builder.ins().iconst(types::I64, nbc.pointer_mask);
    let masked = builder.ins().band(val, mask);
    let tag = builder.ins().iconst(types::I64, nbc.qnan_tag_ptr);
    builder.ins().bor(tag, masked)
}

/// Fully inline list_int bounds check — zero FFI calls.
///
/// Extracts the raw heap pointer from the NaN-boxed list value, then
/// dereferences the object layout directly:
///
///   obj_ptr  = unbox_ptr(list_bits)   // past MoltHeader
///   vec_ptr  = *(obj_ptr as *const *const Vec<i64>)   // offset 0
///   data_ptr = *(vec_ptr + 0)         // Vec::ptr  (offset 0)
///   len      = *(vec_ptr + 8)         // Vec::len  (offset 8)
///
/// Returns (data_ptr, in_bounds) — the caller must branch on in_bounds
/// BEFORE loading/storing the element.
#[cfg(feature = "native-backend")]
#[allow(dead_code)]
fn emit_list_int_bounds_check(
    builder: &mut FunctionBuilder,
    list_bits: Value,
    index_raw: Value,
    _nbc: &NanBoxConsts,
) -> (Value, Value) {
    // Step 1: extract raw pointer from NaN-boxed value.
    //
    // The NaN-boxed pointer layout is: QNAN | TAG_PTR | (addr & POINTER_MASK).
    // To extract the address: mask off the top 16 bits (QNAN+tag), then
    // sign-extend from bit 47 to reconstruct canonical aarch64 addresses.
    //
    // Use _imm variants to avoid introducing SSA variable dependencies that
    // could interact with Cranelift's block sealing in complex control flow.
    let masked = builder.ins().band_imm(list_bits, POINTER_MASK as i64);
    // Sign-extend from bit 47: shift left 16, arithmetic shift right 16.
    let shifted = builder.ins().ishl_imm(masked, 16);
    let obj_ptr = builder.ins().sshr_imm(shifted, 16);
    // Step 2: load *mut Vec<i64> from offset 0 of the object payload
    let vec_ptr = builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), obj_ptr, 0);
    // Step 3: load data pointer from Vec (offset 0) and length (offset 8)
    let data_ptr = builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), vec_ptr, 0);
    let len = builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), vec_ptr, 8);
    // Step 4: unsigned compare index < length
    let in_bounds = builder.ins().icmp(IntCC::UnsignedLessThan, index_raw, len);
    (data_ptr, in_bounds)
}

/// Load element from list_int data pointer at given index.
/// MUST only be called after bounds check passes (i.e., inside the fast block).
#[cfg(feature = "native-backend")]
#[allow(dead_code)]
fn emit_list_int_load(
    builder: &mut FunctionBuilder,
    data_ptr: Value,
    index_raw: Value,
    nbc: &NanBoxConsts,
) -> Value {
    let offset = builder.ins().imul_imm(index_raw, 8);
    let elem_addr = builder.ins().iadd(data_ptr, offset);
    let raw_val = builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), elem_addr, 0);
    box_int_value(builder, raw_val, nbc)
}

/// Store element into list_int data pointer at given index.
/// MUST only be called after bounds check passes (i.e., inside the fast block).
#[cfg(feature = "native-backend")]
#[allow(dead_code)]
fn emit_list_int_store(
    builder: &mut FunctionBuilder,
    data_ptr: Value,
    index_raw: Value,
    value_raw: Value,
) {
    let offset = builder.ins().imul_imm(index_raw, 8);
    let elem_addr = builder.ins().iadd(data_ptr, offset);
    builder
        .ins()
        .store(MemFlagsData::trusted(), value_raw, elem_addr, 0);
}

#[allow(dead_code)]
#[cfg(feature = "native-backend")]
fn emit_maybe_ref_adjust(builder: &mut FunctionBuilder, val: Value, obj_ref_fn: FuncRef) {
    // Keep ref-adjust control flow linear. Hidden branch blocks here can invalidate
    // block-local tracked-value carry if callers do not explicitly propagate tracking.
    // The runtime ref helpers already no-op for non-pointer boxed values.
    let _ = builder.ins().call(obj_ref_fn, &[val]);
}

// ---------------------------------------------------------------------------
// Phase 1: Inline inc_ref_obj as Cranelift IR
//
// Eliminates function-call overhead for the hottest runtime operation (~73
// calls per compiled function). The inlined sequence:
//
//   1. Check if `val` is a heap pointer (NaN-boxed TAG_PTR).
//   2. Extract the raw data pointer from the NaN-box.
//   3. Load the flags field from MoltHeader; skip if IMMORTAL.
//   4. Load the 32-bit refcount, add 1, store back.
//
// Gated by MOLT_INLINE_RC=1 env var so we can A/B test vs call-based RC.
// dec_ref is left as a function call (needs the free/destructor path).
// ---------------------------------------------------------------------------

/// Returns `true` if inline RC codegen is enabled.
///
/// Re-enabled: the inline RC path now uses atomic_rmw (AtomicRmwOp::Add)
/// instead of non-atomic load/iadd/store, which is correct for the
/// AtomicU32 refcount field.
#[cfg(feature = "native-backend")]
fn inline_rc_enabled() -> bool {
    // Disabled: inline RC codegen (even single-branch) causes memory corruption
    // when inc_ref blocks fragment the control flow inside tuple_new. The root
    // cause is Cranelift's handling of SSA values across the brif boundary
    // between the inc_ref blocks and subsequent list_builder_append calls.
    // The function-call path (molt_inc_ref_obj) is both correct and fast
    // enough — it matches Swift's ARC pattern of opaque retain/release calls.
    false
}

/// Emit an inlined `inc_ref_obj` as Cranelift IR.
///
/// Single-branch architecture: only one brif (is_ptr → inc, else → merge).
/// The immortal check uses branchless conditional select to compute the
/// increment delta (0 for immortal, 1 for mortal), avoiding the extra
/// block that caused the Cranelift block-fragmentation corruption bug.
///
/// Equivalent to:
/// ```text
/// if (val & (QNAN | TAG_MASK)) == (QNAN | TAG_PTR):
///     ptr = sign_extend(val & POINTER_MASK)
///     flags = *(ptr - 8) as u32
///     delta = ((flags & IMMORTAL) == 0) ? 1 : 0
///     atomic_add(*(ptr - 12), delta)  // no-op when delta=0
/// ```
#[cfg(feature = "native-backend")]
fn emit_inline_inc_ref_obj(builder: &mut FunctionBuilder, val: Value, nbc: &NanBoxConsts) {
    // Single-branch: only split on is_ptr to avoid block fragmentation.
    let inc_block = builder.create_block();
    let merge_block = builder.create_block();

    // 1. Check if val is a heap pointer: (val & (QNAN | TAG_MASK)) == (QNAN | TAG_PTR)
    let tag_check_mask = builder.ins().iconst(types::I64, nbc.qnan_tag_mask);
    let tag_bits = builder.ins().band(val, tag_check_mask);
    let ptr_tag = builder.ins().iconst(types::I64, nbc.qnan_tag_ptr);
    let is_ptr = builder.ins().icmp(IntCC::Equal, tag_bits, ptr_tag);
    builder.ins().brif(is_ptr, inc_block, &[], merge_block, &[]);

    // 2. Inc block: extract pointer, check immortal branchlessly, atomic inc
    builder.switch_to_block(inc_block);
    let raw_ptr = unbox_ptr_value(builder, val, nbc);

    // Load flags and compute delta branchlessly:
    // delta = (flags & IMMORTAL) == 0 ? 1 : 0
    let flags = builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        raw_ptr,
        HEADER_FLAGS_OFFSET,
    );
    let immortal_mask = builder
        .ins()
        .iconst(types::I32, HEADER_FLAG_IMMORTAL as i64);
    let immortal_bits = builder.ins().band(flags, immortal_mask);
    let zero_i32 = builder.ins().iconst(types::I32, 0);
    let is_mortal = builder.ins().icmp(IntCC::Equal, immortal_bits, zero_i32);
    let one_i32 = builder.ins().iconst(types::I32, 1);
    // Branchless: delta = select(is_mortal, 1, 0)
    let delta = builder.ins().select(is_mortal, one_i32, zero_i32);

    // Atomic add of delta (0 for immortal = no-op, 1 for mortal = inc)
    let rc_offset = builder
        .ins()
        .iconst(types::I64, HEADER_REFCOUNT_OFFSET as i64);
    let rc_addr = builder.ins().iadd(raw_ptr, rc_offset);
    builder.ins().atomic_rmw(
        types::I32,
        MemFlagsData::trusted(),
        AtomicRmwOp::Add,
        rc_addr,
        delta,
    );
    builder.ins().jump(merge_block, &[]);

    // 3. Merge
    builder.switch_to_block(merge_block);
    builder.seal_block(inc_block);
    builder.seal_block(merge_block);
}

/// Emit an inc_ref_obj — either inlined or as a function call depending on
/// the `MOLT_INLINE_RC` flag.
#[cfg(feature = "native-backend")]
pub(crate) fn emit_inc_ref_obj(
    builder: &mut FunctionBuilder,
    val: Value,
    call_ref: FuncRef,
    nbc: &NanBoxConsts,
) {
    if inline_rc_enabled() {
        emit_inline_inc_ref_obj(builder, val, nbc);
    } else {
        builder.ins().call(call_ref, &[val]);
    }
}

/// Emit a ref-adjust (inc_ref_obj) — either inlined or as a function call
/// depending on the `MOLT_INLINE_RC` flag.
#[cfg(feature = "native-backend")]
pub(crate) fn emit_maybe_ref_adjust_v2(
    builder: &mut FunctionBuilder,
    val: Value,
    call_ref: FuncRef,
    nbc: &NanBoxConsts,
) {
    if inline_rc_enabled() {
        emit_inline_inc_ref_obj(builder, val, nbc);
    } else {
        let _ = builder.ins().call(call_ref, &[val]);
    }
}

/// Emit a dec_ref_obj with an inlined tag check: if the value is not a heap
/// pointer (e.g. NaN-boxed int/float/bool/none), skip the dec_ref call
/// entirely. This eliminates function-call + GIL overhead for the common case
/// where cleanup values are immediate integers.
#[cfg(feature = "native-backend")]
#[allow(dead_code)]
pub(crate) fn emit_dec_ref_obj(
    builder: &mut FunctionBuilder,
    val: Value,
    call_ref: FuncRef,
    nbc: &NanBoxConsts,
) {
    if !inline_rc_enabled() {
        builder.ins().call(call_ref, &[val]);
        return;
    }
    // Inline tag check: (val & (QNAN | TAG_MASK)) == (QNAN | TAG_PTR)
    let call_block = builder.create_block();
    let merge_block = builder.create_block();

    let tag_check_mask = builder.ins().iconst(types::I64, nbc.qnan_tag_mask);
    let tag_bits = builder.ins().band(val, tag_check_mask);
    let ptr_tag = builder.ins().iconst(types::I64, nbc.qnan_tag_ptr);
    let is_ptr = builder.ins().icmp(IntCC::Equal, tag_bits, ptr_tag);
    brif_block(builder, is_ptr, call_block, &[], merge_block, &[]);

    // Only call dec_ref_obj for actual heap pointers.
    builder.switch_to_block(call_block);
    builder.ins().call(call_ref, &[val]);
    jump_block(builder, merge_block, &[]);

    builder.switch_to_block(merge_block);
    builder.seal_block(call_block);
    builder.seal_block(merge_block);
}

#[derive(Clone, Copy)]
#[cfg(feature = "native-backend")]
pub(crate) struct VarValue(pub(crate) Value);

#[cfg(feature = "native-backend")]
impl std::ops::Deref for VarValue {
    type Target = Value;

    fn deref(&self) -> &Value {
        &self.0
    }
}

#[cfg(feature = "native-backend")]
pub(crate) fn var_get(
    builder: &mut FunctionBuilder,
    vars: &BTreeMap<String, Variable>,
    name: &str,
) -> Option<VarValue> {
    vars.get(name).map(|var| VarValue(builder.use_var(*var)))
}

/// Get raw value directly (for proven-type consumers that don't need NaN-box).
#[cfg(feature = "native-backend")]
#[allow(dead_code)]
fn var_get_raw(
    builder: &mut FunctionBuilder,
    vars: &BTreeMap<String, Variable>,
    name: &str,
    int_primary_vars: &std::collections::BTreeSet<String>,
    raw_primary_float: &std::collections::BTreeSet<String>,
) -> Option<(VarValue, bool)> {
    let var = *vars.get(name)?;
    let val = builder.use_var(var);
    let is_raw = int_primary_vars.contains(name) || raw_primary_float.contains(name);
    Some((VarValue(val), is_raw))
}

/// Store a raw value as the primary representation for a proven-type variable.
#[cfg(feature = "native-backend")]
#[allow(dead_code)]
fn def_var_raw(
    builder: &mut FunctionBuilder,
    vars: &BTreeMap<String, Variable>,
    name: impl AsRef<str>,
    raw_val: Value,
    int_primary_vars: &mut std::collections::BTreeSet<String>,
) {
    let name_ref = name.as_ref();
    if name_ref == "none" {
        return;
    }
    let var = *vars
        .get(name_ref)
        .unwrap_or_else(|| panic!("Var not found: {name_ref}"));
    builder.def_var(var, raw_val);
    int_primary_vars.insert(name_ref.to_string());
}

#[cfg(feature = "native-backend")]
pub(crate) fn def_var_named(
    builder: &mut FunctionBuilder,
    vars: &BTreeMap<String, Variable>,
    name: impl AsRef<str>,
    val: Value,
) {
    let name_ref = name.as_ref();
    if name_ref == "none" {
        return;
    }
    let var = *vars
        .get(name_ref)
        .unwrap_or_else(|| panic!("Var not found: {name_ref}"));
    if let Err(error) = builder.try_def_var(var, val) {
        let val_type = builder.func.dfg.value_type(val);
        panic!(
            "native variable representation mismatch for {name_ref}: value {val} has CLIF type {val_type}; {error}"
        );
    }
}

/// Seal a block only if it hasn't been sealed yet. Prevents the
/// `!self.is_sealed(block)` assertion panic in Cranelift's SSA builder
/// when multiple code paths attempt to seal the same block.
#[cfg(feature = "native-backend")]
#[inline]
pub(crate) fn seal_block_once(
    builder: &mut FunctionBuilder,
    sealed: &mut std::collections::BTreeSet<Block>,
    block: Block,
) {
    if sealed.insert(block) && builder.func.layout.is_block_inserted(block) {
        builder.seal_block(block);
    }
}

#[cfg(feature = "native-backend")]
pub(crate) fn jump_block(builder: &mut FunctionBuilder, target: Block, args: &[Value]) {
    let block_args: Vec<BlockArg> = args.iter().copied().map(BlockArg::from).collect();
    builder.ins().jump(target, &block_args);
}

#[cfg(feature = "native-backend")]
pub(crate) fn brif_block(
    builder: &mut FunctionBuilder,
    cond: Value,
    then_block: Block,
    then_args: &[Value],
    else_block: Block,
    else_args: &[Value],
) {
    let then_block_args: Vec<BlockArg> = then_args.iter().copied().map(BlockArg::from).collect();
    let else_block_args: Vec<BlockArg> = else_args.iter().copied().map(BlockArg::from).collect();
    builder.ins().brif(
        cond,
        then_block,
        &then_block_args,
        else_block,
        &else_block_args,
    );
}

#[cfg(feature = "native-backend")]
#[allow(dead_code)]
fn parse_inst_id(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i + 4 <= bytes.len() {
        if bytes[i..].starts_with(b"inst") {
            let mut j = i + 4;
            let mut value: usize = 0;
            let mut found = false;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                found = true;
                value = value * 10 + (bytes[j] - b'0') as usize;
                j += 1;
            }
            if found {
                return Some(value);
            }
        }
        i += 1;
    }
    None
}

#[cfg(feature = "native-backend")]
pub(crate) struct TraceOpsConfig {
    pub(crate) stride: usize,
}

#[cfg(feature = "native-backend")]
pub(crate) fn should_trace_ops(func_name: &str) -> Option<TraceOpsConfig> {
    static RAW: OnceLock<Option<String>> = OnceLock::new();
    let raw = RAW
        .get_or_init(|| {
            std::env::var("MOLT_TRACE_OP_PROGRESS")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .as_ref()?;
    let (filter_part, stride_part) = match raw.split_once(':') {
        Some((left, right)) => (left.trim(), Some(right.trim())),
        None => (raw.as_str(), None),
    };
    let stride = stride_part
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(5_000);
    let matches = filter_part == "1"
        || filter_part.eq_ignore_ascii_case("all")
        || func_name == filter_part
        || func_name.contains(filter_part);
    if matches {
        Some(TraceOpsConfig { stride })
    } else {
        None
    }
}

#[cfg(feature = "native-backend")]
#[allow(dead_code)]
fn collect_cleanup_tracked(
    names: &[String],
    last_use: &BTreeMap<String, usize>,
    op_idx: usize,
    skip: Option<&str>,
) -> Vec<String> {
    names
        .iter()
        .filter(|name| skip != Some(name.as_str()))
        .filter(|name| last_use.get(*name).copied().unwrap_or(op_idx) <= op_idx)
        .cloned()
        .collect()
}

#[cfg(feature = "native-backend")]
pub(crate) fn extend_unique_tracked(dst: &mut Vec<String>, src: Vec<String>) {
    if src.is_empty() {
        return;
    }
    if dst.is_empty() {
        dst.extend(src);
        return;
    }
    // Dedup by `name` so multi-predecessor merges don't create double-decref hazards.
    let mut seen: BTreeSet<String> = dst.iter().cloned().collect();
    for name in src {
        if seen.insert(name.clone()) {
            dst.push(name);
        }
    }
}

/// Propagate tracked objects to ALL branch target blocks.
/// Prevents use-after-free when exception handlers access freed objects.
#[cfg(feature = "native-backend")]
pub(crate) fn propagate_tracked_to_branches(
    block_tracked: &mut BTreeMap<cranelift_codegen::ir::Block, Vec<String>>,
    targets: &[cranelift_codegen::ir::Block],
    carry: Vec<String>,
) {
    if carry.is_empty() || targets.is_empty() {
        return;
    }
    if targets.len() == 1 {
        extend_unique_tracked(block_tracked.entry(targets[0]).or_default(), carry);
        return;
    }
    let last_idx = targets.len() - 1;
    for (i, &target) in targets.iter().enumerate() {
        if i == last_idx {
            extend_unique_tracked(block_tracked.entry(target).or_default(), carry);
            return;
        }
        extend_unique_tracked(block_tracked.entry(target).or_default(), carry.clone());
    }
}

#[cfg(feature = "native-backend")]
pub(crate) fn drain_cleanup_tracked_dedup(
    names: &mut Vec<String>,
    last_use: &BTreeMap<String, usize>,
    alias_roots: &BTreeMap<String, String>,
    op_idx: usize,
    skip: Option<&str>,
    already_decrefed: Option<&mut BTreeSet<String>>,
) -> Vec<String> {
    drain_cleanup_tracked_dedup_with_budget(
        names,
        last_use,
        alias_roots,
        op_idx,
        skip,
        already_decrefed,
        None,
    )
}

#[cfg(feature = "native-backend")]
pub(crate) fn drain_cleanup_tracked_dedup_with_authority(
    native_rc_tracking_enabled: bool,
    names: &mut Vec<String>,
    last_use: &BTreeMap<String, usize>,
    alias_roots: &BTreeMap<String, String>,
    op_idx: usize,
    skip: Option<&str>,
    already_decrefed: Option<&mut BTreeSet<String>>,
) -> Vec<String> {
    if !native_rc_tracking_enabled {
        names.clear();
        return Vec::new();
    }
    drain_cleanup_tracked_dedup(names, last_use, alias_roots, op_idx, skip, already_decrefed)
}

#[cfg(feature = "native-backend")]
pub(crate) fn drain_cleanup_tracked_dedup_with_budget(
    names: &mut Vec<String>,
    last_use: &BTreeMap<String, usize>,
    alias_roots: &BTreeMap<String, String>,
    op_idx: usize,
    skip: Option<&str>,
    mut already_decrefed: Option<&mut BTreeSet<String>>,
    mut retain_release_budget: Option<&mut BTreeMap<String, usize>>,
) -> Vec<String> {
    let mut cleanup = Vec::new();
    names.retain(|name| {
        if skip == Some(name.as_str()) {
            return true;
        }
        let cleanup_key = alias_roots
            .get(name)
            .map(String::as_str)
            .unwrap_or(name.as_str());
        if let Some(ref mut set) = already_decrefed
            && set.contains(cleanup_key)
        {
            let budget_allows = if let Some(ref mut budget) = retain_release_budget {
                if let Some(extra) = budget.get_mut(cleanup_key) {
                    if *extra > 0 {
                        *extra -= 1;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            } else {
                false
            };
            if !budget_allows {
                return false;
            }
        }
        let last = last_use.get(name).copied().unwrap_or(usize::MAX);
        if last <= op_idx {
            if let Some(ref mut set) = already_decrefed
                && !set.contains(cleanup_key)
            {
                set.insert(cleanup_key.to_string());
            }
            cleanup.push(name.clone());
            return false;
        }
        true
    });
    cleanup
}

#[cfg(feature = "native-backend")]
pub(crate) fn drain_cleanup_entry_tracked_with_authority(
    native_rc_tracking_enabled: bool,
    names: &mut Vec<String>,
    entry_vars: &mut BTreeMap<String, Value>,
    last_use: &BTreeMap<String, usize>,
    alias_roots: &BTreeMap<String, String>,
    already_decrefed: &mut BTreeSet<String>,
    op_idx: usize,
    skip: Option<&str>,
) -> Vec<Value> {
    if !native_rc_tracking_enabled {
        for name in names.drain(..) {
            entry_vars.remove(&name);
        }
        return Vec::new();
    }
    drain_cleanup_entry_tracked(
        names,
        entry_vars,
        last_use,
        alias_roots,
        already_decrefed,
        op_idx,
        skip,
    )
}

#[cfg(feature = "native-backend")]
pub(crate) fn drain_cleanup_entry_tracked(
    names: &mut Vec<String>,
    entry_vars: &mut BTreeMap<String, Value>,
    last_use: &BTreeMap<String, usize>,
    alias_roots: &BTreeMap<String, String>,
    already_decrefed: &mut BTreeSet<String>,
    op_idx: usize,
    skip: Option<&str>,
) -> Vec<Value> {
    let mut cleanup = Vec::new();
    let mut to_remove = Vec::new();
    names.retain(|name| {
        if skip == Some(name.as_str()) {
            return true;
        }
        // If not in last_use, default to MAX (keep alive) — NOT op_idx.
        // Using op_idx as default causes premature cleanup of variables
        // that are used later but not yet tracked in last_use.
        let last = last_use.get(name).copied().unwrap_or(usize::MAX);
        if last <= op_idx {
            let cleanup_key = alias_roots
                .get(name)
                .map(String::as_str)
                .unwrap_or(name.as_str());
            if already_decrefed.contains(cleanup_key) {
                to_remove.push(name.clone());
                return false;
            }
            if let Some(val) = entry_vars.get(name) {
                cleanup.push(*val);
            }
            already_decrefed.insert(cleanup_key.to_string());
            // Mark for removal from entry_vars so no other cleanup path
            // (exception handler, finalize block) can double dec-ref.
            to_remove.push(name.clone());
            return false;
        }
        true
    });
    for name in to_remove {
        entry_vars.remove(&name);
    }
    cleanup
}

// ---------------------------------------------------------------------------
// RC coalescing: eliminate redundant inc_ref / dec_ref pairs.
// ---------------------------------------------------------------------------

#[cfg(feature = "native-backend")]
#[allow(dead_code)]
const CONTROL_FLOW_OPS: &[&str] = &[
    "if",
    "else",
    "end_if",
    "loop_start",
    "loop_end",
    "loop_for_start",
    "loop_for_end",
    "label",
    "state_label",
    "jump",
    "return",
    "state_yield",
    "check_exception",
    "raise",
];

#[cfg(feature = "native-backend")]
#[allow(dead_code)]
pub(crate) fn compute_rc_coalesce_skips(
    ops: &[OpIR],
    last_use: &BTreeMap<String, usize>,
) -> (HashSet<usize>, HashSet<String>) {
    let cf_set: HashSet<&str> = CONTROL_FLOW_OPS.iter().copied().collect();
    let mut skip_ops: HashSet<usize> = HashSet::new();
    let mut skip_dec_ref: HashSet<String> = HashSet::new();

    for i in 0..ops.len() {
        if skip_ops.contains(&i) {
            continue;
        }
        let a = &ops[i];
        let a_is_inc = matches!(a.kind.as_str(), "inc_ref" | "borrow");
        let a_is_dec = matches!(a.kind.as_str(), "dec_ref" | "release");
        if !a_is_inc && !a_is_dec {
            continue;
        }
        let a_arg = match a.args.as_ref().and_then(|v| v.first()) {
            Some(name) => name.clone(),
            None => continue,
        };
        for j in (i + 1)..ops.len() {
            let b = &ops[j];
            if cf_set.contains(b.kind.as_str()) {
                break;
            }
            let b_kind = b.kind.as_str();
            let b_arg = b.args.as_ref().and_then(|v| v.first());
            let is_match = if a_is_inc {
                matches!(b_kind, "dec_ref" | "release") && b_arg.map(String::as_str) == Some(&a_arg)
            } else {
                matches!(b_kind, "inc_ref" | "borrow") && b_arg.map(String::as_str) == Some(&a_arg)
            };
            if is_match && !skip_ops.contains(&j) {
                skip_ops.insert(i);
                skip_ops.insert(j);
                break;
            }
            let uses_var = b
                .args
                .as_ref()
                .map(|args| args.iter().any(|n| n == &a_arg))
                .unwrap_or(false)
                || b.var.as_ref().map(|v| v == &a_arg).unwrap_or(false)
                || b.out.as_ref().map(|o| o == &a_arg).unwrap_or(false);
            if uses_var {
                break;
            }
        }
    }

    for (idx, op) in ops.iter().enumerate() {
        if skip_ops.contains(&idx) {
            continue;
        }
        if !matches!(op.kind.as_str(), "inc_ref" | "borrow") {
            continue;
        }
        let out_name = match op.out.as_deref() {
            Some(name) if name != "none" => name,
            _ => continue,
        };
        let last = last_use.get(out_name).copied().unwrap_or(idx);
        if last <= idx {
            skip_ops.insert(idx);
            skip_dec_ref.insert(out_name.to_string());
        }
    }

    if !skip_ops.is_empty() || !skip_dec_ref.is_empty() {
        static RC_COALESCE_TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let trace = *RC_COALESCE_TRACE
            .get_or_init(|| std::env::var("MOLT_RC_COALESCE_TRACE").as_deref() == Ok("1"));
        if trace {
            eprintln!(
                "[rc-coalesce] eliminated {} RC ops, {} dec_ref skips",
                skip_ops.len(),
                skip_dec_ref.len()
            );
        }
    }

    (skip_ops, skip_dec_ref)
}

/// Output of a native compilation pass.
///
/// Separating bytes from diagnostics lets callers handle warnings
/// structurally instead of parsing stderr.  The design follows
/// Lattner's principle: compilation is a pure function from IR to
/// (artifact, diagnostics) — side effects are the caller's concern.
#[cfg(feature = "native-backend")]
pub struct CompileOutput {
    /// The compiled object file bytes.
    pub bytes: Vec<u8>,
}

#[cfg(feature = "native-backend")]
pub struct SimpleBackend {
    pub(crate) module: ObjectModule,
    pub(crate) ctx: Context,
    // DETERMINISM: BTreeMap ensures iteration order is independent of hash seed
    pub(crate) trampoline_ids: BTreeMap<TrampolineKey, cranelift_module::FuncId>,
    pub(crate) import_ids: BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    pub skip_ir_passes: bool,
    pub skip_shared_stdlib_partition: bool,
    /// Whether this object emits the per-app `molt_app_resolve_intrinsic` resolver.
    /// Exactly one object per final binary must emit it (the one main_stub.c
    /// registers): the main application object. Stdlib-cache batch objects and all
    /// but one program batch set this `false` to avoid a duplicate symbol.
    pub emit_app_intrinsic_resolver: bool,
    /// Pre-computed per-app intrinsic manifest (the intrinsics reached by the
    /// dynamic name-based resolver path). Set by the orchestrator when the full
    /// function set is split across objects (stdlib cache split / batching) so the
    /// resolver covers names whose defining functions live in another object. When
    /// `None`, `compile` derives it from this object's own `ir.functions` (the
    /// single-object, non-split case where `ir` already holds the full set).
    pub app_intrinsic_manifest: Option<std::collections::BTreeSet<String>>,
    /// Function names that exist in other batches — use Linkage::Import.
    pub external_function_names: std::collections::BTreeSet<String>,
    module_context: Option<NativeBackendModuleContext>,
    // DETERMINISM: BTreeMap ensures iteration order is independent of hash seed
    pub(crate) data_pool: BTreeMap<Vec<u8>, cranelift_module::DataId>,
    pub(crate) next_data_id: u64,
    // Track the arity each user-defined function was declared with so that
    // call sites that reference the same function (potentially with a
    // different number of actual arguments, e.g. kwargs expansion) can
    // construct a matching Cranelift signature for `declare_function`.
    pub(crate) declared_func_arities: BTreeMap<String, usize>,
    /// Track which functions have been given a body (defined), so we can fail
    /// closed if any exported declaration is left without codegen.
    pub(crate) defined_func_names: std::collections::BTreeSet<String>,
    /// Deferred Cranelift function definitions for parallel compilation.
    /// Instead of compiling each function immediately in `define_function`,
    /// we collect the finalized IR here and compile them all in parallel
    /// via `flush_deferred_defines()`.
    pub(crate) deferred_defines: Vec<DeferredDefine>,
}

#[cfg(feature = "native-backend")]
pub(crate) struct DeferredDefine {
    pub(crate) func_id: cranelift_module::FuncId,
    pub(crate) func: cranelift_codegen::ir::Function,
    pub(crate) name: String,
}

#[cfg(feature = "native-backend")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MergeRebindStorageKind {
    BoxedI64,
    RawI64,
    RawBool,
    RawF64,
}

#[cfg(feature = "native-backend")]
#[derive(Clone, Copy, Debug)]
pub(crate) struct MergeRebindSlot {
    pub(crate) slot: cranelift_codegen::ir::StackSlot,
    pub(crate) storage: MergeRebindStorageKind,
}

#[cfg(feature = "native-backend")]
pub(crate) struct IfFrame {
    pub(crate) else_block: Option<Block>,
    pub(crate) merge_block: Block,
    pub(crate) has_else: bool,
    pub(crate) then_terminal: bool,
    pub(crate) else_terminal: bool,
    pub(crate) phi_ops: Vec<(String, String, String)>,
    pub(crate) phi_params: Vec<Value>,
    pub(crate) merge_rebind_names: Vec<String>,
    pub(crate) merge_rebind_params: Vec<Value>,
    pub(crate) merge_rebind_slots: Vec<MergeRebindSlot>,
}

#[cfg(feature = "native-backend")]
pub(crate) struct LoopFrame {
    pub(crate) loop_block: Block,
    pub(crate) body_block: Block,
    pub(crate) after_block: Block,
    pub(crate) index_name: Option<String>,
    pub(crate) next_index: Option<Value>,
    /// True when the loop uses the linearized TIR path (no dedicated
    /// Cranelift loop block; counter flows through phi variables).
    /// `loop_end` must NOT decrement `loop_depth` for linearized loops
    /// because `loop_index_start` did not increment it.
    pub(crate) linearized: bool,
}

#[cfg(feature = "native-backend")]
pub(crate) fn parse_truthy_env(raw: &str) -> bool {
    let norm = raw.trim().to_ascii_lowercase();
    matches!(norm.as_str(), "1" | "true" | "yes" | "on")
}

#[cfg(feature = "native-backend")]
fn compute_function_has_ret(functions: &[FunctionIR]) -> BTreeMap<String, bool> {
    functions
        .iter()
        .map(|func| (func.name.clone(), function_requires_value_return(func)))
        .collect()
}

#[cfg(feature = "native-backend")]
fn merge_function_arities(
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
fn merge_function_has_ret(
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
/// SOUNDNESS — why this must be a union, not a replace (the bug class fixed in
/// design-20 finding #3C activation): `closure_functions` decides whether a
/// `call_guarded`/`call`/`call_internal` site extracts the closure env from the
/// callee function object and prepends it as arg 0 (`function_compiler.rs`
/// "extract env from function object"). It is keyed by the names that appear in
/// `func_new_closure(name)` ops. The module context is built ONCE per
/// compilation unit — for the stdlib cache it is built from the stdlib
/// functions ONLY (`main.rs` `stdlib_module_context`), so it does NOT contain a
/// user program's closures. When a batch that DEFINES a user closure is
/// compiled with that (stdlib) module context set, REPLACING the local scan
/// dropped the user closure from the set → the call site skipped env extraction
/// → the callee received a garbage/zero closure → `'object' object is not
/// subscriptable` when it indexed its cell tuple. The local scan ALWAYS knows
/// the closures defined in this batch; the module context adds cross-batch
/// knowledge. Both are required, exactly like `merge_function_arities` /
/// `merge_function_has_ret` already do for their maps. (This asymmetry was
/// latent until RC drop insertion shifted function sizes enough to change which
/// batch the user code landed in; the bug is the replace semantics, not the
/// drops.)
#[cfg(feature = "native-backend")]
fn merge_closure_functions(
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
fn merge_task_kinds(
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
fn merge_task_closure_sizes(
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
/// guard), never a miscompile — but the union keeps the fast path firing for the
/// current batch's own leaves, matching the other merged metadata.
#[cfg(feature = "native-backend")]
fn merge_leaf_functions(
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
fn backend_setting_requests_llvm(setting: Option<&str>) -> bool {
    setting == Some("llvm")
}

#[cfg(all(feature = "native-backend", not(feature = "llvm")))]
fn assert_requested_llvm_backend_available(use_llvm: bool) {
    if use_llvm {
        panic!(
            "MOLT_BACKEND=llvm requested but molt-backend was built without the llvm feature; rebuild with `--features llvm` or choose the Cranelift backend explicitly"
        );
    }
}

#[cfg(all(feature = "native-backend", feature = "llvm"))]
fn assert_requested_llvm_backend_available(_use_llvm: bool) {}

#[cfg(feature = "native-backend")]
fn emitted_module_symbol(name: &str) -> Option<&str> {
    name.strip_prefix("molt_init_")
}

#[cfg(feature = "native-backend")]
fn emitted_name_matches_module_symbol(name: &str, module_symbol: &str) -> bool {
    if let Some(rest) = name.strip_prefix("molt_init_") {
        return rest == module_symbol;
    }
    name.starts_with(&format!("{module_symbol}__"))
}

#[cfg(feature = "native-backend")]
fn is_user_owned_symbol(
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
fn prune_and_partition_native_stdlib(
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
/// Computed up front — BEFORE the module-phase inliner runs — so the inliner can
/// treat these as **external-linkage** functions and refuse to inline them: a
/// caller that does not own a callee's canonical definition (it lives in another
/// object) must not splice in a private copy of its body. Doing so would (a)
/// drop the external reference the linker resolves against `stdlib_shared.o`,
/// breaking the partition contract, and (b) — once `externalize_*` later clears
/// the in-app body — leave the app running a stale private fork of a function
/// whose real definition is the shared one.
///
/// Returns an empty set when the partition is inactive (no `MOLT_STDLIB_OBJ`, no
/// `MOLT_ENTRY_MODULE`, or the shared object file is absent), so the inliner is
/// unconstrained in the common (non-partitioned) build. This mirrors the exact
/// activation guards and `is_user_owned_symbol` predicate
/// `externalize_shared_stdlib_partition` uses, so the do-not-inline set and the
/// later externalized set are computed from one source of truth.
#[cfg(feature = "native-backend")]
fn shared_stdlib_external_symbols(ir: &SimpleIR) -> BTreeSet<String> {
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
fn externalize_shared_stdlib_partition(ir: &mut SimpleIR) {
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

#[cfg(feature = "native-backend")]
impl Default for SimpleBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "native-backend")]
impl SimpleBackend {
    fn cloned_shared_flags(
        flags: &settings::Flags,
        opt_level_override: Option<&str>,
    ) -> Result<settings::Flags, String> {
        let mut builder = settings::builder();
        for value in flags.iter() {
            let configured = if value.name == "opt_level" {
                opt_level_override
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.value_string())
            } else {
                value.value_string()
            };
            builder
                .set(value.name, &configured)
                .map_err(|err| format!("shared flag {}={configured:?}: {err}", value.name))?;
        }
        Ok(settings::Flags::new(builder))
    }

    fn rebuild_owned_isa(
        target_isa: &dyn isa::TargetIsa,
        opt_level_override: Option<&str>,
    ) -> Result<isa::OwnedTargetIsa, String> {
        let isa_builder = isa::Builder::from_target_isa(target_isa);
        let shared_flags = Self::cloned_shared_flags(target_isa.flags(), opt_level_override)?;
        isa_builder
            .finish(shared_flags)
            .map_err(|err| format!("TargetIsa finish: {err}"))
    }

    pub fn new() -> Self {
        Self::new_with_target(None)
    }

    pub fn new_with_target(target: Option<&str>) -> Self {
        let mut flag_builder = settings::builder();
        flag_builder.set("is_pic", "true").unwrap();
        // Cranelift optimization level: "none", "speed", or "speed_and_size".
        // Default to "speed" for production quality codegen.  Override with
        // MOLT_BACKEND_OPT_LEVEL=none for fast dev-loop compilation (~3-5x
        // faster compile times at the cost of ~30-50% slower generated code).
        let opt_level =
            env_setting("MOLT_BACKEND_OPT_LEVEL").unwrap_or_else(|| "speed".to_string());
        flag_builder
            .set("opt_level", &opt_level)
            .unwrap_or_else(|err| panic!("invalid MOLT_BACKEND_OPT_LEVEL={opt_level:?}: {err:?}"));
        let regalloc_algorithm =
            env_setting("MOLT_BACKEND_REGALLOC_ALGORITHM").unwrap_or_else(|| {
                // When opt_level=none, default to the fast single-pass
                // allocator regardless of build profile — the user has
                // explicitly asked for compile-time speed.
                if opt_level == "none" {
                    "single_pass".to_string()
                } else {
                    "backtracking".to_string()
                }
            });
        flag_builder
            .set("regalloc_algorithm", &regalloc_algorithm)
            .unwrap_or_else(|err| {
                panic!("invalid MOLT_BACKEND_REGALLOC_ALGORITHM={regalloc_algorithm:?}: {err:?}")
            });
        // Cranelift 0.128 adds explicit minimum function alignment tuning.
        // Default to 16-byte release alignment for better i-cache/branch
        // behavior on hot call-heavy kernels; keep debug/dev unchanged.
        let min_alignment_log2 = env_setting("MOLT_BACKEND_MIN_FUNCTION_ALIGNMENT_LOG2")
            .unwrap_or_else(|| {
                if cfg!(debug_assertions) {
                    "0".to_string()
                } else {
                    "4".to_string()
                }
            });
        flag_builder
            .set("log2_min_function_alignment", &min_alignment_log2)
            .unwrap_or_else(|err| {
                panic!(
                    "invalid MOLT_BACKEND_MIN_FUNCTION_ALIGNMENT_LOG2={min_alignment_log2:?}: {err:?}"
                )
            });
        if let Some(libcall_call_conv) = env_setting("MOLT_BACKEND_LIBCALL_CALL_CONV") {
            flag_builder
                .set("libcall_call_conv", &libcall_call_conv)
                .unwrap_or_else(|err| {
                    panic!("invalid MOLT_BACKEND_LIBCALL_CALL_CONV={libcall_call_conv:?}: {err:?}")
                });
        }
        // Cranelift verifier catches IR invariant violations (type mismatches,
        // dominator tree bugs). Enable in debug builds; disable in release for
        // speed. Override with MOLT_BACKEND_ENABLE_VERIFIER=0|1.
        let default_enable_verifier = cfg!(debug_assertions);
        let enable_verifier = env_setting("MOLT_BACKEND_ENABLE_VERIFIER")
            .as_deref()
            .map(parse_truthy_env)
            .unwrap_or(default_enable_verifier);
        flag_builder
            .set(
                "enable_verifier",
                if enable_verifier { "true" } else { "false" },
            )
            .unwrap();
        // Cranelift alias analysis: enables redundant-load elimination across
        // memory operations within a basic block. Safe for our codegen because
        // we never emit raw pointer aliasing between different object fields.
        flag_builder.set("enable_alias_analysis", "true").unwrap();
        // Emit CFG metadata in machine code output — enables downstream tools
        // and profilers to reconstruct control-flow graphs from compiled objects.
        flag_builder.set("machine_code_cfg_info", "true").unwrap();
        // Use colocated libcalls: our generated code and runtime libcalls live
        // in the same link unit — colocated calls skip GOT/PLT indirection and
        // use direct PC-relative calls instead.
        flag_builder.set("use_colocated_libcalls", "true").unwrap();
        // Detect whether we are targeting aarch64 — either because we are
        // compiling natively on aarch64, or because an explicit cross-compile
        // target triple was supplied that contains "aarch64".
        let targeting_aarch64 = match target {
            Some(t) => t.contains("aarch64"),
            None => cfg!(target_arch = "aarch64"),
        };
        // Frame pointers: always preserve on aarch64 to ensure correct stack
        // frame layout for large functions (>16KB frames).  Cranelift 0.128 can
        // generate incorrect SP-relative accesses on aarch64 when frame pointers
        // are omitted and the frame exceeds the immediate offset range, leading
        // to SIGTRAP (exit 133) in generated code.  On x86_64 the cost is one
        // register (rbp); on aarch64 x29 is conventionally reserved anyway.
        // Debug builds always preserve for profiler/debugger support.
        flag_builder
            .set(
                "preserve_frame_pointers",
                if cfg!(debug_assertions) || targeting_aarch64 {
                    "true"
                } else {
                    "false"
                },
            )
            .unwrap();
        // Spectre mitigations: Molt compiles trusted user code (not sandboxed
        // plugins), so Spectre v1 heap/table mitigations add unnecessary overhead.
        flag_builder
            .set("enable_heap_access_spectre_mitigation", "false")
            .unwrap();
        flag_builder
            .set("enable_table_access_spectre_mitigation", "false")
            .unwrap();
        // Stack probing: guard pages detect stack overflow in large/recursive
        // frames instead of silently segfaulting. Cranelift 0.131 does not
        // implement stack probing on AArch64, so enabling it there is a
        // compile-time backend panic. AArch64 keeps frame pointers above; stack
        // probing is enabled only where the selected Cranelift target supports it.
        flag_builder
            .set(
                "enable_probestack",
                if targeting_aarch64 { "false" } else { "true" },
            )
            .unwrap();
        // On x86_64, inline probes are safe and faster for deep recursion.
        // When probing is disabled the strategy setting is inert.
        flag_builder
            .set(
                "probestack_strategy",
                if targeting_aarch64 {
                    "outline"
                } else {
                    "inline"
                },
            )
            .unwrap();
        // MOLT_PORTABLE=1 forces baseline ISA (no host-specific features like AVX2).
        // This ensures reproducible codegen across different machines at the cost of
        // ~5-15% runtime performance on modern CPUs with advanced features.
        let portable = env_setting("MOLT_PORTABLE")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let mut isa_builder = if let Some(triple) = target {
            isa::lookup_by_name(triple).unwrap_or_else(|msg| {
                panic!("target {} is not supported: {}", triple, msg);
            })
        } else if portable {
            // Baseline ISA: no auto-detected host features. Produces portable
            // binaries that run on any CPU supporting the base architecture.
            native_isa_builder_with_options(false).unwrap_or_else(|msg| {
                panic!("host machine is not supported: {}", msg);
            })
        } else {
            // Auto-detect host CPU features (AVX2, SSE4.2, BMI2, POPCNT on x86;
            // NEON, AES, CRC on aarch64). Allows Cranelift to emit feature-specific
            // instructions like vpmovmskb, popcnt, tzcnt, etc.
            native_isa_builder_with_options(true).unwrap_or_else(|msg| {
                panic!("host machine is not supported: {}", msg);
            })
        };

        // Ensure critical ISA-specific features are explicitly enabled when the
        // CPU supports them. While native_isa_builder_with_options(true) probes
        // CPUID/system registers, explicit enablement here serves as a safety net
        // for edge cases (custom target triples, future Cranelift changes) and
        // documents our performance-critical feature requirements.
        //
        // x86_64: BMI1/BMI2 (tzcnt, blsr for bit manipulation in hash probing),
        //         POPCNT (popcount for set operations and hash table occupancy).
        // aarch64: LSE (atomic CAS/SWP for lock-free refcount operations).
        #[cfg(target_arch = "x86_64")]
        if !portable && target.is_none() {
            if std::arch::is_x86_feature_detected!("bmi1") {
                let _ = isa_builder.enable("has_bmi1");
            }
            if std::arch::is_x86_feature_detected!("bmi2") {
                let _ = isa_builder.enable("has_bmi2");
            }
            if std::arch::is_x86_feature_detected!("popcnt") {
                let _ = isa_builder.enable("has_popcnt");
            }
        }
        #[cfg(target_arch = "aarch64")]
        if !portable && target.is_none() && std::arch::is_aarch64_feature_detected!("lse") {
            let _ = isa_builder.enable("has_lse");
        }

        let isa = isa_builder
            .finish(settings::Flags::new(flag_builder))
            .unwrap();
        let mut builder = ObjectBuilder::new(
            isa,
            "molt_output",
            cranelift_module::default_libcall_names(),
        )
        .unwrap();
        // Emit each function into its own object section so the linker can
        // discard unreferenced runtime functions via -dead_strip / --gc-sections.
        builder.per_function_section(true);
        let module = ObjectModule::new(builder);
        let ctx = module.make_context();

        Self {
            module,
            ctx,
            trampoline_ids: BTreeMap::new(),
            import_ids: BTreeMap::new(),
            skip_ir_passes: false,
            skip_shared_stdlib_partition: false,
            emit_app_intrinsic_resolver: true,
            app_intrinsic_manifest: None,
            external_function_names: std::collections::BTreeSet::new(),
            module_context: None,
            data_pool: BTreeMap::new(),
            next_data_id: 0,
            declared_func_arities: BTreeMap::new(),
            defined_func_names: std::collections::BTreeSet::new(),
            deferred_defines: Vec::new(),
        }
    }

    pub fn build_module_context(functions: &[FunctionIR]) -> NativeBackendModuleContext {
        NativeBackendModuleContext::from_functions(functions)
    }

    pub fn set_module_context(&mut self, context: NativeBackendModuleContext) {
        self.module_context = Some(context);
    }

    /// Compile all deferred function definitions in parallel using rayon,
    /// then define the resulting bytes sequentially via `define_function_bytes`.
    /// Any Cranelift compile failure aborts codegen instead of producing a
    /// partial object file with runtime-aborting placeholders.
    fn flush_deferred_defines(&mut self) {
        use cranelift_codegen::control::ControlPlane;
        use rayon::prelude::*;

        let deferred: Vec<DeferredDefine> = std::mem::take(&mut self.deferred_defines);
        if deferred.is_empty() {
            return;
        }

        // Compile all functions in parallel. Each worker gets its own
        // Context + ControlPlane but shares one rebuilt OwnedTargetIsa.
        struct CompiledFunc {
            func_id: cranelift_module::FuncId,
            name: String,
            alignment: u64,
            code: Vec<u8>,
            relocs: Vec<cranelift_module::ModuleReloc>,
        }

        let compile_isa = Self::rebuild_owned_isa(self.module.isa(), None)
            .unwrap_or_else(|err| panic!("failed to rebuild TargetIsa for deferred flush: {err}"));

        let results: Vec<CompiledFunc> = {
            // Arc<dyn TargetIsa> contains a raw pointer that isn't marked
            // Send/Sync, but the target ISA is immutable after construction and
            // safe to share across parallel Cranelift compilation workers.
            #[derive(Clone)]
            struct SendIsa(std::sync::Arc<dyn cranelift_codegen::isa::TargetIsa>);
            unsafe impl Send for SendIsa {}
            unsafe impl Sync for SendIsa {}

            let compile_isa = SendIsa(compile_isa);
            let mut indexed: Vec<(usize, CompiledFunc)> = deferred
                .into_par_iter()
                .enumerate()
                .map(|(idx, item)| {
                    let DeferredDefine {
                        func_id,
                        func,
                        name,
                    } = item;
                    let isa = compile_isa.clone().0;
                    let mut ctx = Context::for_function(func);
                    let mut ctrl = ControlPlane::default();
                    if let Err(err) = ctx.compile(&*isa, &mut ctrl) {
                        let message = format!("Cranelift compilation failed for `{name}`: {err:?}");
                        let _ = crate::debug_artifacts::append_debug_artifact(
                            "native/cranelift_errors.txt",
                            format!("{message}\n"),
                        );
                        panic!("{message}");
                    }
                    let compiled = ctx.compiled_code().unwrap_or_else(|| {
                        panic!("Cranelift produced no compiled code for `{name}`")
                    });
                    let alignment = compiled.buffer.alignment as u64;
                    let code = compiled.buffer.data().to_vec();
                    let relocs: Vec<cranelift_module::ModuleReloc> = compiled
                        .buffer
                        .relocs()
                        .iter()
                        .map(|r| {
                            cranelift_module::ModuleReloc::from_mach_reloc(r, &ctx.func, func_id)
                        })
                        .collect();
                    (
                        idx,
                        CompiledFunc {
                            func_id,
                            name,
                            alignment,
                            code,
                            relocs,
                        },
                    )
                })
                .collect();
            indexed.sort_by_key(|(idx, _)| *idx);
            indexed.into_iter().map(|(_, result)| result).collect()
        };

        // Sequential phase: define compiled functions in original order.
        for cf in results {
            self.module
                .define_function_bytes(cf.func_id, cf.alignment, &cf.code, &cf.relocs)
                .unwrap_or_else(|err| {
                    panic!("define_function_bytes failed for `{}`: {err}", cf.name)
                });
            self.defined_func_names.insert(cf.name);
        }
    }

    pub(crate) fn intern_data_segment(
        module: &mut ObjectModule,
        data_pool: &mut BTreeMap<Vec<u8>, cranelift_module::DataId>,
        next_data_id: &mut u64,
        bytes: &[u8],
    ) -> cranelift_module::DataId {
        if let Some(existing) = data_pool.get(bytes) {
            return *existing;
        }
        let name = format!("data_pool_{}", *next_data_id);
        *next_data_id += 1;
        let data_id = module
            .declare_data(&name, Linkage::Local, false, false)
            .unwrap();
        let mut data_ctx = DataDescription::new();
        data_ctx.define(bytes.to_vec().into_boxed_slice());
        module.define_data(data_id, &data_ctx).unwrap();
        data_pool.insert(bytes.to_vec(), data_id);
        data_id
    }

    /// Walk backwards from `before_idx` to find a `"const"` op whose `out`
    /// matches `var_name` and return its integer value.  Used by the
    /// iter_next peephole to resolve constant index arguments.
    pub(crate) fn resolve_const_int(
        ops: &[OpIR],
        before_idx: usize,
        var_name: &str,
    ) -> Option<i64> {
        for i in (0..before_idx).rev() {
            let op = &ops[i];
            if op.kind == "const"
                && let Some(ref out) = op.out
                && out == var_name
            {
                return op.value;
            }
        }
        None
    }

    /// Cached version of `module.declare_function(name, Linkage::Import, &sig)`.
    /// Returns the `FuncId` for the given runtime import, reusing a previous
    /// declaration when the same name has already been declared.  The signature
    /// shape is validated on cache hits to guard against mismatches.
    ///
    /// Takes split borrows (`module` + `import_ids`) so callers can hold a
    /// concurrent `FunctionBuilder` borrow on `self.ctx.func`.
    pub(crate) fn import_func_id_split(
        module: &mut ObjectModule,
        import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
        name: &'static str,
        params: &[types::Type],
        returns: &[types::Type],
    ) -> cranelift_module::FuncId {
        let shape = ImportSignatureShape::from_types(params, returns);
        if let Some((func_id, cached_shape)) = import_ids.get(name) {
            assert_eq!(
                cached_shape, &shape,
                "import signature mismatch for {name}: {:?} vs {:?}",
                cached_shape, shape
            );
            return *func_id;
        }

        let mut sig = module.make_signature();
        for param in params {
            sig.params.push(AbiParam::new(*param));
        }
        for ret in returns {
            sig.returns.push(AbiParam::new(*ret));
        }
        let func_id = module
            .declare_function(name, Linkage::Import, &sig)
            .unwrap_or_else(|err| {
                panic!("import declaration mismatch for `{name}`: expected {shape:?}: {err}")
            });
        import_ids.insert(name, (func_id, shape));
        func_id
    }

    /// Convenience wrapper around `import_func_id_split` for use when
    /// `&mut self` is not split-borrowed (e.g. in tests).
    #[cfg(test)]
    fn import_func_id(
        &mut self,
        name: &'static str,
        params: &[types::Type],
        returns: &[types::Type],
    ) -> cranelift_module::FuncId {
        Self::import_func_id_split(
            &mut self.module,
            &mut self.import_ids,
            name,
            params,
            returns,
        )
    }

    pub fn compile(mut self, ir: SimpleIR) -> CompileOutput {
        let timing = env_setting("MOLT_BACKEND_TIMING")
            .as_deref()
            .map(parse_truthy_env)
            .unwrap_or(false);
        let compile_start = std::time::Instant::now();
        let mut ir = ir;
        // Backend selection: MOLT_BACKEND=llvm is an explicit contract. A
        // missing LLVM feature must fail closed instead of substituting a
        // different backend and producing misleading validation evidence.
        let backend_setting = env_setting("MOLT_BACKEND");
        let use_llvm = backend_setting_requests_llvm(backend_setting.as_deref());
        assert_requested_llvm_backend_available(use_llvm);
        apply_profile_order(&mut ir);
        // Whole-program reachability is the first backend custody boundary for
        // app objects. The frontend/stdlib transport may carry the full
        // importable stdlib graph, but TIR optimization and codegen must only
        // see functions reachable from declared roots. A second DFE after the
        // module inliner below catches functions made unreachable by inlining.
        if !self.skip_ir_passes {
            eliminate_dead_functions(&mut ir);
        }
        // ── Pre-TIR IR passes (parallel) ────────────────────────
        // Each pass operates on a single FunctionIR with no shared
        // mutable state, so all 8 passes can run in parallel across
        // functions using rayon.  Fusing them into one par_iter_mut
        // avoids 8× thread-pool dispatch overhead and improves cache
        // locality (each function stays hot while all passes run).
        {
            use rayon::prelude::*;
            ir.functions.par_iter_mut().for_each(|func_ir| {
                rewrite_stateful_loops(func_ir);
                // Eliminate UnboundLocalError checks early — they are dead
                // code in type-annotated functions and removing them before
                // other passes prevents the ~11 ops per variable-access
                // pattern from polluting escape analysis and constant folding.
                eliminate_unbound_local_checks(func_ir);
                eliminate_redundant_guard_tags(func_ir);
                elide_dead_struct_allocs(func_ir);
                escape_analysis(func_ir);
                // rc_coalescing has its own MOLT_DISABLE_RC_COALESCE early-return
                // gate — single source of truth, no parallel pre-check.
                rc_coalescing(func_ir);
                fold_constants(&mut func_ir.ops);
                fold_constants_cross_block(&mut func_ir.ops);
                elide_safe_exception_checks(func_ir);
                hoist_loop_invariants(func_ir);
            });
        }
        // ── GPU kernel detection ──
        // Functions containing GPU intrinsic ops (gpu_thread_id, gpu_block_id,
        // etc.) are GPU kernels.  Flag them in metadata so the GPU pipeline can
        // handle them separately, but they still flow through the canonical
        // TIR/LIR pipeline like every other function.
        let mut gpu_kernel_names: Vec<String> = Vec::new();
        for func_ir in &ir.functions {
            let is_gpu = func_ir.ops.iter().any(|op| {
                matches!(
                    op.kind.as_str(),
                    "gpu_thread_id"
                        | "gpu_block_id"
                        | "gpu_block_dim"
                        | "gpu_grid_dim"
                        | "gpu_barrier"
                )
            });
            if is_gpu {
                gpu_kernel_names.push(func_ir.name.clone());
            }
        }
        if !gpu_kernel_names.is_empty() {
            eprintln!(
                "[molt-gpu] Detected {} GPU kernel function(s): {:?}",
                gpu_kernel_names.len(),
                gpu_kernel_names
            );
        }

        // ── TIR optimization pipeline ──
        // The TIR roundtrip (lower->refine->optimize->lower-back) is mandatory
        // for backend-facing functions. Debugging must use dumps and verifier
        // evidence rather than bypassing typed IR.
        // All TIR-lowered control flow uses pure label/jump/br_if patterns
        // (no structured loop_start/loop_end).  The Cranelift function compiler
        // handles back-edges via has_loop_or_backedge detection.
        let mut optimized_tir_by_name: std::collections::BTreeMap<
            String,
            crate::tir::function::TirFunction,
        > = std::collections::BTreeMap::new();
        {
            use rayon::prelude::*;

            let _tir_dump = env_setting("TIR_DUMP").as_deref() == Some("1");
            let _tir_stats = env_setting("TIR_OPT_STATS").as_deref() == Some("1");
            let mut tir_cache =
                crate::tir::cache::CompilationCache::open(crate::tir::cache::backend_cache_dir());

            // Phase 1 (sequential): check cache for every function. For cache
            // hits, apply immediately. For misses, collect the function index,
            // content hash, and op count for bounded optimization batches.
            let mut work_items: Vec<TirOptimizationWorkItem> = Vec::new();

            // Debug: dump raw IR for functions matching MOLT_DUMP_FUNC_IR pattern.
            let dump_func_pattern = std::env::var("MOLT_DUMP_FUNC_IR").ok();

            for (i, func_ir) in ir.functions.iter_mut().enumerate() {
                // Extern functions: bodies live in stdlib_shared.o.
                // They are registered as external before codegen.
                if func_ir.is_extern {
                    continue;
                }

                // Dump raw ops to file for debugging TIR roundtrip issues.
                if let Some(ref pattern) = dump_func_pattern
                    && func_ir.name.contains(pattern.as_str())
                {
                    let sanitized: String = func_ir
                        .name
                        .chars()
                        .map(|c| {
                            if c.is_alphanumeric() || c == '_' {
                                c
                            } else {
                                '_'
                            }
                        })
                        .collect();
                    let mut dump = String::new();
                    dump.push_str(&format!(
                        "// func: {} ({} ops)\n",
                        func_ir.name,
                        func_ir.ops.len()
                    ));
                    dump.push_str(&format!("// params: {:?}\n", func_ir.params));
                    dump.push_str(&format!("// param_types: {:?}\n", func_ir.param_types));
                    for (idx, op) in func_ir.ops.iter().enumerate() {
                        dump.push_str(&format!("{:4}: kind={:30} out={:20} var={:20} args={:40} val={:?} sval={:?} fi={:?} ff={:?}\n",
                                idx, op.kind,
                                op.out.as_deref().unwrap_or(""),
                                op.var.as_deref().unwrap_or(""),
                                op.args.as_ref().map(|a| a.join(",")).unwrap_or_default(),
                                op.value, op.s_value, op.fast_int, op.fast_float));
                    }
                    let _ = crate::debug_artifacts::write_debug_artifact(
                        format!("ir/{sanitized}.txt"),
                        dump,
                    );
                }

                // Loop markers (loop_start, loop_end) are now preserved through
                // the TIR roundtrip via LoopRole metadata on TirFunction, so
                // functions with loops benefit from TIR optimization.
                let body_bytes = crate::tir::serialize::serialize_ops(&func_ir.ops);
                let cache_hash_body = native_tir_cache_hash_body(&body_bytes);
                let content_hash = crate::tir::cache::CompilationCache::compute_hash_with_signature(
                    &func_ir.name,
                    &func_ir.params,
                    func_ir.param_types.as_deref(),
                    &cache_hash_body,
                );
                // Check TIR cache: if we have validated optimized ops from a
                // previous build with the same content hash, reuse them.
                if let Some(cached_bytes) = tir_cache.get(&content_hash)
                    && let Some(cached_tir) =
                        crate::tir::serialize::deserialize_tir_function(&cached_bytes)
                {
                    let cached_ops = crate::tir::lower_to_simple::lower_to_simple_ir(&cached_tir);
                    debug_assert!(
                        crate::tir::lower_to_simple::validate_labels(&cached_ops),
                        "native TIR cache back-conversion emitted invalid labels for '{}'",
                        cached_tir.name
                    );
                    func_ir.ops = cached_ops;
                    optimized_tir_by_name.insert(func_ir.name.clone(), cached_tir);
                    continue;
                }
                work_items.push(TirOptimizationWorkItem {
                    index: i,
                    content_hash,
                    op_count: func_ir.ops.len(),
                });
            }

            let uncached_count = work_items.len();
            if uncached_count > 0 {
                let resource_plan = tir_optimization_resource_plan();
                let work_batches = partition_tir_optimization_work_items_with_limits(
                    work_items,
                    resource_plan.wave_function_limit,
                    resource_plan.wave_op_budget,
                );
                let batch_count = work_batches.len();
                if batch_count == 1 {
                    eprintln!(
                        "MOLT_BACKEND: TIR optimizing {uncached_count} uncached functions with {} worker(s)",
                        resource_plan.threads
                    );
                } else {
                    eprintln!(
                        "MOLT_BACKEND: TIR optimizing {uncached_count} uncached functions in {batch_count} bounded waves with {} worker(s)",
                        resource_plan.threads
                    );
                }
                let tir_start = std::time::Instant::now();

                // Phase 2 (parallel): run the TIR pipeline on every uncached
                // function.  Each work item borrows only its own FunctionIR
                // (via index) and produces an independent result.
                //
                // We cannot borrow &mut ir.functions[i] in parallel because
                // Rust's borrow checker does not allow multiple mutable refs
                // into the same Vec, even at disjoint indices, through closures.
                // Instead we extract the ops, optimize them in parallel, and
                // write them back.
                // Each element: (func_index, content_hash, optimized_ops)
                // Use a custom thread pool with 16MB stacks for TIR.
                // lower_to_simple_ir has deeply nested closures capturing
                // many HashMaps, which exceeds rayon's default 8MB stacks.
                let tir_pool = rayon::ThreadPoolBuilder::new()
                    .num_threads(resource_plan.threads)
                    .stack_size(64 * 1024 * 1024)
                    .build()
                    .expect("Failed to build TIR thread pool");
                for (batch_idx, batch_items) in work_batches.into_iter().enumerate() {
                    let batch_ops = batch_items.iter().map(|wi| wi.op_count).sum::<usize>();
                    if batch_count > 1 {
                        eprintln!(
                            "MOLT_BACKEND: TIR batch {}/{} ({} functions, {} ops / budget {})",
                            batch_idx + 1,
                            batch_count,
                            batch_items.len(),
                            batch_ops,
                            resource_plan.wave_op_budget
                        );
                    }
                    let inputs: Vec<TirOptimizationInput> = batch_items
                        .into_iter()
                        .map(|wi| {
                            let func_ir = &ir.functions[wi.index];
                            TirOptimizationInput {
                                index: wi.index,
                                content_hash: wi.content_hash,
                                name: func_ir.name.clone(),
                                params: func_ir.params.clone(),
                                ops: func_ir.ops.clone(),
                                param_types: func_ir.param_types.clone(),
                            }
                        })
                        .collect();
                    let results: Vec<TirOptimizationOutput> = tir_pool
                        .install(|| inputs.into_par_iter().map(optimize_tir_input).collect());

                    // Phase 3 (sequential): apply validated TIR ops and cache them.
                    for output in results {
                        let func_ir = &mut ir.functions[output.index];
                        func_ir.ops = output.simple_ops;
                        let bytes = crate::tir::serialize::serialize_tir_function(&output.tir_func);
                        tir_cache.put(&output.content_hash, &bytes, vec![]);
                        optimized_tir_by_name.insert(func_ir.name.clone(), output.tir_func);
                    }
                }

                let tir_elapsed = tir_start.elapsed();
                eprintln!(
                    "MOLT_BACKEND: TIR parallel optimization took {tir_elapsed:.2?} for {uncached_count} functions"
                );
            }

            tir_cache.save_index();
        }
        if !self.skip_ir_passes {
            eliminate_dead_ops(&mut ir);
        }
        // Post-TIR: analysis + inlining (from main)
        // Capture task_kinds and task_closure_sizes BEFORE megafunction splitting.
        // Megafunction splitting can separate `func_new` from its corresponding
        // `set_attr_generic_obj(__molt_is_generator__)` into different chunk
        // functions, which breaks the per-function cross-reference in
        // `analyze_native_backend_ir`.  By capturing generator/coroutine
        // annotations now, we ensure they survive the split.
        let pre_split_task_kinds: BTreeMap<String, TrampolineKind>;
        let pre_split_task_closure_sizes: BTreeMap<String, i64>;
        {
            // This pre-split capture only reads task annotations; the leaf set is
            // recomputed post-split below, so skip the (heavier) whole-program
            // leaf analysis here.
            let analysis = analyze_native_backend_ir(&ir, /* compute_leaves */ false);
            pre_split_task_kinds = analysis.task_kinds;
            pre_split_task_closure_sizes = analysis.task_closure_sizes;
            // The SimpleIR-carrier module phase runs for the Cranelift path
            // (gated on skip_ir_passes). It is SKIPPED for the LLVM path
            // (`use_llvm`): LLVM lowers from TIR directly, so it runs the SAME
            // `run_module_pipeline` on its own TIR functions inside the
            // `if use_llvm` branch below and lowers the inlined `TirModule`
            // *directly* — never round-tripping the merged bodies back through
            // SimpleIR. Running the module phase here too would inline twice (once
            // into the SimpleIR `ir.functions`, then again when the LLVM branch
            // re-inlines its TIR lift). One inliner per emitted program: the
            // SimpleIR carrier feeds Cranelift, the TIR module feeds LLVM.
            //
            // The deleted `needs_inlining` heuristic keyed on
            // `kind == "call_internal"`, but the TIR roundtrip's back-conversion
            // re-emits every call as `kind: "call"` — so the flag was always
            // false for TIR-processed functions and had silently disabled
            // production inlining (for the legacy SimpleIR inliner too). The
            // call graph itself is the authority on whether anything is
            // inlinable; an inline-free module just runs a cheap analysis.
            if !self.skip_ir_passes && !use_llvm {
                // E1 ACTIVATION: the TIR function inliner (tir/passes/inliner.rs,
                // via run_module_pipeline) is now the production inliner — SSA-based,
                // exception-label-safe, call-graph bottom-up, cost-model-gated, and
                // it re-optimizes each merged caller through the per-function
                // pipeline. It replaces the legacy SimpleIR `inline_functions`
                // (string-rename, no SSA, no cost model — retired in e-4).
                // Assemble the module from the optimized TIR custody map (fresh
                // worker output or native TIR cache hit) so module transforms and
                // terminal drops do not re-lift the expanded SimpleIR carrier.
                // Back-convert ONLY functions changed by module/drop phases;
                // every unchanged function keeps its byte-identical
                // per-function output.
                // Rollback: MOLT_DISABLE_INLINING=1 (guard in run_inliner).
                let native_tti = crate::tir::target_info::TargetInfo::native_from_simd_caps(
                    crate::tir::target_info::SimdCaps::detect_host(),
                );
                let mut tir_functions = Vec::new();
                let mut idx_map = Vec::new();
                for (idx, func_ir) in ir.functions.iter().enumerate() {
                    if func_ir.is_extern {
                        continue;
                    }
                    let tir_func = optimized_tir_by_name
                        .remove(&func_ir.name)
                        .unwrap_or_else(|| crate::tir::lower_from_simple::lower_to_tir(func_ir));
                    tir_functions.push(tir_func);
                    idx_map.push(idx);
                }
                let mut tir_module = crate::tir::function::TirModule {
                    name: "native_module".to_string(),
                    functions: tir_functions,
                };
                // Functions the shared-stdlib partition will externalize into
                // `stdlib_shared.o` have external linkage: the inliner must keep
                // the external reference rather than fork a private copy of a body
                // this app object does not own (computed BEFORE `externalize_*`
                // clears their ops, from the same predicate it uses).
                let external_symbols = if self.skip_shared_stdlib_partition {
                    BTreeSet::new()
                } else {
                    shared_stdlib_external_symbols(&ir)
                };
                let non_inlinable: std::collections::HashSet<String> =
                    external_symbols.into_iter().collect();
                let module_analysis =
                    crate::tir::run_module_pipeline(&mut tir_module, &native_tti, &non_inlinable);
                let changed: std::collections::HashSet<&str> = module_analysis
                    .changed_functions
                    .iter()
                    .map(String::as_str)
                    .collect();
                for (pos, &orig_idx) in idx_map.iter().enumerate() {
                    let tir_func = &tir_module.functions[pos];
                    if changed.contains(tir_func.name.as_str()) {
                        let ops = crate::tir::lower_to_simple::lower_to_simple_ir(tir_func);
                        debug_assert!(
                            crate::tir::lower_to_simple::validate_labels(&ops),
                            "E1: inlined back-conversion emitted invalid labels for '{}'",
                            tir_func.name
                        );
                        ir.functions[orig_idx].ops = ops;
                    }
                }
            }
        }
        // ── RC drop insertion: terminal phase for the skip_ir_passes path ──────
        // The whole-program module phase (which runs the drop finalizer over its
        // TIR module, see `run_module_pipeline`) is SKIPPED for `skip_ir_passes`
        // builds — the stdlib-cache object and the per-batch application codegen,
        // which forgo inlining/promotion and do per-function-only optimization.
        // Drop insertion is a per-function correctness concern (it closes the
        // expression-temporary leak), NOT a whole-program optimization, so it must
        // still run there. With no module phase, the (already-run, cached)
        // per-function pipeline is the last transform, so drops run here as the
        // terminal step — over the SimpleIR carrier, post-cache (the cache never
        // stores drop-inserted ops keyed by the drop-free input hash). Runs BEFORE
        // `split_megafunctions` so DecRef/value-def pairs stay within one function
        // (the non-skip path likewise drops in the module phase, before splitting).
        // The LLVM lane has its own module phase below and is excluded here.
        if self.skip_ir_passes && !use_llvm {
            let native_tti = crate::tir::target_info::TargetInfo::native_from_simd_caps(
                crate::tir::target_info::SimdCaps::detect_host(),
            );
            crate::tir::drop_phase::finalize_simple_ir_drops_with_tir_custody(
                &mut ir.functions,
                &native_tti,
                &mut optimized_tir_by_name,
            );
        }
        // Dead function elimination: remove functions that are unreachable from
        // the entry point after inlining.  This reduces code size for both the
        // native object and the downstream linker's work.
        if !self.skip_ir_passes {
            eliminate_dead_functions(&mut ir);
        }
        // Megafunction splitting: break up functions with >4000 ops (or
        // MOLT_MAX_FUNCTION_OPS) into private chunk functions to avoid
        // Cranelift's O(n²) register allocator blowup.
        split_megafunctions(&mut ir);
        rewrite_annotate_stubs(&mut ir);
        for func in &mut ir.functions {
            rewrite_copy_aliases(&mut func.ops);
            // Split-field read deforestation: a non-escaping `s.split(sep)[idx]`
            // consumed only by `len`/`ord(field[i])`/`== const` (the shape the
            // split-field-enabled inliner exposes when it splices `parse_int(field)`)
            // is rewritten to bounds-once reads so the field never materializes —
            // the csv/etl ETL release-blocker fix. Runs AFTER copy-alias rewrite so
            // the inlined `len`/`ord_at` consumers reference the field's canonical
            // SSA name directly (pre-collapse they read an alias of it).
            crate::passes::deforest_split_field_reads(func);
            canonicalize_direct_raise_edges(func);
            if std::env::var("MOLT_DUMP_REWRITTEN_FUNC").as_deref() == Ok(func.name.as_str()) {
                let mut dump = String::new();
                for (idx, op) in func.ops.iter().enumerate() {
                    let _ = writeln!(dump, "{idx:04}: {:?}", op);
                }
                let _ = std::fs::write("tmp/rewritten_func_ir.txt", dump);
            }
        }
        // Compute the per-app intrinsic manifest BEFORE the stdlib partition
        // clears extern function bodies. The stdlib_shared.o partition's
        // trampolines reach intrinsics by name too, and those uses must be
        // covered by the per-app resolver (RISK-3) — once `externalize_shared_stdlib_partition`
        // clears their ops, the manifest scan can no longer see them. The native
        // backend emits `molt_app_resolve_intrinsic` over exactly this set so
        // `resolve_symbol`/`resolve_core_symbol` become native-unreachable and
        // the linker dead-strips every unused intrinsic.
        // The manifest must cover every intrinsic reached via the dynamic
        // name-based resolver path across ALL objects of the final binary. When
        // the orchestrator split the program (stdlib cache split / batching) it
        // pre-computes the manifest over the full function set and threads it in;
        // otherwise this object holds the full set and we derive it locally.
        // Only the object that emits the resolver needs (and computes) the
        // manifest; stdlib-cache / non-primary batch objects set
        // `emit_app_intrinsic_resolver = false` and never reference it. Deriving it
        // there would also wrongly demand the staticlib symbol set for an object
        // that takes no intrinsic addresses. When the orchestrator split the
        // program it threads a pre-computed full-set manifest in; otherwise this
        // object holds the full set and derives it locally against the REQUIRED
        // staticlib symbol set (no heuristic fallback — see
        // `runtime_intrinsic_symbols_required`).
        // The per-app resolver is emitted by BOTH the Cranelift path below and
        // the LLVM path (`use_llvm`): the LLVM-compiled application object must
        // carry `molt_app_resolve_intrinsic` (referenced by the CLI's main stub)
        // exactly like the Cranelift object, or the link leaves it undefined and
        // every name-based intrinsic resolution fails at runtime. The manifest
        // scan is backend-independent (it reads `FunctionIR` const_str ops
        // against the linked staticlib's intrinsic symbol set), so compute it —
        // and require the exact symbol set — whenever THIS object will emit the
        // resolver, regardless of which codegen backend produces the bytes.
        let emit_resolver_here = self.emit_app_intrinsic_resolver;
        let app_intrinsic_manifest = if emit_resolver_here {
            self.app_intrinsic_manifest.take().unwrap_or_else(|| {
                // `_checked`: requires the staticlib symbol set (fail-closed)
                // only when some `molt_`-prefixed const_str exists — an empty
                // module (the CLI's post-build feature probe) has a necessarily
                // empty manifest and must not demand a symbol file that is not
                // staged for it.
                crate::passes::compute_intrinsic_manifest_checked(&ir.functions)
            })
        } else {
            self.app_intrinsic_manifest.take().unwrap_or_default()
        };
        if !self.skip_shared_stdlib_partition {
            externalize_shared_stdlib_partition(&mut ir);
        }
        if timing {
            let passes_elapsed = compile_start.elapsed();
            eprintln!("MOLT_BACKEND_TIMING: IR passes took {passes_elapsed:.2?}");
        }
        // ── LLVM backend dispatch ──
        // When MOLT_BACKEND=llvm and the llvm feature is compiled in, route
        // through the LLVM backend instead of Cranelift.  Each function is
        // lifted to TIR, lowered to LLVM IR, then the whole module is
        // optimized and emitted as a native object file.
        #[cfg(feature = "llvm")]
        if use_llvm {
            use crate::llvm_backend::{LlvmBackend, MoltOptLevel};
            use crate::tir::lower_from_simple::lower_to_tir;

            let context = inkwell::context::Context::create();
            let mut llvm = LlvmBackend::new(&context, "molt_module");

            // Declare all runtime functions that lowered code may call into.
            crate::llvm_backend::runtime_imports::declare_runtime_functions(
                llvm.context,
                &llvm.module,
            );

            let func_count = ir.functions.iter().filter(|f| !f.is_extern).count();
            let total_ops: usize = ir
                .functions
                .iter()
                .filter(|f| !f.is_extern)
                .map(|f| f.ops.len())
                .sum();
            eprintln!(
                "MOLT_BACKEND(llvm): compiling {func_count} functions ({total_ops} total ops)"
            );
            let codegen_start = std::time::Instant::now();

            // Unified cost model (Tier-0 S2) for the LLVM target, derived from
            // the host CPU's vector feature string so the vectorizer can size
            // lanes to the actual machine (behavior-neutral today: the width is
            // a dead annotation, but structurally correct for real SIMD codegen).
            let llvm_tti = crate::tir::target_info::TargetInfo::from_llvm_feature_string(
                inkwell::targets::TargetMachine::get_host_cpu_features()
                    .to_str()
                    .unwrap_or(""),
            );
            // Fuse `obj.method(args)` / `super().method(args)` dispatch into the
            // allocation-free `call_method_ic` / `call_super_method_ic` ops
            // BEFORE lifting to TIR. Unlike the Cranelift path (which fuses the
            // post-roundtrip SimpleIR immediately before `compile_func`), the
            // LLVM path lowers directly from the per-function-optimized TIR, so
            // the IC ops must enter the TIR roundtrip as preserved `Copy` ops
            // (`_original_kind`), which `lower_preserved_simpleir_op` lowers. The
            // TIR effects oracle treats a `Copy` carrying `_original_kind` as
            // observably-effecting (effects.rs `op_has_observable_effect_when_dead`),
            // so the per-function pipeline below preserves them. Built from the
            // same fused `func` as `function_repr_facts`, keeping the SimpleIR /
            // TIR pair aligned. Extern (declaration-only) functions have empty
            // bodies — nothing to fuse.
            for func in &mut ir.functions {
                if !func.is_extern {
                    fuse_method_dispatch(func);
                }
            }
            let mut tir_funcs: Vec<(bool, crate::tir::function::TirFunction)> = ir
                .functions
                .iter()
                .map(|func| {
                    let mut tir_func = lower_to_tir(func);
                    // Extern functions (e.g. shared-stdlib-partition symbols
                    // externalized by `externalize_shared_stdlib_partition`) have
                    // had their bodies cleared: they are declaration-only and live
                    // in `stdlib_shared.o`. They lower to a bodyless TIR function
                    // (no blocks, hence no entry block), which would fail the TIR
                    // verifier the moment the optimization pipeline ran on it. They
                    // are *declared* below (`declare_tir_function`) for call
                    // resolution but never *defined*, so there is nothing to
                    // optimize. Mirror the Cranelift per-function pipeline, which
                    // skips extern functions for the same reason. Lower for the
                    // signature only.
                    if !func.is_extern {
                        // Run the full TIR optimization pipeline — same as Cranelift/WASM.
                        // Without this, all values stay DynBox and every operation
                        // dispatches through the runtime instead of emitting native ops.
                        crate::tir::type_refine::refine_types(&mut tir_func);
                        let _stats = crate::tir::passes::run_pipeline(&mut tir_func, &llvm_tti);
                        crate::tir::type_refine::refine_types(&mut tir_func);
                    }
                    (func.is_extern, tir_func)
                })
                .collect();

            // ── Whole-program module phase (Tier-2 E1 inliner activation, LLVM) ──
            // This is the LLVM lane's parity point with native/wasm: it runs the
            // SAME `run_module_pipeline` (CallGraph → ModuleSummaries → bottom-up
            // E1 inliner → module-slot promotion → post-inline rebuild) the
            // Cranelift and WASM drivers run — but on the LLVM lane's own TIR
            // functions, with the LLVM cost model (`llvm_tti`), and it lowers the
            // resulting inlined `TirModule` *directly* to LLVM IR below. There is
            // NO SimpleIR round-trip on the LLVM path: the merged bodies stay in
            // TIR from the inliner straight through `try_lower_tir_to_llvm`. The
            // Cranelift-lane SimpleIR module phase above is skipped for `use_llvm`
            // (see the `!use_llvm` guard) so the program is inlined exactly once.
            //
            // Extern functions are runtime declarations with empty bodies (the
            // shared-stdlib partition's `stdlib_shared.o` symbols, already
            // externalized before this branch): they are not inlinable and stay
            // OUT of the module, so calls to them remain opaque call-graph edges
            // (exactly correct — an extern body is not owned by this object). They
            // are re-declared below for call resolution. Because externalization
            // has already physically removed their bodies, the module the inliner
            // sees contains only locally-owned bodies, so the `non_inlinable` set
            // is empty here (the native lane needs it only because its module
            // phase runs *before* externalization).
            if !self.skip_ir_passes {
                use crate::tir::function::TirModule;

                let mut externs: Vec<crate::tir::function::TirFunction> = Vec::new();
                let mut module = TirModule {
                    name: "llvm_module".to_string(),
                    functions: Vec::new(),
                };
                for (is_extern, tir_func) in tir_funcs.into_iter() {
                    if is_extern {
                        externs.push(tir_func);
                    } else {
                        module.functions.push(tir_func);
                    }
                }

                // Inlines bottom-up and re-optimizes merged callers; leaves every
                // changed body fully type-refined (see `run_inliner`). Rollback:
                // MOLT_DISABLE_INLINING=1 (guard in run_inliner).
                let _module_analysis = crate::tir::run_module_pipeline(
                    &mut module,
                    &llvm_tti,
                    &std::collections::HashSet::new(),
                );

                // Reassemble the lowering list: extern declarations first, then
                // the merged non-extern bodies. Declaration and lowering order is
                // immaterial — LLVM resolves calls by name and functions lower
                // independently.
                tir_funcs = Vec::with_capacity(externs.len() + module.functions.len());
                tir_funcs.extend(externs.into_iter().map(|f| (true, f)));
                tir_funcs.extend(module.functions.into_iter().map(|f| (false, f)));
            } else {
                // skip_ir_passes (LLVM batched / stdlib-cache path): the
                // whole-program module phase — which runs the terminal drop
                // finalizer over its TIR module — is skipped. Drop insertion is a
                // per-function correctness concern, so it still runs here on the
                // per-function-pipeline output, the last transform in this mode.
                // (The non-skip branch above ran drops inside `run_module_pipeline`.)
                // Funnels through the same `finalize_function_drops` entry as the
                // module/SimpleIR finalizers (uniform refine + double-process guard).
                for (is_extern, tir_func) in tir_funcs.iter_mut() {
                    if !*is_extern {
                        let _ =
                            crate::tir::drop_phase::finalize_function_drops(tir_func, &llvm_tti);
                    }
                }
            }

            llvm.function_return_types = tir_funcs
                .iter()
                .map(|(_, func)| (func.name.clone(), func.return_type.clone()))
                .collect();
            // Build the shared representation facts from the SimpleIR function
            // and the post-module-phase TIR the LLVM backend is about to lower.
            // This is the structural convergence point: the LLVM backend's
            // integer-carrier and container dispatch decisions now come from the
            // same `ScalarRepresentationPlan` the other three backends use,
            // instead of treating `TirType::I64` as an exact-i64 carrier. Built
            // in its own pass (after the module phase has run) so the plan's
            // internal lowering never interleaves with the pipeline.
            //
            // Keyed by NAME, not by position: the module phase reorders functions
            // (externs first) and can grow caller bodies, so the pre-inline
            // `ir.functions` order no longer aligns positionally with `tir_funcs`.
            // The SimpleIR function supplies only the name-keyed container-dispatch
            // plan; the soundness-critical `repr_by_value` (the trusted-unbox gate)
            // is derived purely from each merged TIR's own value-range
            // (`repr_by_value_for`'s `FunctionIR` param is unused), so the fresh
            // ValueIds the splice introduced are classified by a value-range
            // computed on the merged body — a false `RawI64Safe` (the 2bf51b730
            // truncation bug-class) cannot be introduced by inlining.
            let simple_by_name: std::collections::HashMap<&str, &FunctionIR> =
                ir.functions.iter().map(|f| (f.name.as_str(), f)).collect();
            llvm.function_repr_facts = tir_funcs
                .iter()
                .filter(|(is_extern, _)| !*is_extern)
                .filter_map(|(_, tir_func)| {
                    simple_by_name.get(tir_func.name.as_str()).map(|func| {
                        (
                            tir_func.name.clone(),
                            crate::representation_plan::LlvmReprFacts::build(func, tir_func),
                        )
                    })
                })
                .collect();

            // Parameter ABI carriers, derived from the SAME repr facts the body
            // lowers against: an unprovable-range `int` param is carried `DynBox`
            // (boxed), a value-range-proven one stays raw `I64`. This is the
            // caller-side coercion target that must agree with the callee's
            // entry-param carrier (`FunctionLowering::effective_block_arg_type`);
            // deriving both from `effective_param_types` over the same
            // `repr_by_value` keeps a heap-BigInt argument boxed end to end
            // (the trusted-unbox truncation bug-class is un-creatable at the call
            // boundary). Externs (no repr facts — they are opaque runtime
            // declarations) keep their declared ABI param types.
            llvm.function_param_types = tir_funcs
                .iter()
                .map(|(is_extern, tir_func)| {
                    let tys = if *is_extern {
                        tir_func.param_types.clone()
                    } else {
                        match llvm.function_repr_facts.get(&tir_func.name) {
                            Some(facts) => facts.effective_param_types(tir_func),
                            None => tir_func.param_types.clone(),
                        }
                    };
                    (tir_func.name.clone(), tys)
                })
                .collect();

            for (_, tir_func) in &tir_funcs {
                crate::llvm_backend::lowering::declare_tir_function(tir_func, &llvm);
            }

            for (is_extern, tir_func) in &tir_funcs {
                if *is_extern {
                    continue;
                }
                if env_setting("TIR_DUMP").as_deref() == Some("1")
                    || env_setting("MOLT_TIR_DUMP").as_deref() == Some("1")
                {
                    eprintln!(
                        "[LLVM] TIR for '{}':\n{}",
                        tir_func.name,
                        crate::tir::printer::print_function(tir_func)
                    );
                }
                crate::llvm_backend::lowering::try_lower_tir_to_llvm(tir_func, &llvm)
                    .unwrap_or_else(|err| panic!("{err}"));
            }

            // ── Per-app intrinsic resolver ────────────────────────────
            // The LLVM-compiled application object must carry
            // `molt_app_resolve_intrinsic` (referenced by the CLI's main stub and
            // registered with the runtime before `molt_runtime_init`) exactly
            // like the Cranelift object. Emitted into the LLVM module here, after
            // every function is lowered, so the manifest intrinsics already exist
            // as declarations whose addresses the resolver table takes. Gated on
            // `emit_resolver_here` so batch/stdlib-cache LLVM objects (which set
            // `emit_app_intrinsic_resolver = false`) never emit a duplicate
            // `_molt_app_resolve_intrinsic` symbol.
            if emit_resolver_here {
                llvm.emit_app_resolver_function(&app_intrinsic_manifest);
            }

            // Dump LLVM IR under the repo-local debug artifact root when
            // MOLT_LLVM_DUMP_IR=1.
            let dump_ir = env_setting("MOLT_LLVM_DUMP_IR").as_deref() == Some("1");
            if dump_ir {
                let _ = crate::debug_artifacts::write_debug_artifact(
                    "llvm/before_opt.ll",
                    llvm.dump_ir(),
                );
            }

            llvm.module.verify().unwrap_or_else(|msg| {
                panic!(
                    "LLVM module verification failed before optimization:\n{}",
                    msg.to_string()
                )
            });

            llvm.optimize(MoltOptLevel::Aggressive)
                .unwrap_or_else(|err| panic!("{err}"));
            llvm.module.verify().unwrap_or_else(|msg| {
                panic!(
                    "LLVM module verification failed after optimization:\n{}",
                    msg.to_string()
                )
            });

            if dump_ir {
                let _ = crate::debug_artifacts::write_debug_artifact(
                    "llvm/after_opt.ll",
                    llvm.dump_ir(),
                );
            }

            if timing {
                let codegen_elapsed = codegen_start.elapsed();
                eprintln!(
                    "MOLT_BACKEND_TIMING: LLVM codegen + optimization took {codegen_elapsed:.2?}"
                );
            }

            let tmp_obj = crate::debug_artifacts::prepare_unique_debug_artifact_path(
                "llvm/molt_llvm_output.o",
            )
            .expect("failed to prepare LLVM object path");
            llvm.emit_object(&tmp_obj, MoltOptLevel::Aggressive)
                .expect("LLVM object emission failed");
            let bytes = std::fs::read(&tmp_obj).unwrap_or_else(|err| {
                panic!(
                    "failed to read LLVM object file at {}: {}",
                    tmp_obj.display(),
                    err
                )
            });
            let _ = std::fs::remove_file(&tmp_obj);

            if timing {
                let total_elapsed = compile_start.elapsed();
                eprintln!(
                    "MOLT_BACKEND_TIMING: total LLVM backend compile: {total_elapsed:.2?}                      ({func_count} functions, {total_ops} ops, {} bytes)",
                    bytes.len()
                );
            }

            return CompileOutput { bytes };
        }
        // Re-analyze after dead function elimination and megafunction
        // splitting so defined_functions/closure_functions reflect only the
        // surviving (and newly created chunk) functions. The leaf set is
        // consumed by codegen (recursion-guard skip). When a whole-program
        // module context is already set (the batched path), its leaf set wins
        // over this per-batch one (see `effective_leaf_functions` below), so
        // skip the redundant per-batch whole-program leaf lift here.
        let need_local_leaves = self.module_context.is_none();
        let mut ir_analysis = analyze_native_backend_ir(&ir, need_local_leaves);
        // Merge pre-split task annotations: megafunction splitting can
        // separate `func_new` from `set_attr_generic_obj(__molt_is_generator__)`
        // into different chunk functions, causing the post-split analysis to
        // miss generator/coroutine annotations.  The pre-split analysis
        // captured these correctly before the ops were split apart.
        for (name, kind) in &pre_split_task_kinds {
            ir_analysis.task_kinds.entry(name.clone()).or_insert(*kind);
        }
        for (name, size) in &pre_split_task_closure_sizes {
            ir_analysis
                .task_closure_sizes
                .entry(name.clone())
                .or_insert(*size);
        }
        // Conditional trace elimination: skip emitting trace_enter/trace_exit calls
        // when tracing is disabled. Each guarded call site emits 2 trace function calls
        // (enter + exit); eliminating them saves codegen work on cache misses and
        // keeps the default native backend lane focused on production semantics.
        // Trace emission is opt-in via MOLT_BACKEND_EMIT_TRACES=1.
        let emit_traces = env_setting("MOLT_BACKEND_EMIT_TRACES")
            .as_deref()
            .map(parse_truthy_env)
            .unwrap_or(false);
        // Compile functions into one module. Backend codegen failures are hard
        // failures: the compiler must not produce partial objects with
        // runtime-aborting placeholders for functions it could not compile.
        // Register extern functions (bodies in stdlib_shared.o) so the
        // backend declares them as Import linkage, resolved by the linker.
        for func in &ir.functions {
            if func.is_extern {
                self.external_function_names.insert(func.name.clone());
            }
        }
        // Filter out extern functions — they have no ops to compile.
        ir.functions.retain(|f| !f.is_extern);
        let func_count = ir.functions.len();
        let total_ops: usize = ir.functions.iter().map(|f| f.ops.len()).sum();
        eprintln!("MOLT_BACKEND: compiling {func_count} functions ({total_ops} total ops)");
        let codegen_start = std::time::Instant::now();
        let local_function_arities: BTreeMap<String, usize> = ir
            .functions
            .iter()
            .map(|func| (func.name.clone(), func.params.len()))
            .collect();
        let local_return_alias_summaries =
            crate::passes::compute_return_alias_summaries(&ir.functions);
        let module_context = self.module_context.clone();
        let effective_function_arities =
            merge_function_arities(module_context.as_ref(), local_function_arities);
        // UNION the module context's whole-program metadata with this batch's
        // LOCAL scan (design-20 finding #3C activation): a `module_context` that
        // was built from a DIFFERENT function set (the stdlib cache) does not
        // contain a closure/task/leaf defined only in this batch. Replacing the
        // local scan dropped those, so a `call_guarded` to a user closure
        // skipped env extraction → garbage closure → subscript TypeError. Mirror
        // the union that `merge_function_arities`/`merge_function_has_ret`
        // already do; the local scan is authoritative for this batch's own
        // definitions, the module context adds cross-batch knowledge.
        let effective_closure_functions = merge_closure_functions(
            module_context.as_ref(),
            ir_analysis.closure_functions.clone(),
        );
        let effective_task_kinds =
            merge_task_kinds(module_context.as_ref(), ir_analysis.task_kinds.clone());
        let effective_task_closure_sizes = merge_task_closure_sizes(
            module_context.as_ref(),
            ir_analysis.task_closure_sizes.clone(),
        );
        let effective_leaf_functions =
            merge_leaf_functions(module_context.as_ref(), ir_analysis.leaf_functions.clone());
        // UNION (same rationale as merge_closure_functions): a module context
        // built from a different function set does not carry THIS batch's own
        // return-alias summaries; the local computation must not be dropped, or a
        // caller in this batch loses the callee's RC-return contract. Local wins
        // on overlap (it is recomputed over the post-optimization bodies).
        let effective_return_alias_summaries = {
            let mut merged = module_context
                .as_ref()
                .map(|context| context.return_alias_summaries.clone())
                .unwrap_or_default();
            merged.extend(local_return_alias_summaries);
            merged
        };
        let local_function_has_ret = compute_function_has_ret(&ir.functions);
        let effective_function_has_ret =
            merge_function_has_ret(module_context.as_ref(), local_function_has_ret);
        let mut module_known_functions = ir_analysis.defined_functions.clone();
        module_known_functions.extend(self.external_function_names.iter().cloned());
        let mut compiled = 0u32;
        let failed = 0u32;
        let mut slowest_func: Option<(String, std::time::Duration)> = None;
        // Progress reporting: pick interval based on function count so the
        // user sees roughly 20 updates during a long build, but at least
        // every 50 functions.
        let progress_interval = (func_count / 20).clamp(1, 50);
        let mut last_progress = std::time::Instant::now();
        let mut deferred_codegen_ops = 0usize;

        for mut func_ir in ir.functions {
            let func_name = func_ir.name.clone();
            let func_op_count = func_ir.ops.len().max(1);
            // Fuse `obj.method(args)` (get_attr_generic_ptr + callargs +
            // call_bind) into a single allocation-free `call_method_ic` op
            // (CPython LOAD_METHOD/CALL_METHOD optimisation).  Run as the LAST
            // transformation before codegen: `call_method_ic` is a backend-only
            // op with no TIR opcode, so it must not re-enter the TIR roundtrip
            // or the whole-program leaf/alias analyses (all already complete).
            fuse_method_dispatch(&mut func_ir);
            let func_start = std::time::Instant::now();
            self.compile_func(
                func_ir,
                &effective_task_kinds,
                &effective_task_closure_sizes,
                &ir_analysis.defined_functions,
                &module_known_functions,
                &effective_closure_functions,
                &effective_return_alias_summaries,
                emit_traces,
                &effective_leaf_functions,
                &effective_function_arities,
                &effective_function_has_ret,
            );
            let func_elapsed = func_start.elapsed();
            if timing && func_elapsed.as_millis() > 500 {
                eprintln!("MOLT_BACKEND_TIMING: function `{func_name}` took {func_elapsed:.2?}");
            }
            if slowest_func.as_ref().is_none_or(|(_, d)| func_elapsed > *d) {
                slowest_func = Some((func_name, func_elapsed));
            }
            deferred_codegen_ops = deferred_codegen_ops.saturating_add(func_op_count);
            if should_flush_deferred_codegen(self.deferred_defines.len(), deferred_codegen_ops) {
                let deferred_count = self.deferred_defines.len();
                let flush_start = std::time::Instant::now();
                self.flush_deferred_defines();
                if timing {
                    let flush_elapsed = flush_start.elapsed();
                    eprintln!(
                        "MOLT_BACKEND_TIMING: bounded Cranelift flush ({deferred_count} functions, {deferred_codegen_ops} source ops) took {flush_elapsed:.2?}"
                    );
                }
                deferred_codegen_ops = 0;
            }
            compiled += 1;
            // Print progress at regular intervals, or every 500ms for
            // slow builds where individual functions take a long time.
            if (compiled as usize).is_multiple_of(progress_interval)
                || last_progress.elapsed().as_millis() >= 500
            {
                let pct = (compiled as f64 / func_count as f64 * 100.0) as u32;
                let elapsed = codegen_start.elapsed();
                eprintln!(
                    "MOLT_BACKEND: [{pct:3}%] compiled {compiled}/{func_count} functions ({elapsed:.1?} elapsed)"
                );
                last_progress = std::time::Instant::now();
            }
        }
        if timing {
            let codegen_elapsed = codegen_start.elapsed();
            eprintln!("MOLT_BACKEND_TIMING: Cranelift codegen took {codegen_elapsed:.2?}");
            if let Some((name, dur)) = &slowest_func {
                eprintln!("MOLT_BACKEND_TIMING: slowest function: `{name}` ({dur:.2?})");
            }
        }
        debug_assert_eq!(failed, 0, "native backend no longer soft-fails functions");
        // ── Parallel Cranelift compilation ────────────────────────
        // All functions were IR-built sequentially above (declarations
        // and Cranelift IR construction are not thread-safe), but actual
        // machine-code compilation (register allocation, instruction
        // selection, encoding) is deferred.  Flush them now in parallel.
        {
            let deferred_count = self.deferred_defines.len();
            if deferred_count > 0 {
                let deferred_ops = deferred_codegen_ops;
                let flush_start = std::time::Instant::now();
                self.flush_deferred_defines();
                if timing {
                    let flush_elapsed = flush_start.elapsed();
                    eprintln!(
                        "MOLT_BACKEND_TIMING: final Cranelift flush ({deferred_count} functions, {deferred_ops} source ops) took {flush_elapsed:.2?}"
                    );
                }
            }
        }
        // ── Per-app intrinsic resolver ────────────────────────────
        // Emit `molt_app_resolve_intrinsic` AFTER the main flush so every
        // intrinsic FuncId created by a direct call already exists in the module
        // (reused via `get_name`); only manifest intrinsics are address-taken
        // here. The main stub registers this resolver before `molt_runtime_init`,
        // so the runtime resolves intrinsics through it instead of the
        // staticlib's `resolve_symbol`, keeping `resolve_symbol`/
        // `resolve_core_symbol` native-unreachable for dead-stripping.
        //
        // Emit it ONCE per final binary, into the designated main application
        // object (`emit_app_intrinsic_resolver`). Stdlib-cache batch objects and
        // all-but-one program batch set this `false`, so there is no duplicate
        // `_molt_app_resolve_intrinsic` symbol at link. The threaded manifest
        // covers every name-resolved intrinsic across all objects, including
        // stdlib wrappers compiled into the separate stdlib cache object.
        if self.emit_app_intrinsic_resolver {
            self.emit_app_resolver_function(&app_intrinsic_manifest);
        }
        // ── Post-compilation: fail closed on declared-but-undefined exports.
        // These are always backend contract violations: either a call site
        // declared an impossible overload, a function was skipped, or codegen
        // failed to define a body.
        let mut undefined_exports = Vec::new();
        let declared: Vec<(String, cranelift_codegen::ir::Signature)> = self
            .module
            .declarations()
            .get_functions()
            .filter_map(|(_fid, decl)| {
                let name = decl.name.clone()?;
                if decl.linkage == cranelift_module::Linkage::Export
                    && !self.defined_func_names.contains(&name)
                {
                    Some((name, decl.signature.clone()))
                } else {
                    None
                }
            })
            .collect();
        for (name, sig) in declared {
            // In batched compilation, functions that exist in another batch
            // are valid imports for the linker to resolve at merge time.
            if !self.external_function_names.is_empty()
                && self.external_function_names.contains(&name)
            {
                self.module
                    .declare_function(&name, cranelift_module::Linkage::Import, &sig)
                    .unwrap_or_else(|err| {
                        panic!("failed to mark cross-batch function `{name}` as import: {err}")
                    });
                continue;
            }
            undefined_exports.push(name);
        }
        if !undefined_exports.is_empty() {
            undefined_exports.sort();
            panic!(
                "native backend left {} exported function declaration(s) undefined: {}",
                undefined_exports.len(),
                undefined_exports.join(", ")
            );
        }

        let emit_start = std::time::Instant::now();
        let SimpleBackend { module, .. } = self;
        #[cfg(target_os = "macos")]
        let mut product = module.finish();
        #[cfg(not(target_os = "macos"))]
        let product = module.finish();
        // Set MachO platform load command so ld doesn't emit
        // "no platform load command found" warnings on macOS.
        #[cfg(target_os = "macos")]
        {
            use cranelift_object::object::write::MachOBuildVersion;
            // Encode macOS 11.0.0 as minimum deployment target.
            // Version encoding: xxxx.yy.zz nibbles => 0x000B0000 = 11.0.0
            let mut bv = MachOBuildVersion::default();
            bv.platform = cranelift_object::object::macho::PLATFORM_MACOS;
            bv.minos = 0x000B_0000; // macOS 11.0.0
            bv.sdk = 0; // no SDK constraint
            product.object.set_macho_build_version(bv);
        }
        let bytes = product.emit().unwrap();
        if timing {
            let emit_elapsed = emit_start.elapsed();
            let total_elapsed = compile_start.elapsed();
            eprintln!("MOLT_BACKEND_TIMING: object emit took {emit_elapsed:.2?}");
            eprintln!(
                "MOLT_BACKEND_TIMING: total backend compile: {total_elapsed:.2?} \
                 ({func_count} functions, {total_ops} ops, {} bytes)",
                bytes.len()
            );
        }
        CompileOutput { bytes }
    }
}

#[cfg(feature = "native-backend")]
impl SimpleBackend {
    /// Emit the per-app intrinsic resolver `molt_app_resolve_intrinsic` into the
    /// user object as a compact, relocated **data table** plus a small O(log n)
    /// binary-search lookup, rather than a giant O(n) linear-scan function.
    ///
    /// Layout (all `Local`, so the linker dead-strips them when the resolver
    /// itself is unreferenced — e.g. WASM builds — and keeps only this object's
    /// table otherwise):
    ///
    /// * `molt_app_intrinsic_names`: the manifest names, sorted by unsigned-byte
    ///   lexicographic order and concatenated (no separators).
    /// * `molt_app_intrinsic_table`: N fixed-size 16-byte records, sorted to
    ///   match the names blob: `[name_off: u32][name_len: u32][func_ptr: u64]`.
    ///   Each `func_ptr` slot carries a single pointer relocation
    ///   (`ARM64_RELOC_UNSIGNED` / `R_X86_64_64` / `R_AARCH64_ABS64` /
    ///   `IMAGE_REL_AMD64_ADDR64`) to the intrinsic, emitted via
    ///   `DataDescription::write_function_addr`. This is the portable, scalable
    ///   relocation form — the linker applies thousands of these without the
    ///   21-bit ADRP / branch-range pressure of thousands of `func_addr`
    ///   instructions packed into one oversized function (the failure mode that
    ///   corrupted the Mach-O header).
    ///
    /// The intrinsic `FuncId`s are declared `Import` (reusing any declaration a
    /// direct call already created), so the linker resolves the pointer relocs
    /// against the runtime staticlib. Only manifest intrinsics are referenced, so
    /// `-dead_strip` / `--gc-sections` still removes every unused intrinsic once
    /// `resolve_symbol` / `resolve_core_symbol` are native-unreachable.
    ///
    /// ABI: `extern "C" fn(name_ptr: i64, name_len: i64) -> i64`. Returns the
    /// intrinsic function pointer as a `u64`, or 0 when the name is not in the
    /// manifest.
    fn emit_app_resolver_function(&mut self, manifest_names: &BTreeSet<String>) {
        const RESOLVER_NAME: &str = "molt_app_resolve_intrinsic";
        const RECORD_BYTES: usize = 16; // u32 name_off + u32 name_len + u64 func_ptr

        // Diagnostic-only (default off): emit the exact per-app intrinsic manifest
        // so size-reduction work can verify, deterministically and at the manifest
        // level (not just the final binary size), exactly which intrinsics the
        // reachability gate keeps. Mirrors the `MOLT_DUMP_*` diagnostic family.
        if std::env::var("MOLT_DUMP_INTRINSIC_MANIFEST").as_deref() == Ok("1") {
            eprintln!("MOLT_INTRINSIC_MANIFEST: count={}", manifest_names.len());
            for name in manifest_names {
                eprintln!("MOLT_INTRINSIC_MANIFEST: {name}");
            }
        }

        // Declare the exported resolver: (i64 name_ptr, i64 name_len) -> i64.
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64));
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        let resolver_id = self
            .module
            .declare_function(RESOLVER_NAME, Linkage::Export, &sig)
            .unwrap_or_else(|e| panic!("failed to declare {RESOLVER_NAME}: {e:?}"));

        // Pre-resolve a FuncId for every manifest intrinsic, reusing any
        // declaration created by a direct call (so we never re-declare with a
        // conflicting signature). The signature only matters when the name was
        // not already declared; the address is taken via a pointer relocation and
        // is signature-independent. `manifest_names` is a `BTreeSet`, so the
        // iteration order is already unsigned-byte lexicographic — exactly the
        // order the binary search requires.
        let mut canonical_sig = self.module.make_signature();
        canonical_sig.params.push(AbiParam::new(types::I64));
        canonical_sig.returns.push(AbiParam::new(types::I64));
        let mut entries: Vec<(&str, cranelift_module::FuncId)> =
            Vec::with_capacity(manifest_names.len());
        for name in manifest_names {
            let func_id = if let Some(cranelift_module::FuncOrDataId::Func(id)) =
                self.module.get_name(name)
            {
                id
            } else {
                self.module
                    .declare_function(name, Linkage::Import, &canonical_sig)
                    .unwrap_or_else(|e| {
                        panic!("app resolver: failed to declare intrinsic '{name}': {e:?}")
                    })
            };
            entries.push((name.as_str(), func_id));
        }
        let count = entries.len();

        // Build the names blob and the record table. The record table's
        // `func_ptr` slots are filled by relocations, not literal bytes; we
        // pre-size the blob with zeros and attach a `write_function_addr` reloc at
        // each slot offset.
        let mut names_blob: Vec<u8> = Vec::new();
        let mut table_blob: Vec<u8> = vec![0u8; count * RECORD_BYTES];
        let mut name_spans: Vec<(u32, u32)> = Vec::with_capacity(count);
        for (idx, (name, _)) in entries.iter().enumerate() {
            let off = names_blob.len();
            let bytes = name.as_bytes();
            assert!(
                off <= u32::MAX as usize && bytes.len() <= u32::MAX as usize,
                "app resolver: intrinsic name table exceeds u32 addressing"
            );
            names_blob.extend_from_slice(bytes);
            name_spans.push((off as u32, bytes.len() as u32));
            let rec = idx * RECORD_BYTES;
            table_blob[rec..rec + 4].copy_from_slice(&(off as u32).to_le_bytes());
            table_blob[rec + 4..rec + 8].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
            // bytes [rec+8 .. rec+16] (the func_ptr) stay zero; the relocation
            // supplies the address at link time.
        }

        // Declare and define the names blob (immutable, no relocations).
        let names_data_id = self
            .module
            .declare_data("molt_app_intrinsic_names", Linkage::Local, false, false)
            .unwrap_or_else(|e| panic!("app resolver: failed to declare names blob: {e:?}"));
        let mut names_desc = DataDescription::new();
        names_desc.define(names_blob.into_boxed_slice());
        self.module
            .define_data(names_data_id, &names_desc)
            .unwrap_or_else(|e| panic!("app resolver: failed to define names blob: {e:?}"));

        // Declare and define the record table with one pointer relocation per
        // func_ptr slot. `import_function` + `write_function_addr` emit a native
        // absolute-pointer relocation (8 bytes) — portable across Mach-O, ELF and
        // COFF — that the linker resolves to the intrinsic in the staticlib.
        let table_data_id = self
            .module
            .declare_data("molt_app_intrinsic_table", Linkage::Local, false, false)
            .unwrap_or_else(|e| panic!("app resolver: failed to declare table: {e:?}"));
        let mut table_desc = DataDescription::new();
        table_desc.set_align(8);
        table_desc.define(table_blob.into_boxed_slice());
        for (idx, (_, func_id)) in entries.iter().enumerate() {
            let func_ref = self.module.declare_func_in_data(*func_id, &mut table_desc);
            let slot = (idx * RECORD_BYTES + 8) as u32;
            table_desc.write_function_addr(slot, func_ref);
        }
        self.module
            .define_data(table_data_id, &table_desc)
            .unwrap_or_else(|e| panic!("app resolver: failed to define table: {e:?}"));

        // Build the lookup function body: binary search over the sorted record
        // table, comparing the query name against each candidate via an unsigned
        // byte-wise lexicographic compare.
        let mut ctx = self.module.make_context();
        ctx.func.signature = sig;
        let mut builder_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);

        let entry_block = builder.create_block();
        builder.append_block_params_for_function_params(entry_block);
        builder.switch_to_block(entry_block);
        builder.seal_block(entry_block);
        let name_ptr = builder.block_params(entry_block)[0];
        let name_len = builder.block_params(entry_block)[1];

        let not_found_block = builder.create_block();

        if count == 0 {
            // Empty manifest: the resolver always reports "not found". The table
            // and names blobs are still emitted (size 0) for layout uniformity.
            builder.ins().jump(not_found_block, &[]);
        } else {
            // Materialize the base addresses of the two data segments.
            let names_gv = self
                .module
                .declare_data_in_func(names_data_id, builder.func);
            let names_base = builder.ins().symbol_value(types::I64, names_gv);
            let table_gv = self
                .module
                .declare_data_in_func(table_data_id, builder.func);
            let table_base = builder.ins().symbol_value(types::I64, table_gv);

            // Binary-search loop: maintain a half-open range [lo, hi).
            //   loop_head(lo, hi): if lo >= hi -> not_found; else probe mid.
            let loop_head = builder.create_block();
            builder.append_block_param(loop_head, types::I64); // lo
            builder.append_block_param(loop_head, types::I64); // hi
            let zero = builder.ins().iconst(types::I64, 0);
            let count_val = builder.ins().iconst(types::I64, count as i64);
            jump_block(&mut builder, loop_head, &[zero, count_val]);

            builder.switch_to_block(loop_head);
            let lo = builder.block_params(loop_head)[0];
            let hi = builder.block_params(loop_head)[1];
            let probe_block = builder.create_block();
            let range_nonempty = builder.ins().icmp(IntCC::SignedLessThan, lo, hi);
            builder
                .ins()
                .brif(range_nonempty, probe_block, &[], not_found_block, &[]);

            // probe: mid = lo + (hi - lo) / 2; load record(mid); compare.
            builder.switch_to_block(probe_block);
            let span = builder.ins().isub(hi, lo);
            let half = builder.ins().ushr_imm(span, 1);
            let mid = builder.ins().iadd(lo, half);
            let rec_stride = builder.ins().iconst(types::I64, RECORD_BYTES as i64);
            let rec_off = builder.ins().imul(mid, rec_stride);
            let rec_ptr = builder.ins().iadd(table_base, rec_off);
            let flags = MemFlagsData::new();
            let cand_off32 = builder.ins().load(types::I32, flags, rec_ptr, 0);
            let cand_len32 = builder.ins().load(types::I32, flags, rec_ptr, 4);
            let cand_off = builder.ins().uextend(types::I64, cand_off32);
            let cand_len = builder.ins().uextend(types::I64, cand_len32);
            let cand_ptr = builder.ins().iadd(names_base, cand_off);

            // cmp = lexicographic_compare(query, candidate) in {-1, 0, 1}.
            let cmp = Self::emit_lexicographic_compare(
                &mut builder,
                name_ptr,
                name_len,
                cand_ptr,
                cand_len,
            );

            // cmp == 0 -> hit: load and return func_ptr at rec_ptr+8.
            let hit_block = builder.create_block();
            let go_left_or_right = builder.create_block();
            let is_eq = builder.ins().icmp_imm(IntCC::Equal, cmp, 0);
            builder
                .ins()
                .brif(is_eq, hit_block, &[], go_left_or_right, &[]);

            builder.switch_to_block(hit_block);
            builder.seal_block(hit_block);
            let func_ptr = builder.ins().load(types::I64, flags, rec_ptr, 8);
            builder.ins().return_(&[func_ptr]);

            // cmp < 0 -> search left half [lo, mid); else right half [mid+1, hi).
            builder.switch_to_block(go_left_or_right);
            builder.seal_block(go_left_or_right);
            let left_block = builder.create_block();
            let right_block = builder.create_block();
            let cmp_lt = builder.ins().icmp_imm(IntCC::SignedLessThan, cmp, 0);
            builder
                .ins()
                .brif(cmp_lt, left_block, &[], right_block, &[]);

            builder.switch_to_block(left_block);
            builder.seal_block(left_block);
            jump_block(&mut builder, loop_head, &[lo, mid]);

            builder.switch_to_block(right_block);
            builder.seal_block(right_block);
            let one = builder.ins().iconst(types::I64, 1);
            let mid_plus_1 = builder.ins().iadd(mid, one);
            jump_block(&mut builder, loop_head, &[mid_plus_1, hi]);

            builder.seal_block(probe_block);
            builder.seal_block(loop_head);
        }

        builder.switch_to_block(not_found_block);
        builder.seal_block(not_found_block);
        let zero_ret = builder.ins().iconst(types::I64, 0);
        builder.ins().return_(&[zero_ret]);

        builder.finalize();

        self.module
            .define_function(resolver_id, &mut ctx)
            .unwrap_or_else(|e| panic!("failed to define {RESOLVER_NAME}: {e:?}"));
        self.defined_func_names.insert(RESOLVER_NAME.to_string());
    }

    /// Emit an unsigned byte-wise lexicographic comparison of two runtime byte
    /// ranges `(a_ptr, a_len)` and `(b_ptr, b_len)`, returning an `I64` in
    /// `{-1, 0, 1}` (a<b, a==b, a>b) — the same ordering `BTreeSet<String>` uses
    /// to sort the table, so binary search is consistent.
    ///
    /// The compare loop walks `min(a_len, b_len)` bytes; on the first differing
    /// byte it returns the sign of the unsigned difference, and on a common
    /// prefix it returns the sign of `a_len - b_len`. All loads stay within their
    /// respective `[0, len)` ranges.
    fn emit_lexicographic_compare(
        builder: &mut FunctionBuilder,
        a_ptr: Value,
        a_len: Value,
        b_ptr: Value,
        b_len: Value,
    ) -> Value {
        let flags = MemFlagsData::new();
        let neg_one = builder.ins().iconst(types::I64, -1);
        let zero = builder.ins().iconst(types::I64, 0);
        let one = builder.ins().iconst(types::I64, 1);

        // min_len = min(a_len, b_len)
        let a_lt_b_len = builder.ins().icmp(IntCC::UnsignedLessThan, a_len, b_len);
        let min_len = builder.ins().select(a_lt_b_len, a_len, b_len);

        // Loop over i in [0, min_len). loop_head(i): if i>=min_len break to tail.
        let loop_head = builder.create_block();
        builder.append_block_param(loop_head, types::I64); // i
        let body_block = builder.create_block();
        let tail_block = builder.create_block();
        let ret_block = builder.create_block();
        builder.append_block_param(ret_block, types::I64); // result
        jump_block(builder, loop_head, &[zero]);

        builder.switch_to_block(loop_head);
        let i = builder.block_params(loop_head)[0];
        let in_range = builder.ins().icmp(IntCC::UnsignedLessThan, i, min_len);
        builder
            .ins()
            .brif(in_range, body_block, &[], tail_block, &[]);

        // body: compare bytes at offset i.
        builder.switch_to_block(body_block);
        builder.seal_block(body_block);
        let a_addr = builder.ins().iadd(a_ptr, i);
        let b_addr = builder.ins().iadd(b_ptr, i);
        let a_byte = builder.ins().uload8(types::I64, flags, a_addr, 0);
        let b_byte = builder.ins().uload8(types::I64, flags, b_addr, 0);
        let bytes_eq = builder.ins().icmp(IntCC::Equal, a_byte, b_byte);
        let advance_block = builder.create_block();
        let diff_block = builder.create_block();
        builder
            .ins()
            .brif(bytes_eq, advance_block, &[], diff_block, &[]);

        // advance: i += 1, continue.
        builder.switch_to_block(advance_block);
        builder.seal_block(advance_block);
        let next_i = builder.ins().iadd(i, one);
        jump_block(builder, loop_head, &[next_i]);

        // diff: bytes differ — sign of (a_byte - b_byte).
        builder.switch_to_block(diff_block);
        builder.seal_block(diff_block);
        let a_lt_b = builder.ins().icmp(IntCC::UnsignedLessThan, a_byte, b_byte);
        let diff_sign = builder.ins().select(a_lt_b, neg_one, one);
        jump_block(builder, ret_block, &[diff_sign]);

        // tail: common prefix equal — order by length.
        builder.switch_to_block(tail_block);
        builder.seal_block(tail_block);
        let len_lt = builder.ins().icmp(IntCC::UnsignedLessThan, a_len, b_len);
        let len_gt = builder.ins().icmp(IntCC::UnsignedGreaterThan, a_len, b_len);
        let lt_or_zero = builder.ins().select(len_lt, neg_one, zero);
        let tail_result = builder.ins().select(len_gt, one, lt_or_zero);
        jump_block(builder, ret_block, &[tail_result]);

        builder.seal_block(loop_head);

        builder.switch_to_block(ret_block);
        builder.seal_block(ret_block);
        builder.block_params(ret_block)[0]
    }

    pub(crate) fn ensure_trampoline(
        module: &mut ObjectModule,
        trampoline_ids: &mut BTreeMap<TrampolineKey, cranelift_module::FuncId>,
        func_name: &str,
        linkage: Linkage,
        spec: TrampolineSpec,
    ) -> cranelift_module::FuncId {
        let TrampolineSpec {
            arity,
            has_closure,
            kind,
            closure_size,
            target_has_ret,
        } = spec;
        let is_import = matches!(linkage, Linkage::Import);
        let key = TrampolineKey {
            name: func_name.to_string(),
            arity,
            has_closure,
            is_import,
            kind,
            closure_size,
            target_has_ret,
        };
        if let Some(id) = trampoline_ids.get(&key) {
            return *id;
        }
        let closure_suffix = if has_closure { "_closure" } else { "" };
        let import_suffix = if is_import { "_import" } else { "" };
        let ret_suffix = if target_has_ret { "" } else { "_void" };
        let kind_suffix = match kind {
            TrampolineKind::Plain => "",
            TrampolineKind::Generator => "_gen",
            TrampolineKind::Coroutine => "_coro",
            TrampolineKind::AsyncGen => "_asyncgen",
        };
        let trampoline_name = format!(
            "{func_name}__molt_trampoline_{arity}{closure_suffix}{kind_suffix}{ret_suffix}{import_suffix}"
        );
        let mut ctx = module.make_context();
        ctx.func.signature.params.push(AbiParam::new(types::I64));
        ctx.func.signature.params.push(AbiParam::new(types::I64));
        ctx.func.signature.params.push(AbiParam::new(types::I64));
        ctx.func.signature.returns.push(AbiParam::new(types::I64));

        let mut builder_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
        let entry_block = builder.create_block();
        builder.append_block_params_for_function_params(entry_block);
        builder.switch_to_block(entry_block);
        builder.seal_block(entry_block);
        let nbc = NanBoxConsts::new(&mut builder);

        let closure_bits = builder.block_params(entry_block)[0];
        let args_ptr = builder.block_params(entry_block)[1];
        let _args_len = builder.block_params(entry_block)[2];

        let poll_target = if matches!(
            kind,
            TrampolineKind::Generator | TrampolineKind::Coroutine | TrampolineKind::AsyncGen
        ) {
            if func_name.ends_with("_poll") {
                func_name.to_string()
            } else {
                format!("{func_name}_poll")
            }
        } else {
            String::new()
        };

        match kind {
            TrampolineKind::Generator => {
                if closure_size < 0 {
                    panic!("generator closure size must be non-negative");
                }
                let payload_slots = arity + usize::from(has_closure);
                let needed = GENERATOR_CONTROL_BYTES as i64 + (payload_slots as i64) * 8;
                if closure_size < needed {
                    panic!("generator closure size too small for trampoline");
                }

                let mut inc_ref_obj_sig = module.make_signature();
                inc_ref_obj_sig.params.push(AbiParam::new(types::I64));
                let inc_ref_obj_callee = module
                    .declare_function("molt_inc_ref_obj", Linkage::Import, &inc_ref_obj_sig)
                    .unwrap();
                let local_inc_ref_obj =
                    module.declare_func_in_func(inc_ref_obj_callee, builder.func);

                let mut poll_sig = module.make_signature();
                poll_sig.params.push(AbiParam::new(types::I64));
                poll_sig.returns.push(AbiParam::new(types::I64));
                let poll_id = module
                    .declare_function(&poll_target, Linkage::Import, &poll_sig)
                    .unwrap();
                let poll_ref = module.declare_func_in_func(poll_id, builder.func);
                let poll_addr = builder.ins().func_addr(types::I64, poll_ref);

                let mut task_sig = module.make_signature();
                task_sig.params.push(AbiParam::new(types::I64));
                task_sig.params.push(AbiParam::new(types::I64));
                task_sig.params.push(AbiParam::new(types::I64));
                task_sig.returns.push(AbiParam::new(types::I64));
                let task_callee = module
                    .declare_function("molt_task_new", Linkage::Import, &task_sig)
                    .unwrap();
                let task_local = module.declare_func_in_func(task_callee, builder.func);
                let size_val = builder.ins().iconst(types::I64, closure_size);
                let kind_val = builder.ins().iconst(types::I64, TASK_KIND_GENERATOR);
                let call = builder
                    .ins()
                    .call(task_local, &[poll_addr, size_val, kind_val]);
                let obj = builder.inst_results(call)[0];
                let obj_ptr = unbox_ptr_value(&mut builder, obj, &nbc);

                let mut offset = GENERATOR_CONTROL_BYTES;
                if has_closure {
                    builder
                        .ins()
                        .store(MemFlagsData::trusted(), closure_bits, obj_ptr, offset);
                    builder.ins().call(local_inc_ref_obj, &[closure_bits]);
                    offset += 8;
                }
                for idx in 0..arity {
                    let arg_offset = (idx * std::mem::size_of::<u64>()) as i32;
                    let arg_val =
                        builder
                            .ins()
                            .load(types::I64, MemFlagsData::trusted(), args_ptr, arg_offset);
                    builder
                        .ins()
                        .store(MemFlagsData::trusted(), arg_val, obj_ptr, offset + arg_offset);
                    builder.ins().call(local_inc_ref_obj, &[arg_val]);
                }
                builder.ins().return_(&[obj]);
            }
            TrampolineKind::Coroutine => {
                if closure_size < 0 {
                    panic!("coroutine closure size must be non-negative");
                }
                let payload_slots = arity + usize::from(has_closure);
                let needed = (payload_slots as i64) * 8;
                if closure_size < needed {
                    panic!("coroutine closure size too small for trampoline");
                }

                let mut poll_sig = module.make_signature();
                poll_sig.params.push(AbiParam::new(types::I64));
                poll_sig.returns.push(AbiParam::new(types::I64));
                let poll_id = module
                    .declare_function(&poll_target, Linkage::Import, &poll_sig)
                    .unwrap();
                let poll_ref = module.declare_func_in_func(poll_id, builder.func);
                let poll_addr = builder.ins().func_addr(types::I64, poll_ref);

                let mut task_sig = module.make_signature();
                task_sig.params.push(AbiParam::new(types::I64));
                task_sig.params.push(AbiParam::new(types::I64));
                task_sig.params.push(AbiParam::new(types::I64));
                task_sig.returns.push(AbiParam::new(types::I64));
                let task_callee = module
                    .declare_function("molt_task_new", Linkage::Import, &task_sig)
                    .unwrap();
                let task_local = module.declare_func_in_func(task_callee, builder.func);
                let size_val = builder.ins().iconst(types::I64, closure_size);
                let kind_val = builder.ins().iconst(types::I64, TASK_KIND_COROUTINE);
                let call = builder
                    .ins()
                    .call(task_local, &[poll_addr, size_val, kind_val]);
                let obj = builder.inst_results(call)[0];
                if payload_slots > 0 {
                    let mut inc_ref_obj_sig = module.make_signature();
                    inc_ref_obj_sig.params.push(AbiParam::new(types::I64));
                    let inc_ref_obj_callee = module
                        .declare_function("molt_inc_ref_obj", Linkage::Import, &inc_ref_obj_sig)
                        .unwrap();
                    let local_inc_ref_obj =
                        module.declare_func_in_func(inc_ref_obj_callee, builder.func);
                    let obj_ptr = unbox_ptr_value(&mut builder, obj, &nbc);

                    let mut offset = 0i32;
                    if has_closure {
                        builder
                            .ins()
                            .store(MemFlagsData::trusted(), closure_bits, obj_ptr, offset);
                        builder.ins().call(local_inc_ref_obj, &[closure_bits]);
                        offset += 8;
                    }
                    for idx in 0..arity {
                        let arg_offset = (idx * std::mem::size_of::<u64>()) as i32;
                        let arg_val = builder.ins().load(
                            types::I64,
                            MemFlagsData::trusted(),
                            args_ptr,
                            arg_offset,
                        );
                        builder.ins().store(
                            MemFlagsData::trusted(),
                            arg_val,
                            obj_ptr,
                            offset + arg_offset,
                        );
                        builder.ins().call(local_inc_ref_obj, &[arg_val]);
                    }
                }

                let mut get_sig = module.make_signature();
                get_sig.returns.push(AbiParam::new(types::I64));
                let get_callee = module
                    .declare_function("molt_cancel_token_get_current", Linkage::Import, &get_sig)
                    .unwrap();
                let get_local = module.declare_func_in_func(get_callee, builder.func);
                let get_call = builder.ins().call(get_local, &[]);
                let current_token = builder.inst_results(get_call)[0];

                let mut reg_sig = module.make_signature();
                reg_sig.params.push(AbiParam::new(types::I64));
                reg_sig.params.push(AbiParam::new(types::I64));
                reg_sig.returns.push(AbiParam::new(types::I64));
                let reg_callee = module
                    .declare_function("molt_task_register_token_owned", Linkage::Import, &reg_sig)
                    .unwrap();
                let reg_local = module.declare_func_in_func(reg_callee, builder.func);
                builder.ins().call(reg_local, &[obj, current_token]);

                builder.ins().return_(&[obj]);
            }
            TrampolineKind::AsyncGen => {
                if closure_size < 0 {
                    panic!("async generator closure size must be non-negative");
                }
                let payload_slots = arity + usize::from(has_closure);
                let needed = GENERATOR_CONTROL_BYTES as i64 + (payload_slots as i64) * 8;
                if closure_size < needed {
                    panic!("async generator closure size too small for trampoline");
                }

                let mut inc_ref_obj_sig = module.make_signature();
                inc_ref_obj_sig.params.push(AbiParam::new(types::I64));
                let inc_ref_obj_callee = module
                    .declare_function("molt_inc_ref_obj", Linkage::Import, &inc_ref_obj_sig)
                    .unwrap();
                let local_inc_ref_obj =
                    module.declare_func_in_func(inc_ref_obj_callee, builder.func);

                let mut poll_sig = module.make_signature();
                poll_sig.params.push(AbiParam::new(types::I64));
                poll_sig.returns.push(AbiParam::new(types::I64));
                let poll_id = module
                    .declare_function(&poll_target, Linkage::Import, &poll_sig)
                    .unwrap();
                let poll_ref = module.declare_func_in_func(poll_id, builder.func);
                let poll_addr = builder.ins().func_addr(types::I64, poll_ref);

                let mut task_sig = module.make_signature();
                task_sig.params.push(AbiParam::new(types::I64));
                task_sig.params.push(AbiParam::new(types::I64));
                task_sig.params.push(AbiParam::new(types::I64));
                task_sig.returns.push(AbiParam::new(types::I64));
                let task_callee = module
                    .declare_function("molt_task_new", Linkage::Import, &task_sig)
                    .unwrap();
                let task_local = module.declare_func_in_func(task_callee, builder.func);
                let size_val = builder.ins().iconst(types::I64, closure_size);
                let kind_val = builder.ins().iconst(types::I64, TASK_KIND_GENERATOR);
                let call = builder
                    .ins()
                    .call(task_local, &[poll_addr, size_val, kind_val]);
                let obj = builder.inst_results(call)[0];
                let obj_ptr = unbox_ptr_value(&mut builder, obj, &nbc);

                let mut offset = GENERATOR_CONTROL_BYTES;
                if has_closure {
                    builder
                        .ins()
                        .store(MemFlagsData::trusted(), closure_bits, obj_ptr, offset);
                    builder.ins().call(local_inc_ref_obj, &[closure_bits]);
                    offset += 8;
                }
                for idx in 0..arity {
                    let arg_offset = (idx * std::mem::size_of::<u64>()) as i32;
                    let arg_val =
                        builder
                            .ins()
                            .load(types::I64, MemFlagsData::trusted(), args_ptr, arg_offset);
                    builder
                        .ins()
                        .store(MemFlagsData::trusted(), arg_val, obj_ptr, offset + arg_offset);
                    builder.ins().call(local_inc_ref_obj, &[arg_val]);
                }

                let mut asyncgen_sig = module.make_signature();
                asyncgen_sig.params.push(AbiParam::new(types::I64));
                asyncgen_sig.returns.push(AbiParam::new(types::I64));
                let asyncgen_callee = module
                    .declare_function("molt_asyncgen_new", Linkage::Import, &asyncgen_sig)
                    .unwrap();
                let asyncgen_local = module.declare_func_in_func(asyncgen_callee, builder.func);
                let asyncgen_call = builder.ins().call(asyncgen_local, &[obj]);
                let asyncgen_obj = builder.inst_results(asyncgen_call)[0];
                builder.ins().return_(&[asyncgen_obj]);
            }
            TrampolineKind::Plain => {
                let mut call_args = Vec::with_capacity(arity + if has_closure { 1 } else { 0 });
                if has_closure {
                    call_args.push(closure_bits);
                }
                for idx in 0..arity {
                    let offset = (idx * std::mem::size_of::<u64>()) as i32;
                    let arg_val =
                        builder
                            .ins()
                            .load(types::I64, MemFlagsData::trusted(), args_ptr, offset);
                    call_args.push(arg_val);
                }

                let mut target_sig = module.make_signature();
                if has_closure {
                    target_sig.params.push(AbiParam::new(types::I64));
                }
                for _ in 0..arity {
                    target_sig.params.push(AbiParam::new(types::I64));
                }
                if target_has_ret {
                    target_sig.returns.push(AbiParam::new(types::I64));
                }
                // Always use Import for the target function inside
                // trampolines: the target is defined by its own
                // compile_func call (Export), and in batched compilation
                // the target may be in a different batch .o file.
                let target_id = module
                    .declare_function(func_name, Linkage::Import, &target_sig)
                    .unwrap();
                let target_ref = module.declare_func_in_func(target_id, builder.func);
                let call = builder.ins().call(target_ref, &call_args);
                if target_has_ret {
                    let res = builder.inst_results(call)[0];
                    builder.ins().return_(&[res]);
                } else {
                    let none_val = builder.ins().iconst(types::I64, box_none());
                    builder.ins().return_(&[none_val]);
                }
            }
        }

        builder.seal_all_blocks();
        builder.finalize();

        let trampoline_id = module
            .declare_function(&trampoline_name, Linkage::Local, &ctx.func.signature)
            .unwrap();
        if let Err(err) = module.define_function(trampoline_id, &mut ctx) {
            panic!("Failed to define trampoline {trampoline_name}: {err:?}");
        }
        trampoline_ids.insert(key, trampoline_id);
        trampoline_id
    }
}

#[cfg(all(test, feature = "native-backend"))]
mod tests {
    use super::{
        DEFERRED_CODEGEN_FLUSH_FUNCTION_LIMIT, DEFERRED_CODEGEN_FLUSH_OP_BUDGET,
        NativeBackendModuleContext, SimpleBackend, TIR_OPTIMIZATION_BASELINE_MEMORY_BYTES,
        TIR_OPTIMIZATION_BATCH_FUNCTION_LIMIT, TIR_OPTIMIZATION_BATCH_OP_BUDGET,
        TIR_OPTIMIZATION_WAVE_FUNCTIONS_PER_THREAD, TIR_OPTIMIZATION_WAVE_OPS_PER_THREAD,
        TIR_OPTIMIZATION_WORKER_MEMORY_BYTES, TirOptimizationWorkItem, TrampolineKey,
        analyze_native_backend_ir, compute_function_has_ret, drain_cleanup_entry_tracked,
        drain_cleanup_entry_tracked_with_authority, drain_cleanup_tracked_dedup_with_authority,
        merge_closure_functions, merge_function_arities, merge_function_has_ret,
        merge_leaf_functions, merge_task_kinds, partition_tir_optimization_work_items,
        partition_tir_optimization_work_items_with_limits, should_flush_deferred_codegen,
        tir_optimization_resource_plan_from_limits,
    };
    use crate::TrampolineKind;
    use crate::ir::{FunctionIR, OpIR, SimpleIR};
    use crate::ir_rewrites::{elide_useless_try_blocks, elide_useless_try_blocks_for_function};
    use crate::passes::ReturnAliasSummary;
    use crate::rewrite_phi_to_store_load;
    use cranelift_codegen::ir::Value;
    use cranelift_codegen::ir::types;
    use cranelift_module::Module;
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::{Mutex, OnceLock};

    fn backend_env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    /// Acquire the process-env serialization lock, tolerating a poisoned mutex.
    ///
    /// The guarded value is `()` — the lock exists only to *serialize* tests
    /// that mutate process-global env vars (`MOLT_BACKEND`, `MOLT_STDLIB_OBJ`,
    /// …) so they do not race. Each such test snapshots and restores the env
    /// vars it touches itself; the mutex protects no shared in-memory invariant.
    /// When one test panics while holding the guard, the mutex is *poisoned*,
    /// but there is no corrupted state to guard against — the only thing the
    /// poison flag would do is convert that single panic into a cascade of
    /// `PoisonError` panics in every later test that takes the lock, hiding the
    /// real failure behind noise. Recovering the guard via `into_inner()` keeps
    /// the mutual-exclusion guarantee intact while letting the genuine failure
    /// stand alone. This is the textbook-sound use of poison recovery: the
    /// protected data carries no invariant the poison could have broken.
    fn acquire_backend_env_lock() -> std::sync::MutexGuard<'static, ()> {
        backend_env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[test]
    fn tir_optimization_work_partition_respects_count_and_op_budgets() {
        let by_count: Vec<TirOptimizationWorkItem> = (0..(TIR_OPTIMIZATION_BATCH_FUNCTION_LIMIT
            + 1))
            .map(|index| TirOptimizationWorkItem {
                index,
                content_hash: format!("hash-{index}"),
                op_count: 1,
            })
            .collect();
        let count_batches = partition_tir_optimization_work_items(by_count);
        assert_eq!(count_batches.len(), 2);
        assert_eq!(
            count_batches[0].len(),
            TIR_OPTIMIZATION_BATCH_FUNCTION_LIMIT
        );
        assert_eq!(count_batches[1].len(), 1);

        let by_ops = vec![
            TirOptimizationWorkItem {
                index: 0,
                content_hash: "a".to_string(),
                op_count: TIR_OPTIMIZATION_BATCH_OP_BUDGET / 2,
            },
            TirOptimizationWorkItem {
                index: 1,
                content_hash: "b".to_string(),
                op_count: TIR_OPTIMIZATION_BATCH_OP_BUDGET / 2,
            },
            TirOptimizationWorkItem {
                index: 2,
                content_hash: "c".to_string(),
                op_count: 1,
            },
        ];
        let op_batches = partition_tir_optimization_work_items(by_ops);
        assert_eq!(op_batches.len(), 2);
        assert_eq!(
            op_batches[0]
                .iter()
                .map(|item| item.op_count)
                .sum::<usize>(),
            TIR_OPTIMIZATION_BATCH_OP_BUDGET
        );
        assert_eq!(op_batches[1][0].index, 2);
    }

    #[test]
    fn tir_optimization_work_partition_accepts_inflight_limits() {
        let work: Vec<TirOptimizationWorkItem> = (0..5)
            .map(|index| TirOptimizationWorkItem {
                index,
                content_hash: format!("hash-{index}"),
                op_count: 3,
            })
            .collect();

        let waves = partition_tir_optimization_work_items_with_limits(work, 2, 6);

        assert_eq!(waves.len(), 3);
        assert_eq!(waves[0].len(), 2);
        assert_eq!(waves[1].len(), 2);
        assert_eq!(waves[2].len(), 1);
        assert!(
            waves
                .iter()
                .all(|wave| wave.iter().map(|item| item.op_count).sum::<usize>() <= 6)
        );
    }

    #[test]
    fn tir_optimization_resource_plan_caps_inflight_work_by_memory_limit() {
        let memory_limit =
            TIR_OPTIMIZATION_BASELINE_MEMORY_BYTES + (2 * TIR_OPTIMIZATION_WORKER_MEMORY_BYTES);

        let plan = tir_optimization_resource_plan_from_limits(8, Some(memory_limit));

        assert_eq!(plan.threads, 2);
        assert_eq!(
            plan.wave_function_limit,
            2 * TIR_OPTIMIZATION_WAVE_FUNCTIONS_PER_THREAD
        );
        assert_eq!(
            plan.wave_op_budget,
            2 * TIR_OPTIMIZATION_WAVE_OPS_PER_THREAD
        );
    }

    #[test]
    fn tir_optimization_resource_plan_serializes_under_twelve_gb_guard() {
        let memory_limit = 12 * 1024 * 1024 * 1024;

        let plan = tir_optimization_resource_plan_from_limits(8, Some(memory_limit));

        assert_eq!(plan.threads, 1);
        assert_eq!(plan.wave_function_limit, 1);
        assert_eq!(plan.wave_op_budget, TIR_OPTIMIZATION_WAVE_OPS_PER_THREAD);
    }

    #[test]
    fn tir_optimization_resource_plan_keeps_cpu_parallelism_without_memory_limit() {
        let plan = tir_optimization_resource_plan_from_limits(3, None);

        assert_eq!(plan.threads, 3);
        assert_eq!(
            plan.wave_function_limit,
            3 * TIR_OPTIMIZATION_WAVE_FUNCTIONS_PER_THREAD
        );
        assert_eq!(
            plan.wave_op_budget,
            3 * TIR_OPTIMIZATION_WAVE_OPS_PER_THREAD
        );
    }

    #[test]
    fn deferred_codegen_flush_predicate_bounds_function_and_op_retention() {
        assert!(!should_flush_deferred_codegen(
            0,
            DEFERRED_CODEGEN_FLUSH_OP_BUDGET
        ));
        assert!(!should_flush_deferred_codegen(
            DEFERRED_CODEGEN_FLUSH_FUNCTION_LIMIT - 1,
            DEFERRED_CODEGEN_FLUSH_OP_BUDGET - 1
        ));
        assert!(should_flush_deferred_codegen(
            DEFERRED_CODEGEN_FLUSH_FUNCTION_LIMIT,
            1
        ));
        assert!(should_flush_deferred_codegen(
            1,
            DEFERRED_CODEGEN_FLUSH_OP_BUDGET
        ));
    }

    fn op_shapes(
        ops: &[OpIR],
    ) -> Vec<(
        String,
        Option<i64>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<Vec<String>>,
    )> {
        ops.iter()
            .map(|op| {
                (
                    op.kind.clone(),
                    op.value,
                    op.out.clone(),
                    op.var.clone(),
                    op.s_value.clone(),
                    op.args.clone(),
                )
            })
            .collect()
    }

    #[test]
    fn try_elision_preserves_try_finally_cleanup_shape() {
        let mut ops = vec![
            OpIR {
                kind: "exception_push".to_string(),
                out: Some("none".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "try_start".to_string(),
                value: Some(10),
                ..OpIR::default()
            },
            OpIR {
                kind: "line".to_string(),
                value: Some(1),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".to_string(),
                value: Some(10),
                ..OpIR::default()
            },
            OpIR {
                kind: "try_end".to_string(),
                value: Some(10),
                ..OpIR::default()
            },
            OpIR {
                kind: "jump".to_string(),
                value: Some(11),
                ..OpIR::default()
            },
            OpIR {
                kind: "label".to_string(),
                value: Some(10),
                ..OpIR::default()
            },
            OpIR {
                kind: "label".to_string(),
                value: Some(11),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_none".to_string(),
                out: Some("finally_value".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "exception_pop".to_string(),
                out: Some("none".to_string()),
                ..OpIR::default()
            },
        ];

        let original = ops.clone();
        elide_useless_try_blocks(&mut ops);

        assert_eq!(
            op_shapes(&ops),
            op_shapes(&original),
            "try/finally must not use the try/except-only elision path"
        );
    }

    #[test]
    fn try_except_elision_drops_body_checks_to_removed_handler_labels() {
        let mut ops = vec![
            OpIR {
                kind: "exception_push".to_string(),
                out: Some("none".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "try_start".to_string(),
                value: Some(10),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".to_string(),
                value: Some(1),
                out: Some("x".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".to_string(),
                value: Some(10),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some("total".to_string()),
                args: Some(vec!["x".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "try_end".to_string(),
                value: Some(10),
                ..OpIR::default()
            },
            OpIR {
                kind: "jump".to_string(),
                value: Some(11),
                ..OpIR::default()
            },
            OpIR {
                kind: "label".to_string(),
                value: Some(10),
                ..OpIR::default()
            },
            OpIR {
                kind: "exception_last".to_string(),
                out: Some("exc".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "exception_match_builtin".to_string(),
                value: Some(3),
                s_value: Some("ValueError".to_string()),
                args: Some(vec!["exc".to_string()]),
                out: Some("matched".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "label".to_string(),
                value: Some(11),
                ..OpIR::default()
            },
            OpIR {
                kind: "exception_pop".to_string(),
                out: Some("none".to_string()),
                ..OpIR::default()
            },
        ];

        elide_useless_try_blocks(&mut ops);

        let kinds: Vec<&str> = ops.iter().map(|op| op.kind.as_str()).collect();
        assert_eq!(kinds, vec!["const", "store_var"]);
        assert!(
            ops.iter()
                .all(|op| !(op.kind == "check_exception" && op.value == Some(10))),
            "eliding a safe try/except body must not leave stale handler checks: {ops:?}"
        );
    }

    #[test]
    fn try_except_elision_keeps_transport_hinted_unknown_add() {
        let mut add = OpIR {
            kind: "add".to_string(),
            args: Some(vec!["left".to_string(), "right".to_string()]),
            out: Some("sum".to_string()),
            ..OpIR::default()
        };
        add.fast_int = Some(true);
        let mut func = FunctionIR {
            name: "transport_hint_try_body".to_string(),
            params: vec!["left".to_string(), "right".to_string()],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "exception_push".to_string(),
                    out: Some("none".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "try_start".to_string(),
                    value: Some(10),
                    ..OpIR::default()
                },
                add,
                OpIR {
                    kind: "check_exception".to_string(),
                    value: Some(10),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store_var".to_string(),
                    var: Some("result".to_string()),
                    args: Some(vec!["sum".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "try_end".to_string(),
                    value: Some(10),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "jump".to_string(),
                    value: Some(11),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "label".to_string(),
                    value: Some(10),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_last".to_string(),
                    out: Some("exc".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_match_builtin".to_string(),
                    value: Some(3),
                    s_value: Some("ValueError".to_string()),
                    args: Some(vec!["exc".to_string()]),
                    out: Some("matched".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "label".to_string(),
                    value: Some(11),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_pop".to_string(),
                    out: Some("none".to_string()),
                    ..OpIR::default()
                },
            ],
        };

        elide_useless_try_blocks_for_function(&mut func);

        assert!(
            func.ops.iter().any(|op| op.kind == "exception_push")
                && func.ops.iter().any(|op| op.kind == "add"),
            "transport hints alone must not elide try/except around Python arithmetic: {:?}",
            func.ops
        );
    }

    #[test]
    fn try_except_elision_uses_typed_int_body_without_transport_hints() {
        let mut func = FunctionIR {
            name: "typed_int_try_body".to_string(),
            params: vec!["left".to_string(), "right".to_string()],
            param_types: Some(vec!["int".to_string(), "int".to_string()]),
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "exception_push".to_string(),
                    out: Some("none".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "try_start".to_string(),
                    value: Some(10),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "add".to_string(),
                    args: Some(vec!["left".to_string(), "right".to_string()]),
                    out: Some("sum".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".to_string(),
                    value: Some(10),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store_var".to_string(),
                    var: Some("result".to_string()),
                    args: Some(vec!["sum".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "try_end".to_string(),
                    value: Some(10),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "jump".to_string(),
                    value: Some(11),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "label".to_string(),
                    value: Some(10),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_last".to_string(),
                    out: Some("exc".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_match_builtin".to_string(),
                    value: Some(3),
                    s_value: Some("ValueError".to_string()),
                    args: Some(vec!["exc".to_string()]),
                    out: Some("matched".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "label".to_string(),
                    value: Some(11),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_pop".to_string(),
                    out: Some("none".to_string()),
                    ..OpIR::default()
                },
            ],
        };

        elide_useless_try_blocks_for_function(&mut func);

        let kinds: Vec<&str> = func.ops.iter().map(|op| op.kind.as_str()).collect();
        assert_eq!(kinds, vec!["add", "store_var"]);
    }

    #[test]
    fn try_except_elision_aborts_when_body_branches_to_removed_wrapper_label() {
        let mut ops = vec![
            OpIR {
                kind: "exception_push".to_string(),
                out: Some("none".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "try_start".to_string(),
                value: Some(10),
                ..OpIR::default()
            },
            OpIR {
                kind: "jump".to_string(),
                value: Some(11),
                ..OpIR::default()
            },
            OpIR {
                kind: "try_end".to_string(),
                value: Some(10),
                ..OpIR::default()
            },
            OpIR {
                kind: "jump".to_string(),
                value: Some(11),
                ..OpIR::default()
            },
            OpIR {
                kind: "label".to_string(),
                value: Some(10),
                ..OpIR::default()
            },
            OpIR {
                kind: "exception_last".to_string(),
                out: Some("exc".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "exception_match_builtin".to_string(),
                value: Some(3),
                s_value: Some("ValueError".to_string()),
                args: Some(vec!["exc".to_string()]),
                out: Some("matched".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "label".to_string(),
                value: Some(11),
                ..OpIR::default()
            },
            OpIR {
                kind: "exception_pop".to_string(),
                out: Some("none".to_string()),
                ..OpIR::default()
            },
        ];

        let original = ops.clone();
        elide_useless_try_blocks(&mut ops);

        assert_eq!(
            op_shapes(&ops),
            op_shapes(&original),
            "try/except elision must be CFG-closed when wrapper-local labels are removed"
        );
    }

    fn compile_trace_probe_object(emit_traces_env: Option<&str>) -> Vec<u8> {
        let _guard = acquire_backend_env_lock();
        match emit_traces_env {
            Some(value) => unsafe { std::env::set_var("MOLT_BACKEND_EMIT_TRACES", value) },
            None => unsafe { std::env::remove_var("MOLT_BACKEND_EMIT_TRACES") },
        }
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![
                    OpIR {
                        kind: "trace_enter_slot".to_string(),
                        value: Some(7),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "trace_exit".to_string(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
            }],
            profile: None,
        };
        let output = SimpleBackend::new().compile(ir);
        unsafe { std::env::remove_var("MOLT_BACKEND_EMIT_TRACES") };
        output.bytes
    }

    fn compile_function_to_clif_text(functions: Vec<FunctionIR>, target_name: &str) -> String {
        let ir = SimpleIR {
            functions,
            profile: None,
        };
        let analysis = analyze_native_backend_ir(&ir, true);
        let function_has_ret = compute_function_has_ret(&ir.functions);
        let function_arities = ir
            .functions
            .iter()
            .map(|func| (func.name.clone(), func.params.len()))
            .collect();
        let return_alias_summaries = crate::passes::compute_return_alias_summaries(&ir.functions);
        let target_func = ir
            .functions
            .into_iter()
            .find(|func| func.name == target_name)
            .unwrap_or_else(|| panic!("missing target function `{target_name}`"));
        let mut backend = SimpleBackend::new();
        backend.compile_func(
            target_func,
            &analysis.task_kinds,
            &analysis.task_closure_sizes,
            &analysis.defined_functions,
            &analysis.defined_functions,
            &analysis.closure_functions,
            &return_alias_summaries,
            false,
            &analysis.leaf_functions,
            &function_arities,
            &function_has_ret,
        );
        backend
            .deferred_defines
            .iter()
            .find(|deferred| deferred.name == target_name)
            .unwrap_or_else(|| panic!("missing deferred function `{target_name}`"))
            .func
            .display()
            .to_string()
    }

    fn roundtrip_function_through_tir(func: &FunctionIR) -> FunctionIR {
        let mut tir = crate::tir::lower_from_simple::lower_to_tir(func);
        crate::tir::type_refine::refine_types(&mut tir);
        let _stats = crate::tir::passes::run_pipeline(
            &mut tir,
            &crate::tir::target_info::TargetInfo::native_release_fast(),
        );
        crate::tir::type_refine::refine_types(&mut tir);
        let lir = crate::tir::lower_to_lir::lower_function_to_lir(&tir, None);
        if let Err(errors) = crate::tir::verify_lir::verify_lir_function(&lir) {
            panic!("LIR verification failed after TIR optimization: {errors:#?}");
        }
        #[cfg(debug_assertions)]
        {
            let repr_violations = crate::tir::verify_lir_repr::verify_register_passable(&lir);
            if !repr_violations.is_empty() {
                eprintln!(
                    "[LIR-repr] {} register-passable violation(s) in '{}': {:?}",
                    repr_violations.len(),
                    func.name,
                    repr_violations,
                );
            }
        }
        let ops = crate::tir::lower_to_simple::lower_to_simple_ir(&tir);
        assert!(
            crate::tir::lower_to_simple::validate_labels(&ops),
            "TIR roundtrip must preserve all referenced labels: {ops:#?}"
        );
        FunctionIR {
            name: func.name.clone(),
            params: func.params.clone(),
            ops,
            param_types: func.param_types.clone(),
            source_file: func.source_file.clone(),
            is_extern: false,
        }
    }

    #[test]
    fn native_backend_skips_trace_imports_by_default() {
        let bytes = compile_trace_probe_object(None);

        assert!(
            !bytes
                .windows(b"molt_trace_enter_slot".len())
                .any(|window| window == b"molt_trace_enter_slot")
        );
        assert!(
            !bytes
                .windows(b"molt_trace_exit".len())
                .any(|window| window == b"molt_trace_exit")
        );
    }

    #[test]
    fn native_backend_can_opt_in_trace_imports() {
        let bytes = compile_trace_probe_object(Some("1"));

        assert!(
            bytes
                .windows(b"molt_trace_enter_slot".len())
                .any(|window| window == b"molt_trace_enter_slot")
        );
        assert!(
            bytes
                .windows(b"molt_trace_exit".len())
                .any(|window| window == b"molt_trace_exit")
        );
    }

    #[test]
    fn native_backend_ir_analysis_skips_inlining_without_internal_calls() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "ret".to_string(),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            }],
            profile: None,
        };

        let analysis = analyze_native_backend_ir(&ir, true);

        assert!(analysis.defined_functions.contains("molt_main"));
    }

    #[test]
    fn native_backend_ir_analysis_collects_task_metadata_once_needed() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![
                    OpIR {
                        kind: "const_bool".to_string(),
                        out: Some("flag".to_string()),
                        value: Some(1),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const".to_string(),
                        out: Some("closure_size".to_string()),
                        value: Some(3),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "func_new_closure".to_string(),
                        out: Some("poll_obj".to_string()),
                        s_value: Some("worker_poll".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "set_attr_generic_obj".to_string(),
                        s_value: Some("__molt_is_coroutine__".to_string()),
                        args: Some(vec!["poll_obj".to_string(), "flag".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "set_attr_generic_obj".to_string(),
                        s_value: Some("__molt_closure_size__".to_string()),
                        args: Some(vec!["poll_obj".to_string(), "closure_size".to_string()]),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
            }],
            profile: None,
        };

        let analysis = analyze_native_backend_ir(&ir, true);

        assert!(analysis.closure_functions.contains("worker_poll"));
        assert_eq!(
            analysis.task_kinds.get("worker_poll"),
            Some(&TrampolineKind::Coroutine)
        );
        assert_eq!(analysis.task_closure_sizes.get("worker_poll"), Some(&3));
    }

    /// The effective whole-program metadata for a batch is the UNION of the
    /// module context (cross-batch) and the batch's LOCAL scan — never a replace
    /// (design-20 finding #3C activation). A module context built from a
    /// different function set (e.g. the stdlib cache) does NOT carry a
    /// closure/task/leaf defined only in this batch; replacing the local scan
    /// dropped it, so a `call_guarded` to that closure skipped env extraction and
    /// the callee received a garbage closure (`'object' is not subscriptable`).
    #[test]
    fn effective_metadata_unions_module_context_with_local_scan() {
        // A module context that knows ONLY a stdlib closure / task / leaf.
        let stdlib_funcs = vec![FunctionIR {
            name: "contextlib___inner".to_string(),
            params: vec!["__molt_closure__".to_string()],
            ops: vec![OpIR {
                kind: "func_new_closure".to_string(),
                s_value: Some("contextlib___inner".to_string()),
                out: Some("v0".to_string()),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
        }];
        let ctx = SimpleBackend::build_module_context(&stdlib_funcs);
        assert!(ctx.closure_functions.contains("contextlib___inner"));

        // The current batch defines its OWN closure that the context never saw.
        let mut local_closures = BTreeSet::new();
        local_closures.insert("app__inner".to_string());
        let merged = merge_closure_functions(Some(&ctx), local_closures);
        assert!(
            merged.contains("app__inner"),
            "the batch's own closure must survive the merge (no replace)"
        );
        assert!(
            merged.contains("contextlib___inner"),
            "the module context's cross-batch closures must also be present"
        );

        // None context → pure local (user-only / non-batched build).
        let mut only_local = BTreeSet::new();
        only_local.insert("app__inner".to_string());
        let merged_none = merge_closure_functions(None, only_local);
        assert!(merged_none.contains("app__inner"));
        assert_eq!(merged_none.len(), 1);

        // Same union contract for task kinds and leaf functions.
        let mut local_tasks = BTreeMap::new();
        local_tasks.insert("app_poll".to_string(), TrampolineKind::Coroutine);
        let merged_tasks = merge_task_kinds(Some(&ctx), local_tasks);
        assert_eq!(
            merged_tasks.get("app_poll"),
            Some(&TrampolineKind::Coroutine)
        );
        let mut local_leaves = BTreeSet::new();
        local_leaves.insert("app_leaf".to_string());
        let merged_leaves = merge_leaf_functions(Some(&ctx), local_leaves);
        assert!(merged_leaves.contains("app_leaf"));
        assert!(merged_leaves.contains("contextlib___inner"));
    }

    #[test]
    fn native_backend_module_context_preserves_cross_batch_alias_metadata() {
        let functions = vec![
            FunctionIR {
                name: "helper".to_string(),
                params: vec!["value".to_string(), "intrinsic".to_string()],
                ops: vec![OpIR {
                    kind: "ret".to_string(),
                    var: Some("value".to_string()),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
            FunctionIR {
                name: "helper_poll".to_string(),
                params: vec!["state".to_string()],
                ops: vec![OpIR {
                    kind: "ret".to_string(),
                    var: Some("state".to_string()),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
        ];

        let context = SimpleBackend::build_module_context(&functions);

        assert_eq!(context.function_arities.get("helper"), Some(&2));
        assert_eq!(context.function_has_ret.get("helper"), Some(&true));
        assert_eq!(
            context.return_alias_summaries.get("helper"),
            Some(&ReturnAliasSummary::Param(0))
        );
        assert!(context.leaf_functions.contains("helper"));
        assert!(context.leaf_functions.contains("helper_poll"));
    }

    #[test]
    fn tir_roundtrip_preserves_store_var_return_alias_summary() {
        let func = FunctionIR {
            name: "helper".to_string(),
            params: vec!["value".to_string()],
            ops: vec![
                OpIR {
                    kind: "store_var".to_string(),
                    var: Some("tmp".to_string()),
                    args: Some(vec!["value".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    var: Some("tmp".to_string()),
                    ..OpIR::default()
                },
            ],
            param_types: Some(vec!["str".to_string()]),
            source_file: None,
            is_extern: false,
        };

        let roundtripped = roundtrip_function_through_tir(&func);
        let summaries =
            crate::passes::compute_return_alias_summaries(std::slice::from_ref(&roundtripped));

        assert_eq!(
            summaries.get("helper"),
            Some(&ReturnAliasSummary::Param(0)),
            "roundtripped params: {:?}; ops: {:?}; summaries: {:?}",
            roundtripped.params,
            roundtripped.ops,
            summaries
        );
    }

    #[test]
    fn native_backend_module_context_preserves_cross_batch_void_return_metadata() {
        let functions = vec![
            FunctionIR {
                name: "value_helper".to_string(),
                params: vec!["value".to_string()],
                ops: vec![OpIR {
                    kind: "ret".to_string(),
                    var: Some("value".to_string()),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
            FunctionIR {
                name: "void_helper".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
        ];

        let context = SimpleBackend::build_module_context(&functions);

        assert_eq!(context.function_has_ret.get("value_helper"), Some(&true));
        assert_eq!(context.function_has_ret.get("void_helper"), Some(&false));
    }

    #[test]
    fn trampoline_key_distinguishes_void_and_value_targets() {
        let value_key = TrampolineKey {
            name: "helper".to_string(),
            arity: 1,
            has_closure: false,
            is_import: false,
            kind: TrampolineKind::Plain,
            closure_size: 0,
            target_has_ret: true,
        };
        let void_key = TrampolineKey {
            target_has_ret: false,
            ..value_key.clone()
        };

        assert_ne!(value_key, void_key);
    }

    #[test]
    fn native_backend_preserves_split_stub_calls_to_void_and_value_chunks() {
        let chunk0 = "__molt_chunk_demo__molt_module_chunk_1_0".to_string();
        let chunk1 = "__molt_chunk_demo__molt_module_chunk_1_1".to_string();
        let stub = "demo__molt_module_chunk_1".to_string();
        let clif = compile_function_to_clif_text(
            vec![
                FunctionIR {
                    name: chunk0,
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: chunk1,
                    params: vec![],
                    ops: vec![
                        OpIR {
                            kind: "const".to_string(),
                            out: Some("chunk_ret".to_string()),
                            value: Some(7),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "ret".to_string(),
                            var: Some("chunk_ret".to_string()),
                            ..OpIR::default()
                        },
                    ],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: stub.clone(),
                    params: vec![],
                    ops: vec![
                        OpIR {
                            kind: "call_internal".to_string(),
                            s_value: Some("__molt_chunk_demo__molt_module_chunk_1_0".to_string()),
                            out: Some("__chunk_discard_0".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "call_internal".to_string(),
                            s_value: Some("__molt_chunk_demo__molt_module_chunk_1_1".to_string()),
                            out: Some("__chunk_ret".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "ret".to_string(),
                            var: Some("__chunk_ret".to_string()),
                            ..OpIR::default()
                        },
                    ],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
            ],
            &stub,
        );
        let local_callees: Vec<String> = clif
            .lines()
            .map(str::trim)
            .filter_map(|line| {
                line.split_once(" = colocated")
                    .map(|(name, _)| name.to_string())
            })
            .collect();
        assert_eq!(
            local_callees.len(),
            2,
            "stub CLIF should reference exactly two local chunk callees:\n{clif}",
        );
        assert!(
            local_callees
                .iter()
                .any(|callee| clif.contains(&format!("call {callee}("))),
            "split stub must retain the direct call to the first void-returning chunk:\n{clif}",
        );
        assert!(
            local_callees
                .iter()
                .any(|callee| clif.contains(&format!("= call {callee}("))),
            "split stub must retain the direct call to the final value-returning chunk:\n{clif}",
        );
    }

    fn compile_caller_with_incompatible_predeclared_helper(caller: FunctionIR) {
        let mut backend = SimpleBackend::new();
        let mut predeclared_sig = backend.module.make_signature();
        predeclared_sig
            .returns
            .push(cranelift_codegen::ir::AbiParam::new(types::I64));
        backend
            .module
            .declare_function(
                "helper",
                cranelift_module::Linkage::Import,
                &predeclared_sig,
            )
            .expect("predeclare helper");

        let defined_functions = BTreeSet::from(["caller".to_string(), "helper".to_string()]);
        let function_arities = BTreeMap::from([
            ("caller".to_string(), caller.params.len()),
            ("helper".to_string(), 1usize),
        ]);
        let function_has_ret =
            BTreeMap::from([("caller".to_string(), true), ("helper".to_string(), true)]);
        backend.compile_func(
            caller,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &defined_functions,
            &defined_functions,
            &BTreeSet::new(),
            &BTreeMap::new(),
            false,
            &BTreeSet::new(),
            &function_arities,
            &function_has_ret,
        );
    }

    #[test]
    #[should_panic(expected = "builtin_func declaration mismatch for `helper`")]
    fn builtin_func_signature_mismatch_fails_closed_at_codegen() {
        compile_caller_with_incompatible_predeclared_helper(FunctionIR {
            name: "caller".to_string(),
            params: Vec::new(),
            ops: vec![
                OpIR {
                    kind: "builtin_func".to_string(),
                    s_value: Some("helper".to_string()),
                    out: Some("helper_obj".to_string()),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    var: Some("helper_obj".to_string()),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        });
    }

    #[test]
    #[should_panic(expected = "func_new declaration mismatch for `helper`")]
    fn func_new_signature_mismatch_fails_closed_at_codegen() {
        compile_caller_with_incompatible_predeclared_helper(FunctionIR {
            name: "caller".to_string(),
            params: Vec::new(),
            ops: vec![
                OpIR {
                    kind: "func_new".to_string(),
                    s_value: Some("helper".to_string()),
                    out: Some("helper_obj".to_string()),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    var: Some("helper_obj".to_string()),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        });
    }

    #[test]
    #[should_panic(expected = "fn_ptr_code_set declaration mismatch for `helper`")]
    fn fn_ptr_code_set_signature_mismatch_fails_closed_at_codegen() {
        compile_caller_with_incompatible_predeclared_helper(FunctionIR {
            name: "caller".to_string(),
            params: Vec::new(),
            ops: vec![
                OpIR {
                    kind: "const_none".to_string(),
                    out: Some("code".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "fn_ptr_code_set".to_string(),
                    s_value: Some("helper".to_string()),
                    args: Some(vec!["code".to_string()]),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        });
    }

    #[test]
    #[should_panic(expected = "asyncgen_locals_register declaration mismatch for `helper`")]
    fn asyncgen_locals_register_signature_mismatch_fails_closed_at_codegen() {
        compile_caller_with_incompatible_predeclared_helper(FunctionIR {
            name: "caller".to_string(),
            params: Vec::new(),
            ops: vec![
                OpIR {
                    kind: "const_none".to_string(),
                    out: Some("names".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_none".to_string(),
                    out: Some("offsets".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "asyncgen_locals_register".to_string(),
                    s_value: Some("helper".to_string()),
                    args: Some(vec!["names".to_string(), "offsets".to_string()]),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        });
    }

    #[test]
    #[should_panic(expected = "gen_locals_register declaration mismatch for `helper`")]
    fn gen_locals_register_signature_mismatch_fails_closed_at_codegen() {
        compile_caller_with_incompatible_predeclared_helper(FunctionIR {
            name: "caller".to_string(),
            params: Vec::new(),
            ops: vec![
                OpIR {
                    kind: "const_none".to_string(),
                    out: Some("names".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_none".to_string(),
                    out: Some("offsets".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "gen_locals_register".to_string(),
                    s_value: Some("helper".to_string()),
                    args: Some(vec!["names".to_string(), "offsets".to_string()]),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        });
    }

    #[test]
    #[should_panic(expected = "call declaration mismatch for `helper`")]
    fn call_signature_mismatch_fails_closed_at_codegen() {
        compile_caller_with_incompatible_predeclared_helper(FunctionIR {
            name: "caller".to_string(),
            params: Vec::new(),
            ops: vec![
                OpIR {
                    kind: "const".to_string(),
                    out: Some("arg".to_string()),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "call".to_string(),
                    s_value: Some("helper".to_string()),
                    out: Some("result".to_string()),
                    args: Some(vec!["arg".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    var: Some("result".to_string()),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        });
    }

    fn compile_missing_static_target_symbol(kind: &str) {
        compile_function_to_clif_text(
            vec![FunctionIR {
                name: "caller".to_string(),
                params: Vec::new(),
                ops: vec![
                    OpIR {
                        kind: kind.to_string(),
                        args: Some(vec!["callee".to_string()]),
                        out: Some("result".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
            }],
            "caller",
        );
    }

    #[test]
    #[should_panic(expected = "call missing static target symbol")]
    fn call_missing_target_symbol_fails_closed_at_codegen() {
        compile_missing_static_target_symbol("call");
    }

    #[test]
    #[should_panic(expected = "call_internal missing static target symbol")]
    fn call_internal_missing_target_symbol_fails_closed_at_codegen() {
        compile_missing_static_target_symbol("call_internal");
    }

    #[test]
    #[should_panic(expected = "call_guarded missing static target symbol")]
    fn call_guarded_missing_target_symbol_fails_closed_at_codegen() {
        compile_missing_static_target_symbol("call_guarded");
    }

    #[test]
    #[should_panic(expected = "const_str missing bytes or string payload for output `missing`")]
    fn const_str_missing_payload_fails_closed_at_codegen() {
        compile_function_to_clif_text(
            vec![FunctionIR {
                name: "const_str_missing_payload".to_string(),
                params: Vec::new(),
                ops: vec![
                    OpIR {
                        kind: "const_str".to_string(),
                        out: Some("missing".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
            }],
            "const_str_missing_payload",
        );
    }

    #[test]
    fn const_str_empty_string_payload_still_compiles() {
        let clif = compile_function_to_clif_text(
            vec![FunctionIR {
                name: "const_str_empty_payload".to_string(),
                params: Vec::new(),
                ops: vec![
                    OpIR {
                        kind: "const_str".to_string(),
                        out: Some("empty".to_string()),
                        s_value: Some(String::new()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        var: Some("empty".to_string()),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
            }],
            "const_str_empty_payload",
        );

        assert!(clif.contains("return"));
    }

    #[test]
    #[should_panic(expected = "call_guarded declaration mismatch for `helper`")]
    fn call_guarded_signature_mismatch_fails_closed_at_codegen() {
        compile_caller_with_incompatible_predeclared_helper(FunctionIR {
            name: "caller".to_string(),
            params: Vec::new(),
            ops: vec![
                OpIR {
                    kind: "const_none".to_string(),
                    out: Some("callee".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    out: Some("arg".to_string()),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "call_guarded".to_string(),
                    s_value: Some("helper".to_string()),
                    out: Some("result".to_string()),
                    args: Some(vec!["callee".to_string(), "arg".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    var: Some("result".to_string()),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        });
    }

    #[test]
    #[should_panic(expected = "call_internal declaration mismatch for `helper`")]
    fn call_internal_signature_mismatch_fails_closed_at_codegen() {
        compile_function_to_clif_text(
            vec![
                FunctionIR {
                    name: "helper".to_string(),
                    params: vec!["value".to_string()],
                    ops: vec![OpIR {
                        kind: "ret".to_string(),
                        var: Some("value".to_string()),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "caller".to_string(),
                    params: Vec::new(),
                    ops: vec![
                        OpIR {
                            kind: "const".to_string(),
                            out: Some("arg".to_string()),
                            value: Some(1),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "func_new".to_string(),
                            s_value: Some("helper".to_string()),
                            out: Some("helper_obj".to_string()),
                            value: Some(0),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "call_internal".to_string(),
                            s_value: Some("helper".to_string()),
                            out: Some("result".to_string()),
                            args: Some(vec!["arg".to_string()]),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "ret".to_string(),
                            var: Some("result".to_string()),
                            ..OpIR::default()
                        },
                    ],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
            ],
            "caller",
        );
    }

    #[test]
    #[should_panic(expected = "func_new_closure declaration mismatch for `helper`")]
    fn func_new_closure_signature_mismatch_fails_closed_at_codegen() {
        compile_caller_with_incompatible_predeclared_helper(FunctionIR {
            name: "caller".to_string(),
            params: Vec::new(),
            ops: vec![
                OpIR {
                    kind: "const_none".to_string(),
                    out: Some("closure".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "func_new_closure".to_string(),
                    s_value: Some("helper".to_string()),
                    out: Some("helper_obj".to_string()),
                    args: Some(vec!["closure".to_string()]),
                    value: Some(0),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    var: Some("helper_obj".to_string()),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        });
    }

    #[test]
    fn compute_function_has_ret_uses_actual_ir_not_name_heuristics() {
        let result = compute_function_has_ret(&[
            FunctionIR {
                name: "demo__molt_module_chunk_1".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
            FunctionIR {
                name: "demo____molt_globals_builtin__".to_string(),
                params: vec![],
                ops: vec![
                    OpIR {
                        kind: "const_none".to_string(),
                        out: Some("ret".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        var: Some("ret".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
        ]);

        assert_eq!(result.get("demo__molt_module_chunk_1"), Some(&false));
        assert_eq!(result.get("demo____molt_globals_builtin__"), Some(&true));
    }

    #[test]
    fn compute_function_has_ret_treats_extern_declarations_as_value_returning() {
        let mut func = FunctionIR {
            name: "importlib__import_module".to_string(),
            params: vec!["name".to_string(), "package".to_string()],
            ops: vec![
                OpIR {
                    kind: "missing".to_string(),
                    out: Some("result".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["result".to_string()]),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };
        crate::externalize_function_with_signature(&mut func);
        let tir = crate::tir::lower_from_simple::lower_to_tir(&func);
        assert_eq!(
            tir.return_type,
            crate::tir::types::TirType::DynBox,
            "TIR declaration metadata preserves extern value signatures from the signature stub",
        );
        let result = compute_function_has_ret(&[func]);

        assert_eq!(
            result.get("importlib__import_module"),
            Some(&true),
            "extern declarations must preserve the source body's value-returning ABI fact",
        );
    }

    #[test]
    fn compute_function_has_ret_preserves_void_extern_declaration_signature() {
        let mut func = FunctionIR {
            name: "stdlib_void_helper".to_string(),
            params: vec![],
            ops: vec![OpIR {
                kind: "ret_void".to_string(),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
        };
        crate::externalize_function_with_signature(&mut func);
        let tir = crate::tir::lower_from_simple::lower_to_tir(&func);
        assert_eq!(
            tir.return_type,
            crate::tir::types::TirType::None,
            "TIR declaration metadata preserves extern void signatures from the signature stub",
        );
        let result = compute_function_has_ret(&[func]);

        assert_eq!(
            result.get("stdlib_void_helper"),
            Some(&false),
            "extern declarations must preserve the source body's void ABI fact",
        );
    }

    #[test]
    fn cranelift_import_declaration_uses_externalized_value_return_signature() {
        let mut extern_helper = FunctionIR {
            name: "stdlib_value_helper".to_string(),
            params: Vec::new(),
            ops: vec![
                OpIR {
                    kind: "missing".to_string(),
                    out: Some("value".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["value".to_string()]),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };
        crate::externalize_function_with_signature(&mut extern_helper);
        let caller = FunctionIR {
            name: "molt_main".to_string(),
            params: Vec::new(),
            ops: vec![
                OpIR {
                    kind: "call".to_string(),
                    s_value: Some("stdlib_value_helper".to_string()),
                    out: Some("result".to_string()),
                    args: Some(Vec::new()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    var: Some("result".to_string()),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };
        let functions = vec![caller.clone(), extern_helper.clone()];
        let module_context = SimpleBackend::build_module_context(&functions);
        assert_eq!(
            module_context.function_has_ret.get("stdlib_value_helper"),
            Some(&true),
            "externalized stdlib helper must keep the value-returning ABI fact in shared module metadata",
        );
        let local_function_arities = BTreeMap::from([("molt_main".to_string(), 0usize)]);
        let effective_function_arities =
            merge_function_arities(Some(&module_context), local_function_arities);
        let local_function_has_ret = compute_function_has_ret(std::slice::from_ref(&caller));
        let effective_function_has_ret =
            merge_function_has_ret(Some(&module_context), local_function_has_ret);
        let mut module_known_functions = BTreeSet::from(["molt_main".to_string()]);
        module_known_functions.insert("stdlib_value_helper".to_string());
        let mut backend = SimpleBackend::new();
        backend.compile_func(
            caller,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeSet::from(["molt_main".to_string()]),
            &module_known_functions,
            &BTreeSet::new(),
            &BTreeMap::new(),
            false,
            &BTreeSet::new(),
            &effective_function_arities,
            &effective_function_has_ret,
        );
        let declaration = backend
            .module
            .declarations()
            .get_functions()
            .find_map(|(_, decl)| {
                (decl.name.as_deref() == Some("stdlib_value_helper")).then_some(decl)
            })
            .expect("stdlib_value_helper import declaration");

        assert_eq!(declaration.linkage, cranelift_module::Linkage::Import);
        assert_eq!(declaration.signature.params.len(), 0);
        assert_eq!(declaration.signature.returns.len(), 1);
        assert_eq!(declaration.signature.returns[0].value_type, types::I64);
    }

    #[test]
    fn compute_function_has_ret_keeps_actual_signature_for_python_callable_targets() {
        let result = compute_function_has_ret(&[
            FunctionIR {
                name: "user_func".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
            FunctionIR {
                name: "demo__molt_module_chunk_1".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "func_new".to_string(),
                    s_value: Some("user_func".to_string()),
                    value: Some(0),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
        ]);

        assert_eq!(result.get("user_func"), Some(&false));
        assert_eq!(result.get("demo__molt_module_chunk_1"), Some(&false));
    }

    #[test]
    fn compute_function_has_ret_treats_state_machines_as_value_returning() {
        let result = compute_function_has_ret(&[FunctionIR {
            name: "raises_only_coroutine_poll".to_string(),
            params: vec!["self".to_string()],
            ops: vec![
                OpIR {
                    kind: "state_switch".to_string(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: Some(vec!["i64".to_string()]),
            source_file: None,
            is_extern: false,
        }]);

        assert_eq!(
            result.get("raises_only_coroutine_poll"),
            Some(&true),
            "poll functions are always invoked through the i64 poll ABI even when every user path raises",
        );
    }

    #[test]
    fn local_function_metadata_overrides_stale_module_context_after_split() {
        let context = NativeBackendModuleContext {
            function_arities: BTreeMap::from([(
                "__molt_chunk_builtins__molt_module_chunk_3_0".to_string(),
                1usize,
            )]),
            function_has_ret: BTreeMap::from([(
                "__molt_chunk_builtins__molt_module_chunk_3_0".to_string(),
                false,
            )]),
            ..NativeBackendModuleContext::default()
        };

        let merged_arities = merge_function_arities(
            Some(&context),
            BTreeMap::from([(
                "__molt_chunk_builtins__molt_module_chunk_3_0".to_string(),
                1usize,
            )]),
        );
        let merged_has_ret = merge_function_has_ret(
            Some(&context),
            BTreeMap::from([(
                "__molt_chunk_builtins__molt_module_chunk_3_0".to_string(),
                true,
            )]),
        );

        assert_eq!(
            merged_arities.get("__molt_chunk_builtins__molt_module_chunk_3_0"),
            Some(&1usize)
        );
        assert_eq!(
            merged_has_ret.get("__molt_chunk_builtins__molt_module_chunk_3_0"),
            Some(&true)
        );
    }

    #[test]
    fn native_backend_import_ids_are_cached_by_symbol() {
        let mut backend = SimpleBackend::new();

        let first = backend.import_func_id("molt_dec_ref", &[types::I64], &[]);
        let second = backend.import_func_id("molt_dec_ref", &[types::I64], &[]);

        assert_eq!(first, second);
        assert_eq!(backend.import_ids.len(), 1);
    }

    #[test]
    fn native_backend_skips_profile_store_imports_when_function_has_no_store_ops() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "ret".to_string(),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            }],
            profile: None,
        };

        let output = SimpleBackend::new().compile(ir);

        assert!(
            !output
                .bytes
                .windows(b"molt_profile_struct_field_store".len())
                .any(|window| window == b"molt_profile_struct_field_store")
        );
        assert!(
            !output
                .bytes
                .windows(b"molt_profile_enabled".len())
                .any(|window| window == b"molt_profile_enabled")
        );
    }

    #[test]
    fn native_backend_keeps_profile_store_imports_when_function_has_store_ops() {
        let _guard = acquire_backend_env_lock();
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![
                    OpIR {
                        kind: "const".to_string(),
                        out: Some("obj".to_string()),
                        value: Some(1),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const".to_string(),
                        out: Some("value".to_string()),
                        value: Some(2),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "store".to_string(),
                        args: Some(vec!["obj".to_string(), "value".to_string()]),
                        value: Some(8),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
            }],
            profile: None,
        };

        let output = SimpleBackend::new().compile(ir);

        assert!(
            output
                .bytes
                .windows(b"molt_profile_struct_field_store".len())
                .any(|window| window == b"molt_profile_struct_field_store")
        );
        assert!(
            output
                .bytes
                .windows(b"molt_profile_enabled".len())
                .any(|window| window == b"molt_profile_enabled")
        );
    }

    fn compile_check_exception_target_shape(name: &str, target: Option<i64>) {
        compile_function_to_clif_text(
            vec![FunctionIR {
                name: name.to_string(),
                params: Vec::new(),
                ops: vec![
                    OpIR {
                        kind: "const_none".to_string(),
                        out: Some("sentinel".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "check_exception".to_string(),
                        value: target,
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
            }],
            name,
        );
    }

    #[test]
    #[should_panic(
        expected = "check_exception missing target label id in function `native_check_exception_missing_target` op 1"
    )]
    fn check_exception_missing_target_fails_closed_at_codegen() {
        compile_check_exception_target_shape("native_check_exception_missing_target", None);
    }

    #[test]
    #[should_panic(
        expected = "check_exception target label 7 is not present in native label map for function `native_check_exception_orphan_target` op 1"
    )]
    fn check_exception_orphan_target_fails_closed_at_codegen() {
        compile_check_exception_target_shape("native_check_exception_orphan_target", Some(7));
    }

    #[test]
    fn native_backend_compiles_exception_label_guard_if_without_else() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "hello_regress____molt_globals_builtin__".to_string(),
                params: vec![],
                ops: vec![
                    OpIR {
                        kind: "exception_stack_enter".to_string(),
                        out: Some("v74".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "exception_stack_depth".to_string(),
                        out: Some("v75".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const_str".to_string(),
                        out: Some("v76".to_string()),
                        s_value: Some("hello_regress".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "check_exception".to_string(),
                        value: Some(2),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "module_cache_get".to_string(),
                        out: Some("v77".to_string()),
                        args: Some(vec!["v76".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "check_exception".to_string(),
                        value: Some(2),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const_str".to_string(),
                        out: Some("v78".to_string()),
                        s_value: Some("__dict__".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "check_exception".to_string(),
                        value: Some(2),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "module_get_attr".to_string(),
                        out: Some("v79".to_string()),
                        args: Some(vec!["v77".to_string(), "v78".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "check_exception".to_string(),
                        value: Some(2),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        var: Some("v79".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "label".to_string(),
                        value: Some(2),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "exception_stack_set_depth".to_string(),
                        args: Some(vec!["v75".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "exception_stack_exit".to_string(),
                        args: Some(vec!["v74".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "exception_last".to_string(),
                        out: Some("v80".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const_none".to_string(),
                        out: Some("v81".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "is".to_string(),
                        out: Some("v82".to_string()),
                        args: Some(vec!["v80".to_string(), "v81".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "not".to_string(),
                        out: Some("v83".to_string()),
                        args: Some(vec!["v82".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "if".to_string(),
                        args: Some(vec!["v83".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "raise".to_string(),
                        args: Some(vec!["v80".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const_none".to_string(),
                        out: Some("v84".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        var: Some("v84".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "end_if".to_string(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
            }],
            profile: None,
        };

        let output = SimpleBackend::new().compile(ir);

        assert!(!output.bytes.is_empty());
    }

    #[test]
    fn native_backend_compiles_tir_roundtripped_exception_label_guard_if_without_else() {
        let func = FunctionIR {
            name: "hello_regress____molt_globals_builtin__".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "exception_stack_enter".to_string(),
                    out: Some("v74".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_stack_depth".to_string(),
                    out: Some("v75".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".to_string(),
                    out: Some("v76".to_string()),
                    s_value: Some("hello_regress".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".to_string(),
                    value: Some(2),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_cache_get".to_string(),
                    out: Some("v77".to_string()),
                    args: Some(vec!["v76".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".to_string(),
                    value: Some(2),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".to_string(),
                    out: Some("v78".to_string()),
                    s_value: Some("__dict__".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".to_string(),
                    value: Some(2),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_get_attr".to_string(),
                    out: Some("v79".to_string()),
                    args: Some(vec!["v77".to_string(), "v78".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".to_string(),
                    value: Some(2),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    var: Some("v79".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "label".to_string(),
                    value: Some(2),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_stack_set_depth".to_string(),
                    args: Some(vec!["v75".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_stack_exit".to_string(),
                    args: Some(vec!["v74".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_last".to_string(),
                    out: Some("v80".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_none".to_string(),
                    out: Some("v81".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "is".to_string(),
                    out: Some("v82".to_string()),
                    args: Some(vec!["v80".to_string(), "v81".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "not".to_string(),
                    out: Some("v83".to_string()),
                    args: Some(vec!["v82".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "if".to_string(),
                    args: Some(vec!["v83".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "raise".to_string(),
                    args: Some(vec!["v80".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_none".to_string(),
                    out: Some("v84".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    var: Some("v84".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "end_if".to_string(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        let roundtripped = roundtrip_function_through_tir(&func);
        let clif = compile_function_to_clif_text(
            vec![roundtripped],
            "hello_regress____molt_globals_builtin__",
        );

        assert!(
            clif.contains("return"),
            "TIR-roundtripped exception function must compile to CLIF:\n{clif}"
        );
    }

    #[cfg(feature = "llvm")]
    #[test]
    fn llvm_backend_keeps_shared_stdlib_partition_external() {
        let _guard = acquire_backend_env_lock();
        let tmp_dir = std::env::temp_dir().join(format!(
            "molt-llvm-stdlib-extern-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time before unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let stdlib_obj = tmp_dir.join("stdlib.o");
        std::fs::write(&stdlib_obj, b"placeholder").expect("write stdlib marker");

        let prev_backend = std::env::var("MOLT_BACKEND").ok();
        let prev_stdlib_obj = std::env::var("MOLT_STDLIB_OBJ").ok();
        let prev_entry_module = std::env::var("MOLT_ENTRY_MODULE").ok();
        let prev_stdlib_symbols = std::env::var("MOLT_STDLIB_MODULE_SYMBOLS").ok();
        unsafe {
            std::env::set_var("MOLT_BACKEND", "llvm");
            std::env::set_var("MOLT_STDLIB_OBJ", &stdlib_obj);
            std::env::set_var("MOLT_ENTRY_MODULE", "app");
            std::env::set_var("MOLT_STDLIB_MODULE_SYMBOLS", "[\"sys\"]");
        }

        let ir = SimpleIR {
            functions: vec![
                FunctionIR {
                    name: "molt_main".to_string(),
                    params: vec![],
                    ops: vec![
                        OpIR {
                            kind: "call".to_string(),
                            s_value: Some("molt_init_sys".to_string()),
                            value: Some(0),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "ret_void".to_string(),
                            ..OpIR::default()
                        },
                    ],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "molt_init_sys".to_string(),
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
            ],
            profile: None,
        };

        let bytes = SimpleBackend::new().compile(ir).bytes;
        let output = tmp_dir.join("out.o");
        std::fs::write(&output, &bytes).expect("write llvm object");
        let nm = std::process::Command::new("nm")
            .args(["-g", output.to_str().expect("utf8 object path")])
            .output()
            .expect("run nm");
        assert!(
            nm.status.success(),
            "nm failed: {}",
            String::from_utf8_lossy(&nm.stderr)
        );
        let symbols = String::from_utf8_lossy(&nm.stdout);
        assert!(
            symbols
                .lines()
                .any(|line| line.contains(" U _molt_init_sys")
                    || line == "                 U molt_init_sys"),
            "shared stdlib symbol must be an undefined external, got:\n{symbols}"
        );
        assert!(
            !symbols
                .lines()
                .any(|line| line.contains(" T _molt_init_sys") || line.contains(" T molt_init_sys")),
            "LLVM output object must not define shared stdlib symbol, got:\n{symbols}"
        );

        match prev_backend {
            Some(value) => unsafe { std::env::set_var("MOLT_BACKEND", value) },
            None => unsafe { std::env::remove_var("MOLT_BACKEND") },
        }
        match prev_stdlib_obj {
            Some(value) => unsafe { std::env::set_var("MOLT_STDLIB_OBJ", value) },
            None => unsafe { std::env::remove_var("MOLT_STDLIB_OBJ") },
        }
        match prev_entry_module {
            Some(value) => unsafe { std::env::set_var("MOLT_ENTRY_MODULE", value) },
            None => unsafe { std::env::remove_var("MOLT_ENTRY_MODULE") },
        }
        match prev_stdlib_symbols {
            Some(value) => unsafe { std::env::set_var("MOLT_STDLIB_MODULE_SYMBOLS", value) },
            None => unsafe { std::env::remove_var("MOLT_STDLIB_MODULE_SYMBOLS") },
        }
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[cfg(not(feature = "llvm"))]
    #[test]
    #[should_panic(
        expected = "MOLT_BACKEND=llvm requested but molt-backend was built without the llvm feature"
    )]
    fn llvm_backend_request_without_feature_fails_closed() {
        super::assert_requested_llvm_backend_available(true);
    }

    #[cfg(not(feature = "llvm"))]
    #[test]
    fn llvm_missing_feature_guard_allows_non_llvm_backend_selection() {
        super::assert_requested_llvm_backend_available(false);
    }

    #[test]
    fn native_backend_compiles_tir_roundtripped_nested_loops() {
        let func = FunctionIR {
            name: "nested_loops".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const".into(),
                    value: Some(0),
                    out: Some("total".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".into(),
                    value: Some(0),
                    out: Some("i".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".into(),
                    value: Some(2),
                    out: Some("outer_limit".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".into(),
                    value: Some(2),
                    out: Some("inner_limit".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".into(),
                    value: Some(1),
                    out: Some("one".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_start".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "lt".into(),
                    args: Some(vec!["i".into(), "outer_limit".into()]),
                    out: Some("outer_cond".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_break_if_false".into(),
                    args: Some(vec!["outer_cond".into()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".into(),
                    value: Some(0),
                    out: Some("j".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_start".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "lt".into(),
                    args: Some(vec!["j".into(), "inner_limit".into()]),
                    out: Some("inner_cond".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_break_if_false".into(),
                    args: Some(vec!["inner_cond".into()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "add".into(),
                    args: Some(vec!["total".into(), "j".into()]),
                    out: Some("total".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "add".into(),
                    args: Some(vec!["j".into(), "one".into()]),
                    out: Some("j".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_continue".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_end".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "add".into(),
                    args: Some(vec!["i".into(), "one".into()]),
                    out: Some("i".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_continue".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_end".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".into(),
                    var: Some("total".into()),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        let roundtripped = roundtrip_function_through_tir(&func);
        let clif = compile_function_to_clif_text(vec![roundtripped], "nested_loops");

        assert!(
            clif.contains("return"),
            "TIR-roundtripped nested-loop function must compile to CLIF:\n{clif}"
        );
    }

    #[test]
    fn annotate_function_object_compiles_without_signature_mismatch() {
        let ir = SimpleIR {
            functions: vec![
                FunctionIR {
                    name: "_sitebuiltins____annotate__".to_string(),
                    params: vec!["format".to_string()],
                    ops: vec![OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "molt_main".to_string(),
                    params: vec![],
                    ops: vec![
                        OpIR {
                            kind: "func_new".to_string(),
                            s_value: Some("_sitebuiltins____annotate__".to_string()),
                            value: Some(1),
                            out: Some("annotate_fn".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "ret_void".to_string(),
                            ..OpIR::default()
                        },
                    ],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
            ],
            profile: None,
        };

        let output = SimpleBackend::new().compile(ir);

        assert!(!output.bytes.is_empty());
    }

    #[test]
    fn guarded_void_function_object_compiles_without_result_panic() {
        let ir = SimpleIR {
            functions: vec![
                FunctionIR {
                    name: "void_helper".to_string(),
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
                FunctionIR {
                    name: "molt_main".to_string(),
                    params: vec![],
                    ops: vec![
                        OpIR {
                            kind: "func_new".to_string(),
                            s_value: Some("void_helper".to_string()),
                            value: Some(0),
                            out: Some("void_fn".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "call_guarded".to_string(),
                            s_value: Some("void_helper".to_string()),
                            args: Some(vec!["void_fn".to_string()]),
                            out: Some("result".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "ret".to_string(),
                            var: Some("result".to_string()),
                            ..OpIR::default()
                        },
                    ],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                },
            ],
            profile: None,
        };

        let output = SimpleBackend::new().compile(ir);

        assert!(!output.bytes.is_empty());
    }

    #[test]
    fn direct_imported_runtime_call_avoids_guarded_call_wrapper() {
        let func = FunctionIR {
            name: "hot_runtime_call".to_string(),
            params: vec![
                "a".to_string(),
                "b".to_string(),
                "c".to_string(),
                "d".to_string(),
                "e".to_string(),
                "f".to_string(),
                "g".to_string(),
                "h".to_string(),
            ],
            ops: vec![
                OpIR {
                    kind: "call".to_string(),
                    s_value: Some("molt_gpu_linear_contiguous".to_string()),
                    args: Some(vec![
                        "a".to_string(),
                        "b".to_string(),
                        "c".to_string(),
                        "d".to_string(),
                        "e".to_string(),
                        "f".to_string(),
                        "g".to_string(),
                        "h".to_string(),
                    ]),
                    out: Some("out".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    var: Some("out".to_string()),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        let clif = compile_function_to_clif_text(vec![func], "hot_runtime_call");

        assert!(
            !clif.contains("molt_guarded_call"),
            "direct imported runtime calls should not route through molt_guarded_call:\n{clif}"
        );
        assert!(
            !clif.contains("explicit_slot"),
            "direct imported runtime calls should not spill args for the guarded-call wrapper:\n{clif}"
        );
    }

    #[test]
    fn native_boxed_or_retains_selected_operand_result() {
        let func = FunctionIR {
            name: "boxed_or_selected_owner".to_string(),
            params: vec!["lhs".to_string(), "rhs".to_string()],
            ops: vec![
                OpIR {
                    kind: "or".to_string(),
                    args: Some(vec!["lhs".to_string(), "rhs".to_string()]),
                    out: Some("selected".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    var: Some("selected".to_string()),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        let clif = compile_function_to_clif_text(vec![func], "boxed_or_selected_owner");
        let selected = clif
            .lines()
            .find_map(|line| line.trim().split_once(" = select ").map(|(v, _)| v.trim()))
            .unwrap_or_else(|| panic!("boxed or must emit a selected result:\n{clif}"));
        let selected_call = format!("({selected})");
        assert!(
            clif.lines().any(|line| {
                let line = line.trim();
                line.starts_with("call fn") && line.contains(&selected_call)
            }),
            "boxed or must retain the selected result before returning it:\n{clif}"
        );
    }

    #[test]
    fn native_shift_lowering_uses_runtime_without_shift_count_proof() {
        let func = FunctionIR {
            name: "shift_runtime_contract".to_string(),
            params: vec!["lhs".to_string(), "rhs".to_string()],
            ops: vec![
                OpIR {
                    kind: "const".to_string(),
                    value: Some(8),
                    out: Some("const_lhs".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    value: Some(1),
                    out: Some("const_rhs".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "lshift".to_string(),
                    args: Some(vec!["lhs".to_string(), "rhs".to_string()]),
                    out: Some("left".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "rshift".to_string(),
                    args: Some(vec!["lhs".to_string(), "rhs".to_string()]),
                    out: Some("right".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "shl".to_string(),
                    args: Some(vec!["const_lhs".to_string(), "const_rhs".to_string()]),
                    out: Some("const_left".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "shr".to_string(),
                    args: Some(vec!["const_lhs".to_string(), "const_rhs".to_string()]),
                    out: Some("const_right".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "add".to_string(),
                    args: Some(vec!["left".to_string(), "right".to_string()]),
                    out: Some("param_sum".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "add".to_string(),
                    args: Some(vec!["const_left".to_string(), "const_right".to_string()]),
                    out: Some("const_sum".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "add".to_string(),
                    args: Some(vec!["param_sum".to_string(), "const_sum".to_string()]),
                    out: Some("out".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    var: Some("out".to_string()),
                    ..OpIR::default()
                },
            ],
            param_types: Some(vec!["int".to_string(), "int".to_string()]),
            source_file: None,
            is_extern: false,
        };

        let clif = compile_function_to_clif_text(vec![func], "shift_runtime_contract");
        let binary_calls = clif
            .lines()
            .filter(|line| line.contains(" = call fn"))
            .count();

        assert!(
            binary_calls >= 4,
            "native shifts over typed params and raw-primary constants must remain runtime calls until range and shift-count proof exists:\n{clif}"
        );
        assert!(
            !clif.contains("ishl.i64 v1, v2") && !clif.contains("sshr v1, v2"),
            "native shifts must not lower directly to raw Cranelift shifts without proof:\n{clif}"
        );
    }

    #[test]
    fn nested_exception_raise_if_does_not_synthesize_zero_predecessors() {
        let clif = compile_function_to_clif_text(
            vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![
                    OpIR {
                        kind: "const_bool".to_string(),
                        value: Some(0),
                        out: Some("flag".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "if".to_string(),
                        args: Some(vec!["flag".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "else".to_string(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "exception_last".to_string(),
                        out: Some("exc".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const_none".to_string(),
                        out: Some("nonev".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "is".to_string(),
                        args: Some(vec!["exc".to_string(), "nonev".to_string()]),
                        out: Some("is_none".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "not".to_string(),
                        args: Some(vec!["is_none".to_string()]),
                        out: Some("has_exc".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "if".to_string(),
                        args: Some(vec!["has_exc".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "exception_stack_clear".to_string(),
                        out: Some("none".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "raise".to_string(),
                        args: Some(vec!["exc".to_string()]),
                        out: Some("none".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "check_exception".to_string(),
                        value: Some(1),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "jump".to_string(),
                        value: Some(1),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "end_if".to_string(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "check_exception".to_string(),
                        value: Some(1),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "end_if".to_string(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "label".to_string(),
                        value: Some(1),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
            }],
            "molt_main",
        );

        let suspicious: Vec<&str> = clif
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with("jump block") && line.contains(" = 0"))
            .collect();

        assert!(
            suspicious.is_empty(),
            "nested exception raise CFG synthesized zero-valued predecessors:\n{}\n\nCLIF:\n{}",
            suspicious.join("\n"),
            clif
        );
    }

    #[test]
    fn rewrite_phi_to_store_load_rewrites_merge_phi() {
        let mut ops = vec![
            OpIR {
                kind: "const_bool".to_string(),
                value: Some(1),
                out: Some("cond".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "if".to_string(),
                args: Some(vec!["cond".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".to_string(),
                value: Some(1),
                out: Some("then_val".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "else".to_string(),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".to_string(),
                value: Some(2),
                out: Some("else_val".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "end_if".to_string(),
                ..OpIR::default()
            },
            OpIR {
                kind: "phi".to_string(),
                out: Some("merged".to_string()),
                args: Some(vec!["then_val".to_string(), "else_val".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".to_string(),
                var: Some("merged".to_string()),
                ..OpIR::default()
            },
        ];

        rewrite_phi_to_store_load(&mut ops);

        assert!(
            ops.iter().all(|op| op.kind != "phi"),
            "phi should be eliminated: {ops:?}"
        );
        assert!(
            ops.iter().any(|op| {
                op.kind == "store_var"
                    && op.var.as_deref() == Some("_phi_merged")
                    && op
                        .args
                        .as_ref()
                        .is_some_and(|args| args.len() == 1 && args[0] == "then_val")
            }),
            "then branch should store merged value"
        );
        assert!(
            ops.iter().any(|op| {
                op.kind == "store_var"
                    && op.var.as_deref() == Some("_phi_merged")
                    && op
                        .args
                        .as_ref()
                        .is_some_and(|args| args.len() == 1 && args[0] == "else_val")
            }),
            "else branch should store merged value"
        );
        assert!(
            ops.iter().any(|op| {
                op.kind == "load_var"
                    && op.var.as_deref() == Some("_phi_merged")
                    && op.out.as_deref() == Some("merged")
            }),
            "merged phi should become load_var"
        );
    }

    #[test]
    fn fast_int_overflow_result_does_not_unbox_merged_bigint_result() {
        let clif = compile_function_to_clif_text(
            vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![
                    OpIR {
                        kind: "const".to_string(),
                        out: Some("base".to_string()),
                        value: Some(2),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const".to_string(),
                        out: Some("exp".to_string()),
                        value: Some(63),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "pow".to_string(),
                        args: Some(vec!["base".to_string(), "exp".to_string()]),
                        out: Some("powv".to_string()),
                        fast_int: Some(true),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const".to_string(),
                        out: Some("one".to_string()),
                        value: Some(1),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "sub".to_string(),
                        args: Some(vec!["powv".to_string(), "one".to_string()]),
                        out: Some("maxsize".to_string()),
                        fast_int: Some(true),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        var: Some("maxsize".to_string()),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
            }],
            "molt_main",
        );

        assert!(
            !clif.contains("block11(v43: i64):\n    v77 = iconst.i64 0x7fff_0000_0000_0000"),
            "merged overflow result must remain boxed until a real inline-int consumer proves otherwise:\n{clif}",
        );
    }

    #[test]
    fn bool_primary_loop_compare_does_not_materialize_boxed_bool() {
        let clif = compile_function_to_clif_text(
            vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![
                    OpIR {
                        kind: "const".to_string(),
                        out: Some("init".to_string()),
                        value: Some(0),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const".to_string(),
                        out: Some("one".to_string()),
                        value: Some(1),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const".to_string(),
                        out: Some("limit".to_string()),
                        value: Some(10),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "store_var".to_string(),
                        var: Some("_bb1_arg0".to_string()),
                        args: Some(vec!["init".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "loop_start".to_string(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "load_var".to_string(),
                        out: Some("i_cur".to_string()),
                        var: Some("_bb1_arg0".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "lt".to_string(),
                        out: Some("keep_going".to_string()),
                        args: Some(vec!["i_cur".to_string(), "limit".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "loop_break_if_false".to_string(),
                        args: Some(vec!["keep_going".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "add".to_string(),
                        out: Some("i_next".to_string()),
                        args: Some(vec!["i_cur".to_string(), "one".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "store_var".to_string(),
                        var: Some("_bb1_arg0".to_string()),
                        args: Some(vec!["i_next".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "loop_continue".to_string(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "loop_end".to_string(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        var: Some("i_cur".to_string()),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
            }],
            "molt_main",
        );

        assert!(
            clif.contains("icmp slt"),
            "loop comparison should lower to a raw signed compare:\n{clif}"
        );
        assert!(
            !clif.contains("0x7ffa_0000_0000_0000"),
            "bool-primary loop compare should not materialize a NaN-boxed bool:\n{clif}"
        );
    }

    #[test]
    fn drain_cleanup_entry_tracked_can_skip_named_value() {
        let mut names = vec!["callee".to_string(), "other".to_string()];
        let mut entry_vars = BTreeMap::new();
        let callee = Value::from_u32(11);
        let other = Value::from_u32(22);
        entry_vars.insert("callee".to_string(), callee);
        entry_vars.insert("other".to_string(), other);
        let last_use = BTreeMap::from([
            ("callee".to_string(), 5usize),
            ("other".to_string(), 5usize),
        ]);
        let alias_roots = BTreeMap::new();
        let mut already_decrefed = BTreeSet::new();

        let cleanup = drain_cleanup_entry_tracked(
            &mut names,
            &mut entry_vars,
            &last_use,
            &alias_roots,
            &mut already_decrefed,
            5,
            Some("callee"),
        );

        assert_eq!(cleanup, vec![other]);
        assert_eq!(names, vec!["callee".to_string()]);
        assert!(entry_vars.contains_key("callee"));
        assert!(!entry_vars.contains_key("other"));
    }

    #[test]
    fn authority_disabled_tracked_drain_clears_without_cleanup() {
        let mut names = vec!["dead".to_string()];
        let last_use = BTreeMap::from([("dead".to_string(), 1usize)]);
        let alias_roots = BTreeMap::new();
        let mut already_decrefed = BTreeSet::new();

        let cleanup = drain_cleanup_tracked_dedup_with_authority(
            false,
            &mut names,
            &last_use,
            &alias_roots,
            1,
            None,
            Some(&mut already_decrefed),
        );

        assert!(cleanup.is_empty());
        assert!(names.is_empty());
        assert!(already_decrefed.is_empty());
    }

    #[test]
    fn authority_disabled_entry_drain_clears_without_cleanup() {
        let mut names = vec!["dead".to_string()];
        let mut entry_vars = BTreeMap::from([("dead".to_string(), Value::from_u32(17))]);
        let last_use = BTreeMap::from([("dead".to_string(), 1usize)]);
        let alias_roots = BTreeMap::new();
        let mut already_decrefed = BTreeSet::new();

        let cleanup = drain_cleanup_entry_tracked_with_authority(
            false,
            &mut names,
            &mut entry_vars,
            &last_use,
            &alias_roots,
            &mut already_decrefed,
            1,
            None,
        );

        assert!(cleanup.is_empty());
        assert!(names.is_empty());
        assert!(entry_vars.is_empty());
        assert!(already_decrefed.is_empty());
    }
}
