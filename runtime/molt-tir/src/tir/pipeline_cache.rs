//! Shared cached SimpleIR -> TIR optimization runner.
//!
//! Native and WASM both need the same authority for cache keys, batching, hit
//! restoration, miss optimization, artifact encoding, and index persistence.
//! Backend-specific code may prepare a function before lowering or consume the
//! optimized [`TirFunction`] afterward, but it must not open `CompilationCache`
//! directly for this pipeline.

use rayon::prelude::*;
use std::collections::{BTreeMap, HashSet};

use crate::{FunctionIR, OpIR};

use super::cache::{CompilationCache, backend_cache_dir};
use super::function::{TirFunction, TirModule};
use super::target_info::TargetInfo;

pub const TIR_OPTIMIZATION_BATCH_FUNCTION_LIMIT: usize = 128;
pub const TIR_OPTIMIZATION_BATCH_OP_BUDGET: usize = 8_000;
pub const TIR_OPTIMIZATION_BASELINE_MEMORY_BYTES: usize = 4 * 1024 * 1024 * 1024;
pub const TIR_OPTIMIZATION_WORKER_MEMORY_BYTES: usize = 8 * 1024 * 1024 * 1024;
pub const TIR_OPTIMIZATION_WAVE_FUNCTIONS_PER_THREAD: usize = 1;
pub const TIR_OPTIMIZATION_WAVE_OPS_PER_THREAD: usize = 1_000;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum TirPipelineCacheFlavor {
    Native,
    Llvm,
    Wasm,
}

