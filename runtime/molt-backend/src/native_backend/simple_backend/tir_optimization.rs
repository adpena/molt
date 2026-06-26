use super::*;
pub(in crate::native_backend::simple_backend) const TIR_OPTIMIZATION_BATCH_FUNCTION_LIMIT: usize =
    128;
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) const TIR_OPTIMIZATION_BATCH_OP_BUDGET: usize = 8_000;
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) const TIR_OPTIMIZATION_BASELINE_MEMORY_BYTES: usize =
    4 * 1024 * 1024 * 1024;
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) const TIR_OPTIMIZATION_WORKER_MEMORY_BYTES: usize =
    8 * 1024 * 1024 * 1024;
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) const TIR_OPTIMIZATION_WAVE_FUNCTIONS_PER_THREAD:
    usize = 1;
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) const TIR_OPTIMIZATION_WAVE_OPS_PER_THREAD: usize =
    1_000;
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) const DEFERRED_CODEGEN_FLUSH_FUNCTION_LIMIT: usize =
    16;
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) const DEFERRED_CODEGEN_FLUSH_OP_BUDGET: usize = 4_000;

#[cfg(feature = "native-backend")]
#[derive(Debug, Eq, PartialEq)]
pub(in crate::native_backend::simple_backend) struct TirOptimizationWorkItem {
    pub(in crate::native_backend::simple_backend) index: usize,
    pub(in crate::native_backend::simple_backend) content_hash: String,
    pub(in crate::native_backend::simple_backend) op_count: usize,
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) struct TirOptimizationInput {
    pub(in crate::native_backend::simple_backend) index: usize,
    pub(in crate::native_backend::simple_backend) content_hash: String,
    pub(in crate::native_backend::simple_backend) name: String,
    pub(in crate::native_backend::simple_backend) params: Vec<String>,
    pub(in crate::native_backend::simple_backend) ops: Vec<OpIR>,
    pub(in crate::native_backend::simple_backend) param_types: Option<Vec<String>>,
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) struct TirOptimizationOutput {
    pub(in crate::native_backend::simple_backend) index: usize,
    pub(in crate::native_backend::simple_backend) content_hash: String,
    pub(in crate::native_backend::simple_backend) simple_ops: Vec<OpIR>,
    pub(in crate::native_backend::simple_backend) tir_func: crate::tir::function::TirFunction,
}

#[cfg(feature = "native-backend")]
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(in crate::native_backend::simple_backend) struct TirOptimizationResourcePlan {
    pub(in crate::native_backend::simple_backend) threads: usize,
    pub(in crate::native_backend::simple_backend) wave_function_limit: usize,
    pub(in crate::native_backend::simple_backend) wave_op_budget: usize,
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn partition_tir_optimization_work_items_with_limits(
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
pub(in crate::native_backend::simple_backend) fn partition_tir_optimization_work_items(
    work_items: Vec<TirOptimizationWorkItem>,
) -> Vec<Vec<TirOptimizationWorkItem>> {
    partition_tir_optimization_work_items_with_limits(
        work_items,
        TIR_OPTIMIZATION_BATCH_FUNCTION_LIMIT,
        TIR_OPTIMIZATION_BATCH_OP_BUDGET,
    )
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn parse_positive_usize_env(
    name: &str,
) -> Option<usize> {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn parse_nonnegative_gb_env(
    name: &str,
) -> Option<usize> {
    let gb = std::env::var(name)
        .ok()
        .and_then(|raw| raw.trim().parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value >= 0.0)?;
    Some((gb * 1024.0 * 1024.0 * 1024.0) as usize)
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn env_memory_limit_bytes() -> Option<usize> {
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
pub(in crate::native_backend::simple_backend) fn rlimit_address_space_bytes() -> Option<usize> {
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
pub(in crate::native_backend::simple_backend) fn rlimit_address_space_bytes() -> Option<usize> {
    None
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn backend_memory_limit_bytes() -> Option<usize> {
    match (env_memory_limit_bytes(), rlimit_address_space_bytes()) {
        (Some(env_limit), Some(rlimit)) => Some(env_limit.min(rlimit)),
        (Some(env_limit), None) => Some(env_limit),
        (None, Some(rlimit)) => Some(rlimit),
        (None, None) => None,
    }
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn tir_optimization_cpu_thread_limit() -> usize {
    parse_positive_usize_env("RAYON_NUM_THREADS")
        .or_else(|| std::thread::available_parallelism().ok().map(usize::from))
        .unwrap_or(1)
        .max(1)
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn tir_optimization_resource_plan_from_limits(
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
pub(in crate::native_backend::simple_backend) fn tir_optimization_resource_plan()
-> TirOptimizationResourcePlan {
    tir_optimization_resource_plan_from_limits(
        tir_optimization_cpu_thread_limit(),
        backend_memory_limit_bytes(),
    )
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn should_flush_deferred_codegen(
    deferred_count: usize,
    deferred_ops: usize,
) -> bool {
    deferred_count > 0
        && (deferred_count >= DEFERRED_CODEGEN_FLUSH_FUNCTION_LIMIT
            || deferred_ops >= DEFERRED_CODEGEN_FLUSH_OP_BUDGET)
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn trace_tir_function_enabled(name: &str) -> bool {
    std::env::var("MOLT_TIR_TRACE_FUNC")
        .ok()
        .is_some_and(|filter| filter == "1" || name.contains(&filter))
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn trace_tir_function_stage(
    name: &str,
    stage: &str,
    simple_ops: usize,
) {
    if trace_tir_function_enabled(name) {
        eprintln!("[TIR-TRACE] {name} {stage}: simple_ops={simple_ops}");
    }
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn native_tir_cache_hash_body(
    simple_body_bytes: &[u8],
) -> Vec<u8> {
    let mut body = b"native-tir-function-cache-v1\0".to_vec();
    body.extend_from_slice(simple_body_bytes);
    body
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::simple_backend) fn optimize_tir_input(
    input: TirOptimizationInput,
) -> TirOptimizationOutput {
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
    let lir_func =
        crate::tir::lower_to_lir::lower_function_to_lir_for_repr_fact_extraction(&tir_func);
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
