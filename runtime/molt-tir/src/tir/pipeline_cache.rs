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

use super::cache::{
    CompilationCache, CompilationCacheKey, CompilationCacheWriteError, backend_cache_dir,
};
use super::function::{TirFunction, TirModule};
use super::target_info::TargetInfo;

pub const TIR_OPTIMIZATION_BATCH_FUNCTION_LIMIT: usize = 128;
pub const TIR_OPTIMIZATION_BATCH_OP_BUDGET: usize = 8_000;
pub const TIR_OPTIMIZATION_BASELINE_MEMORY_BYTES: u64 = 4 * 1024 * 1024 * 1024;
pub const TIR_OPTIMIZATION_WORKER_MEMORY_BYTES: u64 = 8 * 1024 * 1024 * 1024;
pub const TIR_OPTIMIZATION_WAVE_FUNCTIONS_PER_THREAD: usize = 1;
pub const TIR_OPTIMIZATION_WAVE_OPS_PER_THREAD: usize = 1_000;

const GIB_BYTES: u64 = 1024 * 1024 * 1024;
const TIR_PIPELINE_SEMANTIC_EPOCH: &[u8] = b"molt-tir-pipeline-semantics-v1";

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum TirPipelineCacheFlavor {
    Native,
    Llvm,
    Wasm,
    Luau,
    FactGraph,
}

