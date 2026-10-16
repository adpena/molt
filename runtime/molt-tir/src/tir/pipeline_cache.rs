//! Shared cached SimpleIR -> TIR optimization runner.
//!
//! Every backend uses this authority for cache keys, batching, hit restoration,
//! miss optimization, artifact encoding, and index persistence. SimpleIR consumers
//! request a bounded parallel projection; owned-TIR consumers borrow their input
//! and never materialize that unused projection.
//! Backend-specific code may prepare a function before lowering or consume the
//! optimized [`TirFunction`] afterward, but it must not open `CompilationCache`
//! directly for this pipeline.

use molt_ir::backend_environment::{self as backend_env, BackendEnv};
use rayon::prelude::*;
use std::borrow::Borrow;
use std::collections::{BTreeMap, BTreeSet, HashSet};

use crate::{FunctionIR, OpIR};

use super::cache::{CompilationCache, CompilationCacheKey, CompilationCacheWriteError};
use super::function::{TirFunction, TirModule};
use super::target_info::TargetInfo;

pub const TIR_OPTIMIZATION_BATCH_FUNCTION_LIMIT: usize = 128;
pub const TIR_OPTIMIZATION_BATCH_OP_BUDGET: usize = 8_000;
pub const TIR_OPTIMIZATION_BASELINE_MEMORY_BYTES: u64 = 4 * 1024 * 1024 * 1024;
pub const TIR_OPTIMIZATION_WORKER_MEMORY_BYTES: u64 = 8 * 1024 * 1024 * 1024;
pub const TIR_OPTIMIZATION_WAVE_FUNCTIONS_PER_THREAD: usize = 1;
pub const TIR_OPTIMIZATION_WAVE_OPS_PER_THREAD: usize = 1_000;

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

/// The representation required after per-function optimization.
///
/// Both variants retain the same optimized TIR and cache identity. Only
/// `SimpleIr` replaces source bodies with a SimpleIR projection, within each
/// bounded optimization wave. `TirOnly` leaves the borrowed input untouched.
pub enum TirPipelineInput<'a> {
    SimpleIr(&'a mut [FunctionIR]),
    TirOnly(&'a [FunctionIR]),
}

impl TirPipelineInput<'_> {
    fn functions(&self) -> &[FunctionIR] {
        match self {
            Self::SimpleIr(functions) => functions,
            Self::TirOnly(functions) => functions,
        }
    }

    fn install_simple_ops(&mut self, index: usize, ops: Option<Vec<OpIR>>) {
        match (self, ops) {
            (Self::SimpleIr(functions), Some(ops)) => functions[index].ops = ops,
            (Self::TirOnly(_), None) => {}
            _ => unreachable!("TIR output must match the requested representation"),
        }
    }
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
    simple_ops: Option<Vec<OpIR>>,
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
        lowering,
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
            "runtime_requirements={};lowering_tuple_scalarization={};",
            "pgo_hot_functions={};",
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
        lowering.non_escaping_tuple_scalarization,
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

/// Stack for compiler worker and daemon request threads: deep pass recursion
/// needs more than Rust's 2 MiB thread default. Only touched pages are
/// committed.
pub const COMPILE_THREAD_STACK_BYTES: usize = 64 << 20;

/// Pools this process keeps. Each pool has one width; widths differ only when
/// concurrent requests plan different thread counts.
const MAX_COMPILE_THREAD_POOLS: usize = 4;

/// The compiler pool with exactly `threads` workers, shared by every compile
/// in this process. A daemon running several compiles at once then keeps one
/// thread per planned worker instead of one pool per compile: the bound that
/// the make/Cargo jobserver gives concurrent rustc processes. The least
/// recently used pool leaves when a new width needs a slot; its threads exit
/// once the compiles that hold it finish.
pub fn compile_thread_pool(threads: usize) -> std::sync::Arc<rayon::ThreadPool> {
    static POOLS: std::sync::Mutex<Vec<(usize, std::sync::Arc<rayon::ThreadPool>)>> =
        std::sync::Mutex::new(Vec::new());
    let threads = threads.max(1);
    let mut pools = POOLS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let pool = match pools.iter().position(|(width, _)| *width == threads) {
        Some(index) => pools.remove(index).1,
        None => {
            if pools.len() == MAX_COMPILE_THREAD_POOLS {
                pools.remove(0);
            }
            std::sync::Arc::new(
                rayon::ThreadPoolBuilder::new()
                    .num_threads(threads)
                    .stack_size(COMPILE_THREAD_STACK_BYTES)
                    .thread_name(|index| format!("molt-compile-{index}"))
                    .build()
                    .expect("Failed to build compiler thread pool"),
            )
        }
    };
    pools.push((threads, std::sync::Arc::clone(&pool)));
    pool
}

/// The compiler pool for the current request's CPU limit
/// (`RAYON_NUM_THREADS`, else the core count).
pub fn request_thread_pool() -> std::sync::Arc<rayon::ThreadPool> {
    compile_thread_pool(tir_optimization_cpu_thread_limit())
}

/// Map `work` over `items` (a slice, by reference, or a `Vec`, by value) on
/// the request's compiler pool, each item inside the request's settings scope.
/// Results keep `items` order. Concurrent compiles share the pool's threads,
/// so a blocking item (a worker process) holds one slot of the request's CPU
/// limit, as a jobserver token would.
pub fn request_parallel_map<I, O>(items: I, work: impl Fn(I::Item) -> O + Sync + Send) -> Vec<O>
where
    I: rayon::iter::IntoParallelIterator + Send,
    I::Iter: rayon::iter::IndexedParallelIterator,
    O: Send,
{
    let request = molt_ir::backend_environment::carry();
    request_thread_pool().install(|| {
        items
            .into_par_iter()
            .map(|item| request.enter(|| work(item)))
            .collect()
    })
}

fn parse_positive_usize_env(key: molt_ir::backend_environment::BackendEnv) -> Option<usize> {
    molt_ir::backend_environment::var(key)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
}

pub fn tir_optimization_cpu_thread_limit() -> usize {
    parse_positive_usize_env(molt_ir::backend_environment::BackendEnv::RAYON_NUM_THREADS)
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
        molt_passes::memory_budget::backend_memory_limit_bytes(),
    )
}