impl TirPipelineCacheFlavor {
    fn cache_prefix(self) -> &'static [u8] {
        match self {
            Self::Native => b"native-tir-function-cache-v3\0",
            Self::Llvm => b"llvm-tir-function-cache-v1\0",
            Self::Wasm => b"wasm-tir-function-cache-v2\0",
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct TirOptimizationWorkItem {
    pub index: usize,
    pub content_hash: String,
    pub op_count: usize,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct TirOptimizationResourcePlan {
    pub threads: usize,
    pub wave_function_limit: usize,
    pub wave_op_budget: usize,
}

#[derive(Debug)]
pub struct CachedTirCustody {
    optimized_tir_by_name: BTreeMap<String, TirFunction>,
}

impl CachedTirCustody {
    fn new() -> Self {
        Self {
            optimized_tir_by_name: BTreeMap::new(),
        }
    }

    fn insert(&mut self, name: String, tir_func: TirFunction) {
        self.optimized_tir_by_name.insert(name, tir_func);
    }

    fn remove_required(&mut self, name: &str, missing_tir_context: &str) -> TirFunction {
        self.optimized_tir_by_name.remove(name).unwrap_or_else(|| {
            panic!("{missing_tir_context} did not return optimized TIR for '{name}'")
        })
    }

    fn optimized_tir_by_name_mut(&mut self) -> &mut BTreeMap<String, TirFunction> {
        &mut self.optimized_tir_by_name
    }

    pub fn contains_function(&self, name: &str) -> bool {
        self.optimized_tir_by_name.contains_key(name)
    }
}

#[derive(Debug)]
pub struct TirPipelineRun {
    pub cached_tir: CachedTirCustody,
    pub uncached_count: usize,
}

pub struct TirPipelineRunOptions<'a> {
    pub target_info: TargetInfo,
    pub cache_flavor: TirPipelineCacheFlavor,
    pub cache_dir: Option<std::path::PathBuf>,
    pub process_externs: bool,
    pub verify_lir: bool,
    pub tir_dump: bool,
    pub tir_stats: bool,
    pub progress_prefix: Option<&'a str>,
    pub resource_plan: TirOptimizationResourcePlan,
}

pub type TirSimpleIrModuleStageObserver<'a> =
    &'a mut dyn for<'stage> FnMut(TirSimpleIrModulePipelineStage<'stage>);

pub enum TirSimpleIrModulePipelineStage<'a> {
    BeforeModuleLower {
        functions: &'a [FunctionIR],
    },
    AfterModuleLower {
        module: &'a TirModule,
    },
    AfterModulePipeline {
        module: &'a TirModule,
        changed_functions: usize,
        elapsed_ms: u128,
    },
    AfterModuleBackconvert {
        functions: &'a [FunctionIR],
        changed_functions: usize,
    },
}

pub struct TirSimpleIrModulePipelineOptions<'a> {
    pub target_info: &'a TargetInfo,
    pub module_name: &'a str,
    pub non_inlinable: &'a HashSet<String>,
    pub missing_tir_context: &'a str,
    pub backconvert_context: &'a str,
    pub stage_observer: Option<TirSimpleIrModuleStageObserver<'a>>,
}

pub struct TirSimpleIrModulePipelineRun {
    pub module_analysis: super::module_phase::ModuleAnalysis,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum TirOwnedModulePipelineMode {
    ModulePhase,
    TerminalDropsOnly,
}

pub struct TirOwnedModulePipelineOptions<'a> {
    pub target_info: &'a TargetInfo,
    pub module_name: &'a str,
    pub non_inlinable: &'a HashSet<String>,
    pub missing_tir_context: &'a str,
    pub mode: TirOwnedModulePipelineMode,
}

pub struct TirOwnedModulePipelineRun {
    pub tir_functions: Vec<(bool, TirFunction)>,
    pub module_analysis: Option<super::module_phase::ModuleAnalysis>,
}

struct TirOptimizationInput {
    index: usize,
    content_hash: String,
    name: String,
    params: Vec<String>,
    ops: Vec<OpIR>,
    param_types: Option<Vec<String>>,
}

struct TirOptimizationOutput {
    index: usize,
    content_hash: String,
    simple_ops: Vec<OpIR>,
    tir_func: TirFunction,
}

pub fn cached_tir_hash_body(
    cache_flavor: TirPipelineCacheFlavor,
    target_info: &TargetInfo,
    simple_body_bytes: &[u8],
) -> Vec<u8> {
    let mut body = cache_flavor.cache_prefix().to_vec();
    body.extend_from_slice(tir_pipeline_target_fingerprint(target_info).as_bytes());
    body.push(0);
    body.extend_from_slice(simple_body_bytes);
    body
}

pub fn content_hash_for_function(
    func_ir: &FunctionIR,
    cache_flavor: TirPipelineCacheFlavor,
    target_info: &TargetInfo,
) -> String {
    let body_bytes = super::serialize::serialize_ops(&func_ir.ops);
    let cache_hash_body = cached_tir_hash_body(cache_flavor, target_info, &body_bytes);
    CompilationCache::compute_hash_with_signature(
        &func_ir.name,
        &func_ir.params,
        func_ir.param_types.as_deref(),
        &cache_hash_body,
    )
}

pub fn tir_pipeline_target_fingerprint(target_info: &TargetInfo) -> String {
    format!(
        concat!(
            "target={:?};profile={:?};",
            "int_binop={};branch_mispredict={};call_overhead={};",
            "inline={};inline_hot={};pgo_hot={};",
            "unroll_trip={};unroll_body={};",
            "vec_i64={};vec_f64={};",
            "tile_l1={};tile_l2={};l1={};l2={};size={};",
            "os={};family={};arch={};ptr={};endian={};"
        ),
        target_info.target,
        target_info.profile,
        target_info.int_binop_cost,
        target_info.branch_mispredict_cost,
        target_info.call_overhead,
        target_info.inline_op_limit,
        target_info.inline_hot_op_limit,
        target_info.pgo_hot_call_threshold,
        target_info.unroll_max_trip,
        target_info.unroll_max_body,
        target_info.vector_width_i64,
        target_info.vector_width_f64,
        target_info.tile_l1,
        target_info.tile_l2,
        target_info.l1_cache_bytes,
        target_info.l2_cache_bytes,
        target_info.optimize_for_size,
        std::env::consts::OS,
        std::env::consts::FAMILY,
        std::env::consts::ARCH,
        target_pointer_width(),
        target_endianness(),
    )
}

fn target_pointer_width() -> &'static str {
    if cfg!(target_pointer_width = "64") {
        "64"
    } else if cfg!(target_pointer_width = "32") {
        "32"
    } else if cfg!(target_pointer_width = "16") {
        "16"
    } else {
        "unknown"
    }
}

fn target_endianness() -> &'static str {
    if cfg!(target_endian = "little") {
        "little"
    } else if cfg!(target_endian = "big") {
        "big"
    } else {
        "unknown"
    }
}