impl TirPipelineCacheFlavor {
    fn cache_discriminant(self) -> &'static [u8] {
        match self {
            Self::Native => b"native",
            Self::Llvm => b"llvm",
            Self::Wasm => b"wasm",
            Self::Luau => b"luau",
            Self::FactGraph => b"fact-graph",
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

struct PreparedTirFunction {
    function: FunctionIR,
    content_hash: String,
}

impl PreparedTirFunction {
    fn new<F>(
        source: &FunctionIR,
        cache_flavor: TirPipelineCacheFlavor,
        target_info: &TargetInfo,
        preprocess_before_lowering: &F,
    ) -> Self
    where
        F: Fn(&mut FunctionIR) + ?Sized,
    {
        let mut function = source.clone();
        trace_tir_function_stage(&function.name, "start", function.ops.len());
        preprocess_before_lowering(&mut function);
        let content_hash = content_hash_for_function(&function, cache_flavor, target_info);
        Self {
            function,
            content_hash,
        }
    }
}

struct TirPreparationWorkItem {
    index: usize,
    op_count: usize,
}

struct TirOptimizationInput {
    index: usize,
    prepared_function: PreparedTirFunction,
}

struct TirOptimizationOutput {
    index: usize,
    content_hash: String,
    simple_ops: Vec<OpIR>,
    tir_func: TirFunction,
}

pub fn content_hash_for_function(
    func_ir: &FunctionIR,
    cache_flavor: TirPipelineCacheFlavor,
    target_info: &TargetInfo,
) -> String {
    let mut key = CompilationCacheKey::new(b"molt-cached-tir-prepared-function-key-v1");
    key.field(b"pipeline-semantic-epoch", TIR_PIPELINE_SEMANTIC_EPOCH);
    key.field(b"cache-flavor", cache_flavor.cache_discriminant());
    let target_fingerprint = tir_pipeline_target_fingerprint(target_info);
    key.field(b"target-contract", target_fingerprint.as_bytes());
    key.digest_field(b"function-ir-contract", |writer| {
        crate::write_function_ir_contract(func_ir, writer)
    })
    .expect("digest-backed FunctionIR contract serialization cannot fail");
    key.finish_hex()
}

pub fn tir_pipeline_target_fingerprint(target_info: &TargetInfo) -> String {
    let TargetInfo {
        target,
        profile,
        extern_function_linkage,
        supported_numeric_semantics,
        supported_runtime_semantics,
        int_binop_cost,
        branch_mispredict_cost,
        call_overhead,
        inline_op_limit,
        inline_hot_op_limit,
        pgo_hot_call_threshold,
        unroll_max_trip,
        unroll_max_body,
        vector_width_i64,
        vector_width_f64,
        tile_l1,
        tile_l2,
        l1_cache_bytes,
        l2_cache_bytes,
        optimize_for_size,
        profile_data,
    } = target_info;
    let super::target_info::NumericTargetCapabilities {
        arbitrary_precision_integers,
        exact_integer_literal_max_magnitude,
        cpython_float_divmod,
        cpython_power,
    } = supported_numeric_semantics;
    let hot_functions = profile_data.as_ref().map_or_else(
        || "none".to_string(),
        |super::target_info::ProfileData { hot_functions }| {
            hot_functions
                .iter()
                .map(|name| format!("{}:{name}", name.len()))
                .collect::<Vec<_>>()
                .join(",")
        },
    );
    let platform = target_platform_fingerprint(*target);
    format!(
        concat!(
            "target={};profile={:?};extern={};",
            "numeric_bigint={};numeric_literal={:?};numeric_divmod={};numeric_power={};",
            "runtime_requirements={};pgo_hot_functions={};",
            "int_binop={};branch_mispredict={};call_overhead={};",
            "inline={};inline_hot={};pgo_hot={};",
            "unroll_trip={};unroll_body={};",
            "vec_i64={};vec_f64={};",
            "tile_l1={};tile_l2={};l1={};l2={};size={};",
            "platform={};"
        ),
        target.as_str(),
        profile,
        extern_function_linkage,
        arbitrary_precision_integers,
        exact_integer_literal_max_magnitude,
        cpython_float_divmod,
        cpython_power,
        supported_runtime_semantics.bits(),
        hot_functions,
        int_binop_cost,
        branch_mispredict_cost,
        call_overhead,
        inline_op_limit,
        inline_hot_op_limit,
        pgo_hot_call_threshold,
        unroll_max_trip,
        unroll_max_body,
        vector_width_i64,
        vector_width_f64,
        tile_l1,
        tile_l2,
        l1_cache_bytes,
        l2_cache_bytes,
        optimize_for_size,
        platform,
    )
}

fn target_platform_fingerprint(target: super::target_info::TargetKind) -> String {
    use super::target_info::TargetKind;

    match target {
        TargetKind::NativeCranelift | TargetKind::Llvm => format!(
            "host-os={};host-family={};host-arch={};host-ptr={};host-endian={}",
            std::env::consts::OS,
            std::env::consts::FAMILY,
            std::env::consts::ARCH,
            usize::BITS,
            if cfg!(target_endian = "little") {
                "little"
            } else {
                "big"
            },
        ),
        TargetKind::Wasm => "wasm32;ptr=32;endian=little".to_string(),
        TargetKind::Luau => "portable-luau-source".to_string(),
        TargetKind::Rust => "portable-rust-source".to_string(),
        TargetKind::Mlir => "portable-mlir".to_string(),
    }
}

pub fn partition_tir_optimization_work_items_with_limits(
    work_items: Vec<TirOptimizationWorkItem>,
    max_functions_per_batch: usize,
    max_ops_per_batch: usize,
) -> Vec<Vec<TirOptimizationWorkItem>> {
    partition_work_items_with_limits(
        work_items,
        max_functions_per_batch,
        max_ops_per_batch,
        |item| item.op_count,
    )
}

fn partition_work_items_with_limits<T, F>(
    work_items: Vec<T>,
    max_functions_per_batch: usize,
    max_ops_per_batch: usize,
    op_count: F,
) -> Vec<Vec<T>>
where
    F: Fn(&T) -> usize,
{
    let max_functions = max_functions_per_batch.max(1);
    let max_ops = max_ops_per_batch.max(1);
    let mut batches: Vec<Vec<T>> = Vec::new();
    let mut current: Vec<T> = Vec::new();
    let mut current_ops = 0usize;

    for item in work_items {
        let item_ops = op_count(&item).max(1);
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

pub fn parse_nonnegative_gb_env(name: &str) -> Option<u64> {
    let gb = std::env::var(name)
        .ok()
        .and_then(|raw| raw.trim().parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value >= 0.0)?;
    Some((gb * GIB_BYTES as f64).min(u64::MAX as f64) as u64)
}

pub fn env_memory_limit_bytes() -> Option<u64> {
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
pub fn rlimit_address_space_bytes() -> Option<u64> {
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
        let widened = u128::from(raw);
        Some(widened.min(u128::from(u64::MAX)) as u64)
    }
}

#[cfg(not(unix))]
pub fn rlimit_address_space_bytes() -> Option<u64> {
    None
}

pub fn backend_memory_limit_bytes() -> Option<u64> {
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
    memory_limit_bytes: Option<u64>,
) -> TirOptimizationResourcePlan {
    let cpu_threads = cpu_threads.max(1);
    let memory_threads = memory_limit_bytes
        .map(|limit| {
            if limit <= TIR_OPTIMIZATION_BASELINE_MEMORY_BYTES {
                1
            } else {
                let worker_threads = ((limit - TIR_OPTIMIZATION_BASELINE_MEMORY_BYTES)
                    / TIR_OPTIMIZATION_WORKER_MEMORY_BYTES)
                    .max(1);
                usize::try_from(worker_threads).unwrap_or(usize::MAX)
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
    let work_items: Vec<TirPreparationWorkItem> = functions
        .iter()
        .enumerate()
        .filter(|(_, function)| options.process_externs || !function.is_extern)
        .map(|(index, function)| TirPreparationWorkItem {
            index,
            op_count: function.ops.len(),
        })
        .collect();
    let mut uncached_count = 0usize;
    if !work_items.is_empty() {
        let work_batches = partition_work_items_with_limits(
            work_items,
            options.resource_plan.wave_function_limit,
            options.resource_plan.wave_op_budget,
            |item| item.op_count,
        );
        let batch_count = work_batches.len();
        let tir_start = std::time::Instant::now();
        let tir_pool = rayon::ThreadPoolBuilder::new()
            .num_threads(options.resource_plan.threads)
            .stack_size(64 * 1024 * 1024)
            .build()
            .expect("Failed to build TIR thread pool");
        for (batch_idx, batch_items) in work_batches.into_iter().enumerate() {
            let inputs: Vec<TirOptimizationInput> = tir_pool.install(|| {
                batch_items
                    .par_iter()
                    .map(|work_item| TirOptimizationInput {
                        index: work_item.index,
                        prepared_function: PreparedTirFunction::new(
                            &functions[work_item.index],
                            options.cache_flavor,
                            &options.target_info,
                            &preprocess_before_lowering,
                        ),
                    })
                    .collect()
            });
            let prepared_ops = inputs
                .iter()
                .map(|input| input.prepared_function.function.ops.len())
                .sum::<usize>();
            let prepared_count = inputs.len();
            let mut misses = Vec::new();
            for input in inputs {
                let index = input.index;
                let content_hash = &input.prepared_function.content_hash;
                if let Some(cached_bytes) = tir_cache.get(content_hash)
                    && let Some(cached_tir_func) =
                        super::serialize::deserialize_tir_function(&cached_bytes)
                {
                    verify_lir_if_requested(&cached_tir_func, options.verify_lir);
                    let cached_ops = super::lower_to_simple::lower_to_simple_ir(&cached_tir_func);
                    assert!(
                        super::lower_to_simple::validate_labels(&cached_ops),
                        "cached TIR back-conversion emitted invalid labels for '{}'",
                        cached_tir_func.name
                    );
                    let func_ir = &mut functions[index];
                    func_ir.ops = cached_ops;
                    cached_tir_custody.insert(func_ir.name.clone(), cached_tir_func);
                } else {
                    misses.push(input);
                }
            }

            uncached_count += misses.len();
            if let Some(prefix) = options.progress_prefix
                && !misses.is_empty()
            {
                if batch_count == 1 {
                    eprintln!(
                        "{prefix}: TIR optimizing {} uncached functions with {} worker(s)",
                        misses.len(),
                        options.resource_plan.threads
                    );
                } else {
                    eprintln!(
                        "{prefix}: TIR wave {}/{} ({} uncached / {} prepared functions, {} prepared ops / budget {})",
                        batch_idx + 1,
                        batch_count,
                        misses.len(),
                        prepared_count,
                        prepared_ops,
                        options.resource_plan.wave_op_budget
                    );
                }
            }
            let results: Vec<TirOptimizationOutput> = tir_pool.install(|| {
                misses
                    .into_par_iter()
                    .map(|input| {
                        optimize_tir_input(
                            input,
                            &options.target_info,
                            options.tir_dump,
                            options.tir_stats,
                            options.verify_lir,
                        )
                    })
                    .collect()
            });

            for output in results {
                let func_ir = &mut functions[output.index];
                func_ir.ops = output.simple_ops;
                let bytes = super::serialize::serialize_tir_function(&output.tir_func)
                    .unwrap_or_else(|error| {
                        panic!(
                            "cached TIR serialization failed for '{}': {error}",
                            output.tir_func.name
                        )
                    });
                if let Err(error) = tir_cache.put(&output.content_hash, &bytes) {
                    match error {
                        CompilationCacheWriteError::Integrity(error) => {
                            panic!(
                                "persistent TIR cache rejected artifact for '{}': {error}",
                                func_ir.name
                            )
                        }
                        CompilationCacheWriteError::Unavailable(error) => {
                            eprintln!(
                                "MOLT_CACHE: TIR cache unavailable for '{}': {error}",
                                func_ir.name
                            );
                        }
                    }
                }
                cached_tir_custody.insert(func_ir.name.clone(), output.tir_func);
            }
        }

        if let Some(prefix) = options.progress_prefix
            && uncached_count > 0
        {
            let tir_elapsed = tir_start.elapsed();
            eprintln!(
                "{prefix}: TIR parallel optimization took {tir_elapsed:.2?} for {uncached_count} functions"
            );
        }
    }

    if let Err(error) = tir_cache.save_index() {
        eprintln!("MOLT_CACHE: persistent TIR cache index unavailable: {error}");
    }
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

fn optimize_tir_input(
    input: TirOptimizationInput,
    target_info: &TargetInfo,
    tir_dump: bool,
    tir_stats: bool,
    verify_lir: bool,
) -> TirOptimizationOutput {
    let idx = input.index;
    let PreparedTirFunction {
        function: tmp_func,
        content_hash,
    } = input.prepared_function;

    let func_name = tmp_func.name.clone();
    let mut tir_func = super::lower_from_simple::lower_to_tir_for_target(&tmp_func, target_info);
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
        let mut func = FunctionIR {
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
            execution_context: crate::ExecutionContextPolicy::None,
        };

        let native = TargetInfo::native_release_fast();
        let wasm = TargetInfo::wasm_release_fast();

        assert_ne!(
            tir_pipeline_target_fingerprint(&native),
            tir_pipeline_target_fingerprint(&wasm)
        );
        let native_fingerprint = tir_pipeline_target_fingerprint(&native);
        let mut semantic_mutations = Vec::new();
        let mut changed = native.clone();
        changed.extern_function_linkage = !changed.extern_function_linkage;
        semantic_mutations.push(changed);
        let mut changed = native.clone();
        changed = changed.with_profile_data(super::super::target_info::ProfileData {
            hot_functions: std::collections::BTreeSet::from(["hot".to_string()]),
        });
        semantic_mutations.push(changed);
        let mut changed = native.clone();
        changed
            .supported_numeric_semantics
            .arbitrary_precision_integers = false;
        semantic_mutations.push(changed);
        let mut changed = native.clone();
        changed
            .supported_numeric_semantics
            .exact_integer_literal_max_magnitude = Some(1 << 53);
        semantic_mutations.push(changed);
        let mut changed = native.clone();
        changed.supported_numeric_semantics.cpython_float_divmod = false;
        semantic_mutations.push(changed);
        let mut changed = native.clone();
        changed.supported_numeric_semantics.cpython_power = false;
        semantic_mutations.push(changed);
        let mut changed = native.clone();
        changed.supported_runtime_semantics = changed.supported_runtime_semantics.difference(
            crate::tir::op_kinds_generated::SimpleIrRuntimeRequirements::PENDING_CALL_EVAL_BREAKER,
        );
        semantic_mutations.push(changed);
        for changed in semantic_mutations {
            assert_ne!(
                native_fingerprint,
                tir_pipeline_target_fingerprint(&changed),
                "every semantic target field must participate in persistent TIR cache identity"
            );
        }
        assert!(tir_pipeline_target_fingerprint(&native).contains(std::env::consts::OS));
        assert!(tir_pipeline_target_fingerprint(&native).contains(std::env::consts::ARCH));
        let wasm_fingerprint = tir_pipeline_target_fingerprint(&wasm);
        assert!(wasm_fingerprint.contains("platform=wasm32;ptr=32;endian=little"));
        assert!(!wasm_fingerprint.contains("host-os="));
        assert_ne!(
            content_hash_for_function(&func, TirPipelineCacheFlavor::Native, &native),
            content_hash_for_function(&func, TirPipelineCacheFlavor::Native, &wasm)
        );
        assert_ne!(
            content_hash_for_function(&func, TirPipelineCacheFlavor::Native, &native),
            content_hash_for_function(&func, TirPipelineCacheFlavor::Wasm, &wasm)
        );
        assert_ne!(
            content_hash_for_function(&func, TirPipelineCacheFlavor::Wasm, &wasm),
            content_hash_for_function(&func, TirPipelineCacheFlavor::Luau, &wasm)
        );
        assert_ne!(
            content_hash_for_function(&func, TirPipelineCacheFlavor::Native, &native),
            content_hash_for_function(&func, TirPipelineCacheFlavor::FactGraph, &native)
        );

        let without_source =
            content_hash_for_function(&func, TirPipelineCacheFlavor::FactGraph, &native);
        func.source_file = Some("app.py".to_string());
        let with_source =
            content_hash_for_function(&func, TirPipelineCacheFlavor::FactGraph, &native);
        func.source_file = Some("other.py".to_string());
        let with_other_source =
            content_hash_for_function(&func, TirPipelineCacheFlavor::FactGraph, &native);

        assert_ne!(without_source, with_source);
        assert_ne!(with_source, with_other_source);

        let baseline = func.clone();
        let baseline_hash =
            content_hash_for_function(&baseline, TirPipelineCacheFlavor::Native, &native);
        let mut contract_mutations = Vec::new();
        let mut changed = baseline.clone();
        changed.params.push("y".to_string());
        contract_mutations.push(changed);
        let mut changed = baseline.clone();
        changed.ops.push(OpIR {
            kind: "ret_void".to_string(),
            ..Default::default()
        });
        contract_mutations.push(changed);
        let mut changed = baseline.clone();
        changed.param_types = Some(vec!["float".to_string(), "int".to_string()]);
        contract_mutations.push(changed);
        let mut changed = baseline.clone();
        changed.source_file = Some("third.py".to_string());
        contract_mutations.push(changed);
        let mut changed = baseline.clone();
        changed.is_extern = true;
        contract_mutations.push(changed);
        for policy in [
            crate::ExecutionContextPolicy::Local,
            crate::ExecutionContextPolicy::Inherited,
        ] {
            let mut changed = baseline.clone();
            changed.execution_context = policy;
            contract_mutations.push(changed);
        }
        for changed in contract_mutations {
            assert_ne!(
                baseline_hash,
                content_hash_for_function(&changed, TirPipelineCacheFlavor::Native, &native),
                "every non-name FunctionIR semantic/ABI field must participate in cache identity"
            );
        }
    }

    #[test]
    fn one_semantic_epoch_and_exhaustive_flavor_discriminants_own_cache_invalidation() {
        assert_eq!(
            TIR_PIPELINE_SEMANTIC_EPOCH,
            b"molt-tir-pipeline-semantics-v1"
        );
        let discriminants = [
            TirPipelineCacheFlavor::Native.cache_discriminant(),
            TirPipelineCacheFlavor::Llvm.cache_discriminant(),
            TirPipelineCacheFlavor::Wasm.cache_discriminant(),
            TirPipelineCacheFlavor::Luau.cache_discriminant(),
            TirPipelineCacheFlavor::FactGraph.cache_discriminant(),
        ];
        assert_eq!(
            discriminants.into_iter().collect::<HashSet<_>>().len(),
            discriminants.len(),
            "each cache flavor needs one stable discriminator under the shared semantic epoch"
        );
    }

    #[test]
    fn preprocessing_is_part_of_cache_identity_on_misses_and_hits() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let target_info = TargetInfo::native_release_fast();
        let cache_dir = std::env::temp_dir().join(format!(
            "molt-tir-prepared-function-cache-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&cache_dir);
        let base = FunctionIR {
            name: "prepared_cache_identity".to_string(),
            params: Vec::new(),
            ops: vec![OpIR {
                kind: "ret_void".to_string(),
                ..Default::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
            execution_context: crate::ExecutionContextPolicy::None,
        };
        let options = |cache_dir: &std::path::Path| TirPipelineRunOptions {
            target_info: target_info.clone(),
            cache_flavor: TirPipelineCacheFlavor::FactGraph,
            cache_dir: Some(cache_dir.to_path_buf()),
            process_externs: false,
            verify_lir: false,
            tir_dump: false,
            tir_stats: false,
            progress_prefix: None,
            resource_plan: TirOptimizationResourcePlan {
                threads: 1,
                wave_function_limit: 1,
                wave_op_budget: 16,
            },
        };
        let preprocessing_calls = AtomicUsize::new(0);
        let run_with_source = |source_file: &'static str| {
            let mut functions = vec![base.clone()];
            run_cached_tir_pipeline(&mut functions, options(&cache_dir), |function| {
                preprocessing_calls.fetch_add(1, Ordering::Relaxed);
                function.source_file = Some(source_file.to_string());
            })
        };
        let cached_source = |run: &TirPipelineRun| {
            run.cached_tir
                .optimized_tir_by_name
                .get("prepared_cache_identity")
                .and_then(|function| function.attrs.get(crate::tir::ops::SOURCE_FILE_ATTR))
                .cloned()
        };

        let first_miss = run_with_source("first.py");
        assert_eq!(first_miss.uncached_count, 1);
        assert_eq!(
            cached_source(&first_miss),
            Some(crate::tir::ops::AttrValue::Str("first.py".to_string()))
        );

        let changed_preprocessing_miss = run_with_source("second.py");
        assert_eq!(
            changed_preprocessing_miss.uncached_count, 1,
            "identical source IR with different prepared IR must not reuse a stale artifact"
        );
        assert_eq!(
            cached_source(&changed_preprocessing_miss),
            Some(crate::tir::ops::AttrValue::Str("second.py".to_string()))
        );

        let matching_preprocessing_hit = run_with_source("second.py");
        assert_eq!(matching_preprocessing_hit.uncached_count, 0);
        assert_eq!(
            cached_source(&matching_preprocessing_hit),
            Some(crate::tir::ops::AttrValue::Str("second.py".to_string()))
        );
        assert_eq!(
            preprocessing_calls.load(Ordering::Relaxed),
            3,
            "preparation must run before every lookup so hits and misses use the same identity"
        );
        let _ = std::fs::remove_dir_all(cache_dir);
    }

    #[test]
    fn cache_policy_separates_none_local_inherited_and_wrong_policy_preseed_cannot_hit() {
        let target_info = TargetInfo::native_release_fast();
        let cache_dir = std::env::temp_dir().join(format!(
            "molt-tir-execution-policy-cache-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&cache_dir);
        let base = FunctionIR {
            name: "policy_separation".to_string(),
            params: Vec::new(),
            ops: vec![OpIR {
                kind: "ret_void".to_string(),
                ..Default::default()
            }],
            param_types: None,
            source_file: Some("policy.py".to_string()),
            is_extern: false,
            execution_context: crate::ExecutionContextPolicy::None,
        };
        let hash = |policy| {
            let mut function = base.clone();
            function.execution_context = policy;
            content_hash_for_function(&function, TirPipelineCacheFlavor::FactGraph, &target_info)
        };
        assert_ne!(
            hash(crate::ExecutionContextPolicy::None),
            hash(crate::ExecutionContextPolicy::Local)
        );
        assert_ne!(
            hash(crate::ExecutionContextPolicy::None),
            hash(crate::ExecutionContextPolicy::Inherited)
        );
        assert_ne!(
            hash(crate::ExecutionContextPolicy::Local),
            hash(crate::ExecutionContextPolicy::Inherited)
        );

        let options = |cache_dir: &std::path::Path| TirPipelineRunOptions {
            target_info: target_info.clone(),
            cache_flavor: TirPipelineCacheFlavor::FactGraph,
            cache_dir: Some(cache_dir.to_path_buf()),
            process_externs: false,
            verify_lir: false,
            tir_dump: false,
            tir_stats: false,
            progress_prefix: None,
            resource_plan: TirOptimizationResourcePlan {
                threads: 1,
                wave_function_limit: 1,
                wave_op_budget: 16,
            },
        };
        let mut wrong_policy = vec![base.clone()];
        let seeded = run_cached_tir_pipeline(&mut wrong_policy, options(&cache_dir), |_| {});
        assert_eq!(seeded.uncached_count, 1);

        let mut requested = vec![FunctionIR {
            execution_context: crate::ExecutionContextPolicy::Inherited,
            ..base.clone()
        }];
        let miss = run_cached_tir_pipeline(&mut requested, options(&cache_dir), |_| {});
        assert_eq!(
            miss.uncached_count, 1,
            "a cache artifact preseeded under the wrong execution policy must not hit"
        );
        let hit = run_cached_tir_pipeline(&mut requested, options(&cache_dir), |_| {});
        assert_eq!(hit.uncached_count, 0);
        let _ = std::fs::remove_dir_all(cache_dir);
    }

    #[test]
    fn cached_tir_custody_preserves_function_source_file() {
        let mut functions = vec![FunctionIR {
            name: "molt_main".to_string(),
            params: Vec::new(),
            ops: vec![OpIR {
                kind: "ret_void".to_string(),
                ..Default::default()
            }],
            param_types: None,
            source_file: Some("app.py".to_string()),
            is_extern: false,
            execution_context: crate::ExecutionContextPolicy::Inherited,
        }];
        let target_info = TargetInfo::native_release_fast();
        let cache_dir =
            std::env::temp_dir().join(format!("molt-tir-source-file-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&cache_dir);
        let mut run = run_cached_tir_pipeline(
            &mut functions,
            TirPipelineRunOptions {
                target_info: target_info.clone(),
                cache_flavor: TirPipelineCacheFlavor::FactGraph,
                cache_dir: Some(cache_dir.clone()),
                process_externs: false,
                verify_lir: false,
                tir_dump: false,
                tir_stats: false,
                progress_prefix: None,
                resource_plan: TirOptimizationResourcePlan {
                    threads: 1,
                    wave_function_limit: 1,
                    wave_op_budget: 16,
                },
            },
            |_| {},
        );
        let non_inlinable = HashSet::new();
        let owned_run = run_owned_module_pipeline_from_cached_tir(
            &functions,
            &mut run.cached_tir,
            TirOwnedModulePipelineOptions {
                target_info: &target_info,
                module_name: "fact_graph_module",
                non_inlinable: &non_inlinable,
                missing_tir_context: "source-file test",
                mode: TirOwnedModulePipelineMode::TerminalDropsOnly,
            },
        );

        let (_, tir_func) = owned_run
            .tir_functions
            .iter()
            .find(|(_, func)| func.name == "molt_main")
            .expect("molt_main TIR");
        assert_eq!(
            tir_func.attrs.get(crate::tir::ops::SOURCE_FILE_ATTR),
            Some(&crate::tir::ops::AttrValue::Str("app.py".to_string()))
        );
        assert_eq!(
            tir_func.execution_context,
            crate::ExecutionContextPolicy::Inherited,
            "SimpleIR -> cached TIR custody must preserve the module execution-context ABI"
        );
        let _ = std::fs::remove_dir_all(cache_dir);
    }
}