pub fn trace_tir_function_enabled(name: &str) -> bool {
    backend_env::var(BackendEnv::MOLT_TIR_TRACE_FUNC)
        .ok()
        .is_some_and(|filter| filter == "1" || name.contains(&filter))
}

pub fn trace_tir_function_stage(name: &str, stage: &str, simple_ops: usize) {
    if trace_tir_function_enabled(name) {
        eprintln!("[TIR-TRACE] {name} {stage}: simple_ops={simple_ops}");
    }
}

pub fn run_cached_tir_pipeline<F>(
    mut input: TirPipelineInput<'_>,
    options: TirPipelineRunOptions<'_>,
    preprocess_before_lowering: F,
) -> TirPipelineRun
where
    F: Fn(&mut FunctionIR) + Sync,
{
    let project_simple_ir = matches!(input, TirPipelineInput::SimpleIr(_));
    // Reusing optimized TIR would suppress pass dumps, audits and instruments.
    let cache_reuse_allowed = !options.tir_dump
        && !options.tir_stats
        && !molt_ir::backend_environment::compilation_diagnostics_requested();
    let mut cached_tir_custody = CachedTirCustody::new();
    let mut tir_cache = options.cache_dir.clone().map_or_else(
        CompilationCache::open_backend_namespace,
        CompilationCache::open,
    );
    let work_items: Vec<TirPreparationWorkItem> = input
        .functions()
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
        // Pool workers serve concurrent compiles, so each work item runs
        // under its own compile's backend request scope.
        let request = backend_env::carry();
        let tir_pool = compile_thread_pool(options.resource_plan.threads);
        for (batch_idx, batch_items) in work_batches.into_iter().enumerate() {
            let inputs: Vec<TirOptimizationInput> = tir_pool.install(|| {
                batch_items
                    .par_iter()
                    .map(|work_item| {
                        request.enter(|| TirOptimizationInput {
                            index: work_item.index,
                            prepared_function: PreparedTirFunction::new(
                                &input.functions()[work_item.index],
                                options.cache_flavor,
                                &options.target_info,
                                &preprocess_before_lowering,
                            ),
                        })
                    })
                    .collect()
            });
            let prepared_ops = inputs
                .iter()
                .map(|input| input.prepared_function.function.ops.len())
                .sum::<usize>();
            let prepared_count = inputs.len();
            let mut misses = Vec::new();
            for prepared in inputs {
                let index = prepared.index;
                let content_hash = &prepared.prepared_function.content_hash;
                if cache_reuse_allowed
                    && let Some(cached_bytes) = tir_cache.get(content_hash)
                    && let Some(cached_tir_func) =
                        super::serialize::deserialize_tir_function(&cached_bytes)
                {
                    verify_lir_if_requested(&cached_tir_func, options.verify_lir);
                    let cached_ops = project_simple_ir.then(|| {
                        let ops = super::lower_to_simple::lower_to_simple_ir(&cached_tir_func);
                        assert!(
                            super::lower_to_simple::validate_labels(&ops),
                            "cached TIR back-conversion emitted invalid labels for '{}'",
                            cached_tir_func.name
                        );
                        ops
                    });
                    let func_ir = &input.functions()[index];
                    assert_eq!(
                        func_ir.return_abi, cached_tir_func.return_abi,
                        "cached TIR must preserve the authored function return ABI"
                    );
                    let name = func_ir.name.clone();
                    input.install_simple_ops(index, cached_ops);
                    cached_tir_custody.insert(name, cached_tir_func);
                } else {
                    misses.push(prepared);
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
                        request.enter(|| {
                            optimize_tir_input(
                                input,
                                &options.target_info,
                                options.tir_dump,
                                options.tir_stats,
                                options.verify_lir,
                                project_simple_ir,
                            )
                        })
                    })
                    .collect()
            });

            for output in results {
                let func_ir = &input.functions()[output.index];
                assert_eq!(
                    func_ir.return_abi, output.tir_func.return_abi,
                    "optimized TIR must preserve the authored function return ABI"
                );
                let name = func_ir.name.clone();
                input.install_simple_ops(output.index, output.simple_ops);
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
                                name
                            )
                        }
                        CompilationCacheWriteError::Unavailable(error) => {
                            eprintln!("MOLT_CACHE: TIR cache unavailable for '{}': {error}", name);
                        }
                    }
                }
                cached_tir_custody.insert(name, output.tir_func);
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

    if let Err(error) = tir_cache.trim_if_due() {
        eprintln!("MOLT_CACHE: persistent TIR cache trim failed: {error}");
    }
    TirPipelineRun {
        cached_tir: cached_tir_custody,
        uncached_count,
    }
}