pub fn partition_tir_optimization_work_items_with_limits(
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

#[cfg(test)]
pub fn partition_tir_optimization_work_items(
    work_items: Vec<TirOptimizationWorkItem>,
) -> Vec<Vec<TirOptimizationWorkItem>> {
    partition_tir_optimization_work_items_with_limits(
        work_items,
        TIR_OPTIMIZATION_BATCH_FUNCTION_LIMIT,
        TIR_OPTIMIZATION_BATCH_OP_BUDGET,
    )
}

pub fn parse_positive_usize_env(name: &str) -> Option<usize> {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
}

pub fn parse_nonnegative_gb_env(name: &str) -> Option<usize> {
    let gb = std::env::var(name)
        .ok()
        .and_then(|raw| raw.trim().parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value >= 0.0)?;
    Some((gb * 1024.0 * 1024.0 * 1024.0) as usize)
}

pub fn env_memory_limit_bytes() -> Option<usize> {
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

#[cfg(unix)]
pub fn rlimit_address_space_bytes() -> Option<usize> {
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

#[cfg(not(unix))]
pub fn rlimit_address_space_bytes() -> Option<usize> {
    None
}

pub fn backend_memory_limit_bytes() -> Option<usize> {
    match (env_memory_limit_bytes(), rlimit_address_space_bytes()) {
        (Some(env_limit), Some(rlimit)) => Some(env_limit.min(rlimit)),
        (Some(env_limit), None) => Some(env_limit),
        (None, Some(rlimit)) => Some(rlimit),
        (None, None) => None,
    }
}

pub fn tir_optimization_cpu_thread_limit() -> usize {
    parse_positive_usize_env("RAYON_NUM_THREADS")
        .or_else(|| std::thread::available_parallelism().ok().map(usize::from))
        .unwrap_or(1)
        .max(1)
}

pub fn tir_optimization_resource_plan_from_limits(
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

pub fn tir_optimization_resource_plan() -> TirOptimizationResourcePlan {
    tir_optimization_resource_plan_from_limits(
        tir_optimization_cpu_thread_limit(),
        backend_memory_limit_bytes(),
    )
}

pub fn trace_tir_function_enabled(name: &str) -> bool {
    std::env::var("MOLT_TIR_TRACE_FUNC")
        .ok()
        .is_some_and(|filter| filter == "1" || name.contains(&filter))
}

pub fn trace_tir_function_stage(name: &str, stage: &str, simple_ops: usize) {
    if trace_tir_function_enabled(name) {
        eprintln!("[TIR-TRACE] {name} {stage}: simple_ops={simple_ops}");
    }
}

pub fn run_cached_tir_pipeline<F>(
    functions: &mut [FunctionIR],
    options: TirPipelineRunOptions<'_>,
    preprocess_before_lowering: F,
) -> TirPipelineRun
where
    F: Fn(&mut FunctionIR) + Sync,
{
    let mut cached_tir_custody = CachedTirCustody::new();
    let mut tir_cache =
        CompilationCache::open(options.cache_dir.clone().unwrap_or_else(backend_cache_dir));
    let mut work_items: Vec<TirOptimizationWorkItem> = Vec::new();

    for (i, func_ir) in functions.iter_mut().enumerate() {
        if func_ir.is_extern && !options.process_externs {
            continue;
        }

        let content_hash =
            content_hash_for_function(func_ir, options.cache_flavor, &options.target_info);
        if let Some(cached_bytes) = tir_cache.get(&content_hash)
            && let Some(cached_tir_func) = super::serialize::deserialize_tir_function(&cached_bytes)
        {
            verify_lir_if_requested(&cached_tir_func, options.verify_lir);
            let cached_ops = super::lower_to_simple::lower_to_simple_ir(&cached_tir_func);
            assert!(
                super::lower_to_simple::validate_labels(&cached_ops),
                "cached TIR back-conversion emitted invalid labels for '{}'",
                cached_tir_func.name
            );
            func_ir.ops = cached_ops;
            cached_tir_custody.insert(func_ir.name.clone(), cached_tir_func);
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
        let work_batches = partition_tir_optimization_work_items_with_limits(
            work_items,
            options.resource_plan.wave_function_limit,
            options.resource_plan.wave_op_budget,
        );
        let batch_count = work_batches.len();
        if let Some(prefix) = options.progress_prefix {
            if batch_count == 1 {
                eprintln!(
                    "{prefix}: TIR optimizing {uncached_count} uncached functions with {} worker(s)",
                    options.resource_plan.threads
                );
            } else {
                eprintln!(
                    "{prefix}: TIR optimizing {uncached_count} uncached functions in {batch_count} bounded waves with {} worker(s)",
                    options.resource_plan.threads
                );
            }
        }
        let tir_start = std::time::Instant::now();
        let tir_pool = rayon::ThreadPoolBuilder::new()
            .num_threads(options.resource_plan.threads)
            .stack_size(64 * 1024 * 1024)
            .build()
            .expect("Failed to build TIR thread pool");
        for (batch_idx, batch_items) in work_batches.into_iter().enumerate() {
            let batch_ops = batch_items.iter().map(|wi| wi.op_count).sum::<usize>();
            if let Some(prefix) = options.progress_prefix
                && batch_count > 1
            {
                eprintln!(
                    "{prefix}: TIR batch {}/{} ({} functions, {} ops / budget {})",
                    batch_idx + 1,
                    batch_count,
                    batch_items.len(),
                    batch_ops,
                    options.resource_plan.wave_op_budget
                );
            }
            let inputs: Vec<TirOptimizationInput> = batch_items
                .into_iter()
                .map(|wi| {
                    let func_ir = &functions[wi.index];
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
            let results: Vec<TirOptimizationOutput> = tir_pool.install(|| {
                inputs
                    .into_par_iter()
                    .map(|input| {
                        optimize_tir_input(
                            input,
                            &options.target_info,
                            options.tir_dump,
                            options.tir_stats,
                            options.verify_lir,
                            &preprocess_before_lowering,
                        )
                    })
                    .collect()
            });

            for output in results {
                let func_ir = &mut functions[output.index];
                func_ir.ops = output.simple_ops;
                let bytes = super::serialize::serialize_tir_function(&output.tir_func);
                tir_cache.put(&output.content_hash, &bytes, vec![]);
                cached_tir_custody.insert(func_ir.name.clone(), output.tir_func);
            }
        }

        if let Some(prefix) = options.progress_prefix {
            let tir_elapsed = tir_start.elapsed();
            eprintln!(
                "{prefix}: TIR parallel optimization took {tir_elapsed:.2?} for {uncached_count} functions"
            );
        }
    }

    tir_cache.save_index();
    TirPipelineRun {
        cached_tir: cached_tir_custody,
        uncached_count,
    }
}

pub fn run_simple_ir_module_pipeline_from_cached_tir(
    functions: &mut [FunctionIR],
    cached_tir: &mut CachedTirCustody,
    mut options: TirSimpleIrModulePipelineOptions<'_>,
) -> TirSimpleIrModulePipelineRun {
    emit_simple_ir_module_stage(
        &mut options.stage_observer,
        TirSimpleIrModulePipelineStage::BeforeModuleLower { functions },
    );
    let (mut module, idx_map) = take_local_tir_module_from_cached_tir(
        functions,
        cached_tir,
        options.module_name,
        options.missing_tir_context,
    );
    emit_simple_ir_module_stage(
        &mut options.stage_observer,
        TirSimpleIrModulePipelineStage::AfterModuleLower { module: &module },
    );
    let module_pipeline_start = std::time::Instant::now();
    let module_analysis = super::module_phase::run_module_pipeline(
        &mut module,
        options.target_info,
        options.non_inlinable,
    );
    let module_pipeline_elapsed_ms = module_pipeline_start.elapsed().as_millis();
    emit_simple_ir_module_stage(
        &mut options.stage_observer,
        TirSimpleIrModulePipelineStage::AfterModulePipeline {
            module: &module,
            changed_functions: module_analysis.changed_functions.len(),
            elapsed_ms: module_pipeline_elapsed_ms,
        },
    );
    backconvert_changed_tir_module_to_simple_ir(
        functions,
        &module,
        &idx_map,
        &module_analysis.changed_functions,
        options.backconvert_context,
    );
    emit_simple_ir_module_stage(
        &mut options.stage_observer,
        TirSimpleIrModulePipelineStage::AfterModuleBackconvert {
            functions,
            changed_functions: module_analysis.changed_functions.len(),
        },
    );
    TirSimpleIrModulePipelineRun { module_analysis }
}

pub fn finalize_simple_ir_drops_from_cached_tir(
    functions: &mut [FunctionIR],
    target_info: &TargetInfo,
    cached_tir: &mut CachedTirCustody,
) {
    super::drop_phase::finalize_simple_ir_drops_with_tir_custody(
        functions,
        target_info,
        cached_tir.optimized_tir_by_name_mut(),
    );
}

pub fn run_owned_module_pipeline_from_cached_tir(
    functions: &[FunctionIR],
    cached_tir: &mut CachedTirCustody,
    options: TirOwnedModulePipelineOptions<'_>,
) -> TirOwnedModulePipelineRun {
    let mut tir_functions = take_ordered_tir_functions_from_cached_tir(
        functions,
        cached_tir,
        options.missing_tir_context,
    );
    let module_analysis = match options.mode {
        TirOwnedModulePipelineMode::ModulePhase => {
            let mut externs: Vec<TirFunction> = Vec::new();
            let mut module = TirModule {
                name: options.module_name.to_string(),
                functions: Vec::new(),
            };
            for (is_extern, tir_func) in tir_functions.into_iter() {
                if is_extern {
                    externs.push(tir_func);
                } else {
                    module.functions.push(tir_func);
                }
            }
            let module_analysis = super::module_phase::run_module_pipeline(
                &mut module,
                options.target_info,
                options.non_inlinable,
            );
            tir_functions = Vec::with_capacity(externs.len() + module.functions.len());
            tir_functions.extend(externs.into_iter().map(|func| (true, func)));
            tir_functions.extend(module.functions.into_iter().map(|func| (false, func)));
            Some(module_analysis)
        }
        TirOwnedModulePipelineMode::TerminalDropsOnly => {
            for (is_extern, tir_func) in tir_functions.iter_mut() {
                if !*is_extern {
                    let _ =
                        super::drop_phase::finalize_function_drops(tir_func, options.target_info);
                }
            }
            None
        }
    };
    TirOwnedModulePipelineRun {
        tir_functions,
        module_analysis,
    }
}

fn emit_simple_ir_module_stage(
    stage_observer: &mut Option<TirSimpleIrModuleStageObserver<'_>>,
    stage: TirSimpleIrModulePipelineStage<'_>,
) {
    if let Some(observer) = stage_observer.as_mut() {
        (*observer)(stage);
    }
}

fn take_local_tir_module_from_cached_tir(
    functions: &[FunctionIR],
    cached_tir: &mut CachedTirCustody,
    module_name: &str,
    missing_tir_context: &str,
) -> (TirModule, Vec<usize>) {
    let mut tir_functions = Vec::new();
    let mut idx_map = Vec::new();
    for (idx, func_ir) in functions.iter().enumerate() {
        if func_ir.is_extern {
            continue;
        }
        let tir_func = cached_tir.remove_required(&func_ir.name, missing_tir_context);
        tir_functions.push(tir_func);
        idx_map.push(idx);
    }
    (
        TirModule {
            name: module_name.to_string(),
            functions: tir_functions,
        },
        idx_map,
    )
}

fn take_ordered_tir_functions_from_cached_tir(
    functions: &[FunctionIR],
    cached_tir: &mut CachedTirCustody,
    missing_tir_context: &str,
) -> Vec<(bool, TirFunction)> {
    functions
        .iter()
        .map(|func_ir| {
            let tir_func = if func_ir.is_extern {
                super::lower_from_simple::lower_to_tir(func_ir)
            } else {
                cached_tir.remove_required(&func_ir.name, missing_tir_context)
            };
            (func_ir.is_extern, tir_func)
        })
        .collect()
}

fn backconvert_changed_tir_module_to_simple_ir(
    functions: &mut [FunctionIR],
    module: &TirModule,
    idx_map: &[usize],
    changed_functions: &[String],
    backconvert_context: &str,
) {
    let changed: HashSet<&str> = changed_functions.iter().map(String::as_str).collect();
    for (pos, &orig_idx) in idx_map.iter().enumerate() {
        let tir_func = &module.functions[pos];
        if !changed.contains(tir_func.name.as_str()) {
            continue;
        }
        let ops = super::lower_to_simple::lower_to_simple_ir(tir_func);
        debug_assert!(
            super::lower_to_simple::validate_labels(&ops),
            "{backconvert_context} back-conversion emitted invalid labels for '{}'",
            tir_func.name
        );
        functions[orig_idx].ops = ops;
    }
}

fn optimize_tir_input<F>(
    input: TirOptimizationInput,
    target_info: &TargetInfo,
    tir_dump: bool,
    tir_stats: bool,
    verify_lir: bool,
    preprocess_before_lowering: &F,
) -> TirOptimizationOutput
where
    F: Fn(&mut FunctionIR) + Sync,
{
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
    preprocess_before_lowering(&mut tmp_func);

    let func_name = tmp_func.name.clone();
    let mut tir_func = super::lower_from_simple::lower_to_tir(&tmp_func);
    if trace_tir_function_enabled(&func_name) {
        trace_tir_blocks(&func_name, "after_lower_to_tir", &tir_func);
    }
    super::type_refine::refine_types(&mut tir_func);
    if trace_tir_function_enabled(&func_name) {
        trace_tir_blocks(&func_name, "after_refine_1", &tir_func);
    }
    let stats = super::passes::run_pipeline(&mut tir_func, target_info);
    if trace_tir_function_enabled(&func_name) {
        trace_tir_blocks(&func_name, "after_pipeline", &tir_func);
    }
    super::type_refine::refine_types(&mut tir_func);
    if trace_tir_function_enabled(&func_name) {
        trace_tir_blocks(&func_name, "after_refine_2", &tir_func);
    }
    if tir_dump {
        eprintln!("{}", super::printer::print_function(&tir_func));
    }
    if tir_stats {
        for s in &stats {
            eprintln!(
                "[TIR] {}: {} values changed, {} attrs changed, {} removed, {} added",
                s.name, s.values_changed, s.attrs_changed, s.ops_removed, s.ops_added
            );
        }
    }
    verify_lir_if_requested(&tir_func, verify_lir);
    let ops = super::lower_to_simple::lower_to_simple_ir(&tir_func);
    trace_tir_function_stage(&func_name, "after_lower_to_simple", ops.len());
    assert!(
        super::lower_to_simple::validate_labels(&ops),
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

fn verify_lir_if_requested(tir_func: &TirFunction, verify_lir: bool) {
    if !verify_lir {
        return;
    }
    let func_name = &tir_func.name;
    let lir_func = super::lower_to_lir::lower_function_to_lir_for_repr_fact_extraction(tir_func);
    if trace_tir_function_enabled(func_name) {
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
    if let Err(errors) = super::verify_lir::verify_lir_function(&lir_func) {
        panic!(
            "[LIR] verification failed for '{}': {:?}",
            func_name, errors
        );
    }
    #[cfg(debug_assertions)]
    {
        let repr_violations = super::verify_lir_repr::verify_register_passable(&lir_func);
        if !repr_violations.is_empty() {
            eprintln!(
                "[LIR-repr] {} register-passable violation(s) in '{}': {:?}",
                repr_violations.len(),
                func_name,
                repr_violations,
            );
        }
    }
}

fn trace_tir_blocks(name: &str, stage: &str, tir_func: &TirFunction) {
    eprintln!(
        "[TIR-TRACE] {name} {stage}: blocks={} ops={}",
        tir_func.blocks.len(),
        tir_func
            .blocks
            .values()
            .map(|block| block.ops.len())
            .sum::<usize>()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_partition_respects_count_and_op_budgets() {
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
    fn work_partition_accepts_inflight_limits() {
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
    fn resource_plan_caps_inflight_work_by_memory_limit() {
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
    fn resource_plan_serializes_under_twelve_gb_guard() {
        let memory_limit = 12 * 1024 * 1024 * 1024;

        let plan = tir_optimization_resource_plan_from_limits(8, Some(memory_limit));

        assert_eq!(plan.threads, 1);
        assert_eq!(plan.wave_function_limit, 1);
        assert_eq!(plan.wave_op_budget, TIR_OPTIMIZATION_WAVE_OPS_PER_THREAD);
    }

    #[test]
    fn resource_plan_keeps_cpu_parallelism_without_memory_limit() {
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
    fn cache_hash_includes_target_and_platform_fingerprint() {
        let func = FunctionIR {
            name: "f".to_string(),
            params: vec!["x".to_string()],
            ops: vec![OpIR {
                kind: "const_int".to_string(),
                out: Some("x".to_string()),
                value: Some(1),
                ..Default::default()
            }],
            param_types: Some(vec!["int".to_string()]),
            source_file: None,
            is_extern: false,
        };

        let native = TargetInfo::native_release_fast();
        let wasm = TargetInfo::wasm_release_fast();

        assert_ne!(
            tir_pipeline_target_fingerprint(&native),
            tir_pipeline_target_fingerprint(&wasm)
        );
        assert!(tir_pipeline_target_fingerprint(&native).contains(std::env::consts::OS));
        assert!(tir_pipeline_target_fingerprint(&native).contains(std::env::consts::ARCH));
        assert_ne!(
            content_hash_for_function(&func, TirPipelineCacheFlavor::Native, &native),
            content_hash_for_function(&func, TirPipelineCacheFlavor::Native, &wasm)
        );
        assert_ne!(
            content_hash_for_function(&func, TirPipelineCacheFlavor::Native, &native),
            content_hash_for_function(&func, TirPipelineCacheFlavor::Wasm, &wasm)
        );
    }
}