/// Partition optimized bodies before the terminal ownership phase.
///
/// Only newly generated stubs/chunks lose their old function analysis. Every
/// unchanged function keeps its optimized TIR; its cache is neither rehashed
/// nor rebuilt. Transport allocations and reads must enter DropInsertion along
/// with the body they serve, never inherit another function's completed drops.
pub fn partition_cached_functions_before_drops(
    ir: &mut crate::SimpleIR,
    cached_tir: &mut CachedTirCustody,
    target_info: &TargetInfo,
    partition: impl FnOnce(&mut crate::SimpleIR) -> BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let sources = partition(ir);
    if sources.is_empty() {
        return sources;
    }
    let changed: HashSet<&str> = sources
        .keys()
        .chain(sources.values())
        .map(String::as_str)
        .collect();
    for function in &mut ir.functions {
        if !changed.contains(function.name.as_str()) {
            continue;
        }
        let mut tir = super::lower_from_simple::lower_to_tir_for_target(function, target_info);
        super::type_refine::refine_types(&mut tir);
        // The new function boundary changes SSA, dominance and parameter facts.
        // Optimize the changed bodies under the same target before module-wide
        // analysis; this pipeline deliberately does not insert terminal drops.
        super::passes::run_pipeline(&mut tir, target_info);
        super::type_refine::refine_types(&mut tir);
        function.ops = super::lower_to_simple::lower_to_simple_ir(&tir);
        cached_tir.insert(function.name.clone(), tir);
    }
    sources
}

pub fn run_simple_ir_module_pipeline_from_cached_tir(
    functions: &mut [FunctionIR],
    cached_tir: &mut CachedTirCustody,
    mut options: TirSimpleIrModulePipelineOptions<'_>,
) -> usize {
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
    // This duration exists solely for the optional stage observer.
    let module_pipeline_start = options
        .stage_observer
        .is_some()
        .then(std::time::Instant::now);
    let changed_functions = super::module_phase::run_module_pipeline(
        &mut module,
        options.target_info,
        options.non_inlinable,
    );
    if let Some(start) = module_pipeline_start {
        emit_simple_ir_module_stage(
            &mut options.stage_observer,
            TirSimpleIrModulePipelineStage::AfterModulePipeline {
                module: &module,
                changed_functions: changed_functions.len(),
                elapsed_ms: start.elapsed().as_millis(),
            },
        );
    }
    backconvert_changed_tir_module_to_simple_ir(
        functions,
        module,
        &idx_map,
        &changed_functions,
        options.backconvert_context,
    );
    emit_simple_ir_module_stage(
        &mut options.stage_observer,
        TirSimpleIrModulePipelineStage::AfterModuleBackconvert {
            functions,
            changed_functions: changed_functions.len(),
        },
    );
    changed_functions.len()
}

pub fn finalize_simple_ir_drops_from_cached_tir(
    functions: &mut [FunctionIR],
    target_info: &TargetInfo,
    cached_tir: &mut CachedTirCustody,
    consume_final_tir: impl FnMut(&TirFunction),
) {
    for function in functions.iter().filter(|function| !function.is_extern) {
        assert!(
            cached_tir.contains_function(&function.name),
            "cached terminal drops are missing typed custody for '{}'",
            function.name,
        );
    }
    super::drop_phase::finalize_simple_ir_drops_with_tir_custody(
        functions,
        target_info,
        cached_tir.optimized_tir_by_name_mut(),
        consume_final_tir,
    );
}

/// Consume source bodies during assembly when the caller owns them; borrowed
/// sources (for diagnostics) stay untouched. No local SimpleIR ops are read.
pub fn run_owned_module_pipeline_from_cached_tir<F: Borrow<FunctionIR>>(
    functions: impl IntoIterator<Item = F>,
    cached_tir: &mut CachedTirCustody,
    options: TirOwnedModulePipelineOptions<'_>,
) -> Vec<(bool, TirFunction)> {
    let mut tir_functions = take_ordered_tir_functions_from_cached_tir(
        functions,
        cached_tir,
        options.missing_tir_context,
    );
    match options.mode {
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
            super::module_phase::run_module_pipeline(
                &mut module,
                options.target_info,
                options.non_inlinable,
            );
            tir_functions = Vec::with_capacity(externs.len() + module.functions.len());
            tir_functions.extend(externs.into_iter().map(|func| (true, func)));
            tir_functions.extend(module.functions.into_iter().map(|func| (false, func)));
        }
        TirOwnedModulePipelineMode::TerminalDropsOnly => {
            for (is_extern, tir_func) in tir_functions.iter_mut() {
                if !*is_extern {
                    let _ =
                        super::drop_phase::finalize_function_drops(tir_func, options.target_info);
                }
            }
        }
    }
    tir_functions
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

fn take_ordered_tir_functions_from_cached_tir<F: Borrow<FunctionIR>>(
    functions: impl IntoIterator<Item = F>,
    cached_tir: &mut CachedTirCustody,
    missing_tir_context: &str,
) -> Vec<(bool, TirFunction)> {
    functions
        .into_iter()
        .map(|source| {
            let func_ir = source.borrow();
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
    module: TirModule,
    idx_map: &[usize],
    changed_functions: &BTreeSet<String>,
    backconvert_context: &str,
) {
    assert_eq!(
        module.functions.len(),
        idx_map.len(),
        "module optimization must preserve the local function index mapping"
    );
    // This is the terminal consumer of the module's TIR bodies. Retire each
    // body as its SimpleIR replacement is installed, including unchanged TIR.
    for (tir_func, &orig_idx) in module.functions.into_iter().zip(idx_map) {
        if !changed_functions.contains(tir_func.name.as_str()) {
            continue;
        }
        let function = &mut functions[orig_idx];
        assert_eq!(
            function.return_abi, tir_func.return_abi,
            "module optimization must preserve the authored function return ABI"
        );
        // The optimized TIR owns the replacement; lowering cannot read the old
        // SimpleIR body. Release it before allocating the replacement body.
        drop(std::mem::take(&mut function.ops));
        let ops = super::lower_to_simple::lower_to_simple_ir(&tir_func);
        debug_assert!(
            super::lower_to_simple::validate_labels(&ops),
            "{backconvert_context} back-conversion emitted invalid labels for '{}'",
            tir_func.name
        );
        function.ops = ops;
    }
}

fn optimize_tir_input(
    input: TirOptimizationInput,
    target_info: &TargetInfo,
    tir_dump: bool,
    tir_stats: bool,
    verify_lir: bool,
    project_simple_ir: bool,
) -> TirOptimizationOutput {
    let idx = input.index;
    let PreparedTirFunction {
        function: tmp_func,
        content_hash,
    } = input.prepared_function;

    let func_name = tmp_func.name.clone();
    let mut tir_func = super::lower_from_simple::lower_to_tir_for_target(&tmp_func, target_info);
    drop(tmp_func);
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
    let ops = project_simple_ir.then(|| {
        let ops = super::lower_to_simple::lower_to_simple_ir(&tir_func);
        trace_tir_function_stage(&func_name, "after_lower_to_simple", ops.len());
        assert!(
            super::lower_to_simple::validate_labels(&ops),
            "TIR roundtrip emitted invalid labels for '{}'",
            func_name
        );
        ops
    });
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

    fn projection_fixture() -> Vec<FunctionIR> {
        let constant = |name: &str, value| FunctionIR {
            name: name.into(),
            return_abi: molt_ir::FunctionReturnAbi::Value,
            source_file: Some("projection.py".into()),
            execution_context: crate::ExecutionContextPolicy::Inherited,
            ops: vec![
                OpIR {
                    kind: "const".into(),
                    value: Some(value),
                    out: Some("authored".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".into(),
                    args: Some(vec!["authored".into()]),
                    ..OpIR::default()
                },
            ],
            ..FunctionIR::default()
        };
        vec![
            constant("first", 41),
            FunctionIR {
                name: "external".into(),
                is_extern: true,
                return_abi: molt_ir::FunctionReturnAbi::Value,
                ..FunctionIR::default()
            },
            constant("last", 23),
        ]
    }

    fn projection_test_options(cache_dir: &std::path::Path) -> TirPipelineRunOptions<'static> {
        TirPipelineRunOptions {
            target_info: TargetInfo::native_release_fast(),
            cache_flavor: TirPipelineCacheFlavor::FactGraph,
            cache_dir: Some(cache_dir.to_path_buf()),
            process_externs: false,
            verify_lir: false,
            tir_dump: false,
            tir_stats: false,
            progress_prefix: None,
            resource_plan: TirOptimizationResourcePlan {
                threads: 2,
                wave_function_limit: 2,
                wave_op_budget: 16,
            },
        }
    }

    fn cached_body_bytes(run: &TirPipelineRun) -> Vec<(String, Vec<u8>)> {
        run.cached_tir
            .optimized_tir_by_name
            .iter()
            .map(|(name, tir)| {
                (
                    name.clone(),
                    super::super::serialize::serialize_tir_function(tir).unwrap(),
                )
            })
            .collect()
    }

    #[test]
    fn tir_only_and_simple_ir_share_cache_results_without_mutating_borrowed_input() {
        let cache_dir =
            std::env::temp_dir().join(format!("molt-tir-projection-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&cache_dir);
        let source = projection_fixture();
        let source_bytes = serde_json::to_vec(&source).unwrap();
        let direct = run_cached_tir_pipeline(
            TirPipelineInput::TirOnly(&source),
            projection_test_options(&cache_dir),
            |_| {},
        );
        assert_eq!(direct.uncached_count, 2);
        assert_eq!(
            direct
                .cached_tir
                .optimized_tir_by_name
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["first", "last"]
        );
        let expected_tir = cached_body_bytes(&direct);
        let mut projected = source.clone();
        let projected_hit = run_cached_tir_pipeline(
            TirPipelineInput::SimpleIr(&mut projected),
            projection_test_options(&cache_dir),
            |_| {},
        );
        assert_eq!(
            projected_hit.uncached_count, 0,
            "representation demand must not partition the cache"
        );
        assert_eq!(cached_body_bytes(&projected_hit), expected_tir);
        assert_eq!(
            serde_json::to_vec(&projected[1]).unwrap(),
            serde_json::to_vec(&source[1]).unwrap(),
            "extern is unchanged"
        );
        for (before, after) in source.iter().zip(&projected) {
            assert_eq!(
                before.function_signature().unwrap(),
                after.function_signature().unwrap()
            );
            assert_eq!(before.source_file, after.source_file);
        }
        let direct_hit = run_cached_tir_pipeline(
            TirPipelineInput::TirOnly(&source),
            projection_test_options(&cache_dir),
            |_| {},
        );
        assert_eq!(direct_hit.uncached_count, 0);
        assert_eq!(cached_body_bytes(&direct_hit), expected_tir);
        // A diagnostic request must execute even though both representation
        // consumers can use the already-populated cache on ordinary requests.
        let mut diagnostics = projection_test_options(&cache_dir);
        diagnostics.tir_stats = true;
        let diagnostic =
            run_cached_tir_pipeline(TirPipelineInput::TirOnly(&source), diagnostics, |_| {});
        assert_eq!(diagnostic.uncached_count, 2);
        assert_eq!(cached_body_bytes(&diagnostic), expected_tir);
        // An independently cold projection must produce the same typed bodies.
        let cold_dir = cache_dir.join("cold-projection");
        let mut cold_source = source.clone();
        let projected_miss = run_cached_tir_pipeline(
            TirPipelineInput::SimpleIr(&mut cold_source),
            projection_test_options(&cold_dir),
            |_| {},
        );
        assert_eq!(projected_miss.uncached_count, 2);
        assert_eq!(cached_body_bytes(&projected_miss), expected_tir);
        assert_eq!(
            serde_json::to_vec(&cold_source).unwrap(),
            serde_json::to_vec(&projected).unwrap()
        );
        assert_eq!(serde_json::to_vec(&source).unwrap(), source_bytes);
        let _ = std::fs::remove_dir_all(cache_dir);
    }

    #[test]
    fn every_representation_refuses_cached_and_optimized_return_abi_changes() {
        use std::panic::{AssertUnwindSafe, catch_unwind};
        for projected in [false, true] {
            for cached in [false, true] {
                let cache_dir = std::env::temp_dir().join(format!(
                    "molt-tir-projection-abi-{}-{projected}-{cached}",
                    std::process::id()
                ));
                let _ = std::fs::remove_dir_all(&cache_dir);
                let mut source = vec![FunctionIR {
                    name: "authored_void".into(),
                    return_abi: molt_ir::FunctionReturnAbi::Void,
                    ops: vec![OpIR {
                        kind: "ret_void".into(),
                        ..OpIR::default()
                    }],
                    ..FunctionIR::default()
                }];
                let before = serde_json::to_vec(&source).unwrap();
                if cached {
                    let options = projection_test_options(&cache_dir);
                    let key = content_hash_for_function(
                        &source[0],
                        options.cache_flavor,
                        &options.target_info,
                    );
                    let mut other_source = projection_fixture().remove(0);
                    other_source.name = source[0].name.clone();
                    let wrong = super::super::lower_from_simple::lower_to_tir(&other_source);
                    let mut cache = CompilationCache::open(cache_dir.clone());
                    cache
                        .put(
                            &key,
                            &super::super::serialize::serialize_tir_function(&wrong).unwrap(),
                        )
                        .unwrap();
                }
                let error = catch_unwind(AssertUnwindSafe(|| {
                    let input = if projected {
                        TirPipelineInput::SimpleIr(&mut source)
                    } else {
                        TirPipelineInput::TirOnly(&source)
                    };
                    run_cached_tir_pipeline(
                        input,
                        projection_test_options(&cache_dir),
                        |function| {
                            if !cached {
                                function.return_abi = molt_ir::FunctionReturnAbi::Value;
                                function.ops = projection_fixture().remove(0).ops;
                            }
                        },
                    )
                }))
                .expect_err("an optimized or cached body must not replace the authored ABI");
                let message = error
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| error.downcast_ref::<&str>().copied())
                    .unwrap();
                let expected = if cached {
                    "cached TIR must preserve the authored function return ABI"
                } else {
                    "optimized TIR must preserve the authored function return ABI"
                };
                assert!(message.contains(expected), "{message}");
                assert_eq!(
                    serde_json::to_vec(&source).unwrap(),
                    before,
                    "refusal must precede replacement"
                );
                let _ = std::fs::remove_dir_all(cache_dir);
            }
        }
    }

    #[test]
    fn owned_module_assembly_retires_each_source_and_matches_borrowed_order_and_metadata() {
        use std::cell::Cell;
        use std::rc::Rc;
        struct Source {
            function: FunctionIR,
            drops: Rc<Cell<usize>>,
        }
        impl Borrow<FunctionIR> for Source {
            fn borrow(&self) -> &FunctionIR {
                &self.function
            }
        }
        impl Drop for Source {
            fn drop(&mut self) {
                self.drops.set(self.drops.get() + 1);
            }
        }
        let functions = projection_fixture();
        let original = serde_json::to_vec(&functions).unwrap();
        let target_info = TargetInfo::native_release_fast();
        let non_inlinable = HashSet::new();
        let cache = || {
            let mut cache = CachedTirCustody::new();
            for f in functions.iter().filter(|f| !f.is_extern) {
                cache.insert(
                    f.name.clone(),
                    super::super::lower_from_simple::lower_to_tir(f),
                );
            }
            cache
        };
        let bytes = |functions: &[(bool, TirFunction)]| {
            functions
                .iter()
                .map(|(external, f)| {
                    (
                        *external,
                        super::super::serialize::serialize_tir_function(f).unwrap(),
                    )
                })
                .collect::<Vec<_>>()
        };
        for mode in [
            TirOwnedModulePipelineMode::TerminalDropsOnly,
            TirOwnedModulePipelineMode::ModulePhase,
        ] {
            let options = || TirOwnedModulePipelineOptions {
                target_info: &target_info,
                module_name: "owned_source",
                non_inlinable: &non_inlinable,
                missing_tir_context: "owned source test",
                mode,
            };
            let borrowed =
                run_owned_module_pipeline_from_cached_tir(&functions, &mut cache(), options());
            let drops = Rc::new(Cell::new(0));
            let sources = functions
                .clone()
                .into_iter()
                .enumerate()
                .map(|(index, function)| {
                    assert_eq!(
                        drops.get(),
                        index,
                        "previous source must retire before the next is requested"
                    );
                    Source {
                        function,
                        drops: Rc::clone(&drops),
                    }
                });
            let owned = run_owned_module_pipeline_from_cached_tir(sources, &mut cache(), options());
            assert_eq!(drops.get(), 3);
            assert_eq!(bytes(&owned), bytes(&borrowed));
            let expected = match mode {
                TirOwnedModulePipelineMode::TerminalDropsOnly => vec!["first", "external", "last"],
                TirOwnedModulePipelineMode::ModulePhase => vec!["external", "first", "last"],
            };
            assert_eq!(
                owned
                    .iter()
                    .map(|(_, f)| f.name.as_str())
                    .collect::<Vec<_>>(),
                expected
            );
            assert_eq!(owned.iter().filter(|(external, _)| *external).count(), 1);
            for (_, function) in &owned {
                assert_eq!(function.return_abi, molt_ir::FunctionReturnAbi::Value);
            }
        }
        assert_eq!(serde_json::to_vec(&functions).unwrap(), original);
    }

    #[test]
    fn module_backconversion_preserves_selective_bodies_metadata_and_extern_indices() {
        let constant_function = |name: &str, value| FunctionIR {
            name: name.into(),
            return_abi: molt_ir::FunctionReturnAbi::Value,
            source_file: Some("module_lifetime.py".into()),
            execution_context: crate::ExecutionContextPolicy::Inherited,
            ops: vec![
                OpIR {
                    kind: "const".into(),
                    value: Some(value),
                    out: Some("result".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".into(),
                    args: Some(vec!["result".into()]),
                    ..OpIR::default()
                },
            ],
            ..FunctionIR::default()
        };
        let external = |name: &str| FunctionIR {
            name: name.into(),
            is_extern: true,
            return_abi: molt_ir::FunctionReturnAbi::Value,
            ..FunctionIR::default()
        };
        let mut functions = vec![
            external("external_before"),
            constant_function("changed_left", 11),
            constant_function("unchanged", 22),
            external("external_between"),
            constant_function("changed_right", 33),
        ];
        let before = serde_json::to_value(&functions).unwrap();
        let module = TirModule {
            name: "selective_module".into(),
            functions: vec![
                super::super::lower_from_simple::lower_to_tir(&constant_function(
                    "changed_left",
                    101,
                )),
                super::super::lower_from_simple::lower_to_tir(&functions[2]),
                super::super::lower_from_simple::lower_to_tir(&constant_function(
                    "changed_right",
                    303,
                )),
            ],
        };
        backconvert_changed_tir_module_to_simple_ir(
            &mut functions,
            module,
            &[1, 2, 4],
            &BTreeSet::from(["changed_right".into(), "changed_left".into()]),
            "selective module test",
        );
        let after = serde_json::to_value(&functions).unwrap();
        for index in [0, 2, 3] {
            assert_eq!(after[index], before[index], "unchanged or extern body");
        }
        for (index, value) in [(1, 101), (4, 303)] {
            let constants: Vec<_> = functions[index]
                .ops
                .iter()
                .filter(|op| op.kind == "const")
                .map(|op| op.value)
                .collect();
            assert_eq!(constants, vec![Some(value)]);
            assert!(functions[index].ops.iter().any(|op| op.kind == "ret"));
            let mut old_metadata = before[index].clone();
            let mut new_metadata = after[index].clone();
            old_metadata.as_object_mut().unwrap().remove("ops");
            new_metadata.as_object_mut().unwrap().remove("ops");
            assert_eq!(new_metadata, old_metadata, "authored function metadata");
        }
    }

    #[test]
    fn module_backconversion_refuses_abi_change_before_retiring_simple_ir() {
        let mut functions = vec![FunctionIR {
            name: "authored_void".into(),
            return_abi: molt_ir::FunctionReturnAbi::Void,
            ops: vec![OpIR {
                kind: "ret_void".into(),
                ..OpIR::default()
            }],
            ..FunctionIR::default()
        }];
        let before = serde_json::to_value(&functions).unwrap();
        let mut tir = super::super::lower_from_simple::lower_to_tir(&functions[0]);
        tir.return_abi = molt_ir::FunctionReturnAbi::Value;
        let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            backconvert_changed_tir_module_to_simple_ir(
                &mut functions,
                TirModule {
                    name: "invalid_abi".into(),
                    functions: vec![tir],
                },
                &[0],
                &BTreeSet::from(["authored_void".into()]),
                "ABI refusal test",
            );
        }));
        assert!(
            failure.is_err(),
            "a module pass cannot change the return ABI"
        );
        assert_eq!(serde_json::to_value(&functions).unwrap(), before);
    }

    #[test]
    fn module_backconversion_refuses_incomplete_local_index_mapping() {
        for local_count in [0, 2] {
            let mut functions = vec![FunctionIR {
                name: "mapped".into(),
                return_abi: molt_ir::FunctionReturnAbi::Void,
                ops: vec![OpIR {
                    kind: "ret_void".into(),
                    ..OpIR::default()
                }],
                ..FunctionIR::default()
            }];
            let before = serde_json::to_value(&functions).unwrap();
            let module = TirModule {
                name: "invalid_mapping".into(),
                functions: (0..local_count)
                    .map(|_| super::super::lower_from_simple::lower_to_tir(&functions[0]))
                    .collect(),
            };
            let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                backconvert_changed_tir_module_to_simple_ir(
                    &mut functions,
                    module,
                    &[0],
                    &BTreeSet::new(),
                    "index mapping refusal test",
                );
            }));
            assert!(failure.is_err(), "local mapping cannot silently truncate");
            assert_eq!(serde_json::to_value(&functions).unwrap(), before);
        }
    }

    #[test]
    fn cached_partition_preserves_unmodified_tir_and_plans_every_new_transport() {
        for target in [
            TargetInfo::native_release_fast(),
            TargetInfo::wasm_release_fast(),
        ] {
            let large = FunctionIR {
                name: "split_transport_owner".into(),
                execution_context: crate::ExecutionContextPolicy::Inherited,
                return_abi: molt_ir::FunctionReturnAbi::Value,
                params: vec!["input".into()],
                ops: vec![
                    OpIR {
                        kind: "line".into(),
                        value: Some(1),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "list_new".into(),
                        args: Some(vec!["input".into()]),
                        out: Some("kept".into()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "line".into(),
                        value: Some(2),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "list_new".into(),
                        args: Some(vec!["kept".into()]),
                        out: Some("outer".into()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".into(),
                        args: Some(vec!["outer".into()]),
                        ..OpIR::default()
                    },
                ],
                ..FunctionIR::default()
            };
            let unchanged = FunctionIR {
                name: "unchanged".into(),
                return_abi: molt_ir::FunctionReturnAbi::Value,
                params: vec!["arg".into()],
                ops: vec![OpIR {
                    kind: "ret".into(),
                    args: Some(vec!["arg".into()]),
                    ..OpIR::default()
                }],
                ..FunctionIR::default()
            };
            let mut ir = crate::SimpleIR {
                functions: vec![large, unchanged],
                profile: None,
            };
            crate::validate_simple_ir(&ir).expect("valid source before outlining");
            let mut cache = CachedTirCustody::new();
            for function in &ir.functions {
                cache.insert(
                    function.name.clone(),
                    super::super::lower_from_simple::lower_to_tir_for_target(function, &target),
                );
            }
            let unchanged_params = cache.optimized_tir_by_name["unchanged"]
                .param_names
                .as_ptr();
            let sources =
                partition_cached_functions_before_drops(&mut ir, &mut cache, &target, |ir| {
                    let original = ir.functions.remove(0);
                    let mut names = ir.functions.iter().map(|f| f.name.clone()).collect();
                    let (stub, chunks) =
                        crate::passes::split_large_function(original, 3, &mut names)
                            .expect("fixture must exercise actual cross-chunk transport");
                    assert!(
                        chunks
                            .iter()
                            .any(|chunk| chunk.ops.iter().any(|op| op.kind == "index"))
                    );
                    let sources = chunks
                        .iter()
                        .map(|chunk| (chunk.name.clone(), stub.name.clone()))
                        .collect();
                    ir.functions.extend(chunks);
                    ir.functions.push(stub);
                    sources
                });
            assert!(!sources.is_empty());
            assert_eq!(
                unchanged_params,
                cache.optimized_tir_by_name["unchanged"]
                    .param_names
                    .as_ptr()
            );
            for function in &ir.functions {
                assert!(cache.contains_function(&function.name));
                assert!(
                    !cache.optimized_tir_by_name[&function.name]
                        .attrs
                        .contains_key(super::super::passes::drop_insertion::DROP_INSERTED_ATTR)
                );
            }
            finalize_simple_ir_drops_from_cached_tir(
                &mut ir.functions,
                &target,
                &mut cache,
                |_| {},
            );
            for function in &ir.functions {
                assert!(
                    function
                        .ops
                        .iter()
                        .any(|op| op.kind
                            == super::super::passes::drop_insertion::DROP_INSERTED_ATTR),
                    "{} must receive its own terminal drop plan",
                    function.name
                );
            }
        }
    }

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
            return_abi: molt_ir::FunctionReturnAbi::Void,
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
            codegen_partition: false,
            parameter_custody: Vec::new(),
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
        changed.return_abi = molt_ir::FunctionReturnAbi::Value;
        contract_mutations.push(changed);
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
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "prepared_cache_identity".to_string(),
            params: Vec::new(),
            ops: vec![OpIR {
                kind: "ret_void".to_string(),
                ..Default::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
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
            run_cached_tir_pipeline(
                TirPipelineInput::SimpleIr(&mut functions),
                options(&cache_dir),
                |function| {
                    preprocessing_calls.fetch_add(1, Ordering::Relaxed);
                    function.source_file = Some(source_file.to_string());
                },
            )
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
        for (tir_dump, tir_stats) in [(true, false), (false, true)] {
            let mut diagnostic_options = options(&cache_dir);
            diagnostic_options.tir_dump = tir_dump;
            diagnostic_options.tir_stats = tir_stats;
            let mut functions = vec![base.clone()];
            let diagnostic_run = run_cached_tir_pipeline(
                TirPipelineInput::SimpleIr(&mut functions),
                diagnostic_options,
                |function| function.source_file = Some("second.py".to_string()),
            );
            assert_eq!(
                diagnostic_run.uncached_count, 1,
                "requested pass diagnostics must execute even with matching cached TIR"
            );
            assert_eq!(
                cached_source(&diagnostic_run),
                cached_source(&matching_preprocessing_hit)
            );
        }
        assert_eq!(
            run_with_source("second.py").uncached_count,
            0,
            "diagnostic execution must preserve ordinary TIR cache reuse"
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
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "policy_separation".to_string(),
            params: Vec::new(),
            ops: vec![OpIR {
                kind: "ret_void".to_string(),
                ..Default::default()
            }],
            param_types: None,
            source_file: Some("policy.py".to_string()),
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
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
        let seeded = run_cached_tir_pipeline(
            TirPipelineInput::SimpleIr(&mut wrong_policy),
            options(&cache_dir),
            |_| {},
        );
        assert_eq!(seeded.uncached_count, 1);

        let mut requested = vec![FunctionIR {
            execution_context: crate::ExecutionContextPolicy::Inherited,
            ..base.clone()
        }];
        let miss = run_cached_tir_pipeline(
            TirPipelineInput::SimpleIr(&mut requested),
            options(&cache_dir),
            |_| {},
        );
        assert_eq!(
            miss.uncached_count, 1,
            "a cache artifact preseeded under the wrong execution policy must not hit"
        );
        let hit = run_cached_tir_pipeline(
            TirPipelineInput::SimpleIr(&mut requested),
            options(&cache_dir),
            |_| {},
        );
        assert_eq!(hit.uncached_count, 0);
        let _ = std::fs::remove_dir_all(cache_dir);
    }

    #[test]
    fn cached_tir_custody_preserves_function_source_file() {
        let mut functions = vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "molt_main".to_string(),
            params: Vec::new(),
            ops: vec![OpIR {
                kind: "ret_void".to_string(),
                ..Default::default()
            }],
            param_types: None,
            source_file: Some("app.py".to_string()),
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: crate::ExecutionContextPolicy::Inherited,
        }];
        let target_info = TargetInfo::native_release_fast();
        let cache_dir =
            std::env::temp_dir().join(format!("molt-tir-source-file-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&cache_dir);
        let mut run = run_cached_tir_pipeline(
            TirPipelineInput::SimpleIr(&mut functions),
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
